//! Archives sealed with the backup passphrase.
//!
//! An archive carries the master key, which opens every credential the
//! database holds, so a copy of the backup folder (an off-site sync, a NAS
//! share) is worth as much as the installation itself. With a passphrase set,
//! every archive is an age file (age-encryption.org/v1) sealed for it: worth
//! nothing without the passphrase, and opened with `age -d` where Routarr
//! cannot run.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use age::secrecy::{ExposeSecret, SecretString};
use tracing::{info, warn};

use super::{Scaffold, backup_dir, list, partial_path};
use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::state::AppState;

/// The setting that holds the passphrase, sealed with the master key.
pub const SETTING: &str = "backup_passphrase";

/// What a sealed archive's name ends with, after `.zip`.
pub const SUFFIX: &str = ".age";

/// The scrypt cost an archive is sealed with: 2^17, 128 MiB, OWASP's floor
/// for scrypt. Fixed rather than calibrated by `age` on the machine that seals:
/// sealed on a fast one, the archive would be refused by the slow one that
/// restores it.
const SEALING_WORK: u8 = 17;

/// The highest scrypt cost an archive may ask to be opened, one step above
/// what Routarr seals with, for an archive sealed again with the `age` tool.
const OPENING_WORK: u8 = 18;

/// What every age file starts with.
const MAGIC: &[u8] = b"age-encryption.org/v1\n";

/// Whether an archive's name says it is sealed.
pub fn is_sealed_name(name: &str) -> bool {
    name.ends_with(SUFFIX)
}

/// Whether `path` holds an age file, whatever it is called.
pub(super) fn is_sealed_file(path: &Path) -> bool {
    let mut start = [0u8; MAGIC.len()];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut start)).is_ok()
        && start == MAGIC
}

/// The passphrase the settings hold, opened. `None` when none is set, and an
/// error when one is set and cannot be opened: an archive taken in the clear
/// in its place would carry the credentials the operator asked to seal.
pub async fn passphrase(state: &AppState) -> AppResult<Option<SecretString>> {
    let settings = state.settings().await;
    let Some(sealed) = settings.raw(SETTING).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let opened = state
        .secrets
        .open(sealed)
        .map_err(|e| AppError::Internal(format!("the backup passphrase cannot be opened: {e}")))?;
    Ok(Some(SecretString::from(opened)))
}

/// The passphrase a database holds, for the work done before the server
/// builds its state: the backup before a migration, and the restore with the
/// server stopped. The master key is read where it is, never generated: a
/// start that finds it missing has to refuse.
pub async fn stored_passphrase(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
) -> AppResult<Option<SecretString>> {
    let sealed: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(SETTING)
        .fetch_optional(pool)
        .await?;
    let Some(sealed) = sealed.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(key) = super::key_in_place(config, &config.secret_key_path()) else {
        return Err(AppError::Internal(
            "the backup passphrase is sealed with a master key that is not here".into(),
        ));
    };
    let salt: Option<String> = sqlx::query_scalar("SELECT salt FROM secret_salt LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let secrets = crate::crypto::SecretBox::load(
        Some(&key),
        config.previous_secret_key.as_deref(),
        &config.secret_key_path(),
        salt.as_deref().map(str::as_bytes),
    )?;
    Ok(Some(SecretString::from(secrets.open(&sealed)?)))
}

/// Seal `plain` into `sealed` for `passphrase`, a stream at a time: an archive
/// is the whole database. Synchronous: the caller runs it on a blocking thread.
pub(super) fn seal(plain: &Path, sealed: &Path, passphrase: &SecretString) -> AppResult<()> {
    let failed = |e: &dyn std::fmt::Display| {
        AppError::Internal(format!("cannot seal {}: {e}", sealed.display()))
    };
    let mut recipient = age::scrypt::Recipient::new(passphrase.clone());
    recipient.set_work_factor(SEALING_WORK);
    let encryptor = age::Encryptor::with_recipients(std::iter::once(&recipient as _))
        .map_err(|e| failed(&e))?;
    let file = crate::crypto::create_private(sealed, false).map_err(|e| failed(&e))?;
    let mut writer = encryptor.wrap_output(file).map_err(|e| failed(&e))?;
    let mut source = std::fs::File::open(plain).map_err(|e| failed(&e))?;
    std::io::copy(&mut source, &mut writer).map_err(|e| failed(&e))?;
    let file = writer.finish().map_err(|e| failed(&e))?;
    file.sync_all().map_err(|e| failed(&e))
}

/// Open `sealed` into `plain` with `passphrase`. `Ok(false)` when the
/// passphrase does not open it, which only the person restoring can mend.
pub(super) fn open(sealed: &Path, plain: &Path, passphrase: &SecretString) -> AppResult<bool> {
    let Some(mut reader) = reader(sealed, passphrase)? else {
        return Ok(false);
    };
    let mut out = crate::crypto::create_private(plain, false)
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", plain.display())))?;
    std::io::copy(&mut reader, &mut out).map_err(|e| match e.kind() {
        // A chunk whose tag does not match: the file was altered or cut.
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof => {
            AppError::BadRequest(format!("the archive is damaged: {e}"))
        }
        _ => AppError::Internal(format!("cannot write {}: {e}", plain.display())),
    })?;
    out.flush().ok();
    out.sync_all()
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", plain.display())))?;
    Ok(true)
}

/// Whether `passphrase` opens `sealed`. Reads the header alone: the payload is
/// left unread.
fn opens(sealed: &Path, passphrase: &SecretString) -> AppResult<bool> {
    Ok(reader(sealed, passphrase)?.is_some())
}

type Reader = age::stream::StreamReader<std::io::BufReader<std::fs::File>>;

fn reader(sealed: &Path, passphrase: &SecretString) -> AppResult<Option<Reader>> {
    let file =
        std::fs::File::open(sealed).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let decryptor = age::Decryptor::new_buffered(std::io::BufReader::new(file))
        .map_err(|e| AppError::BadRequest(format!("not a readable archive: {e}")))?;
    let mut identity = age::scrypt::Identity::new(passphrase.clone());
    identity.set_max_work_factor(OPENING_WORK);
    match decryptor.decrypt(std::iter::once(&identity as _)) {
        Ok(reader) => Ok(Some(reader)),
        Err(age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys) => Ok(None),
        Err(age::DecryptError::ExcessiveWork { required, .. }) => Err(AppError::BadRequest(
            format!("the archive asks a scrypt cost of 2^{required}, above what a restore accepts"),
        )),
        Err(e) => Err(AppError::BadRequest(format!("not a readable archive: {e}"))),
    }
}

/// A sealed archive opened beside itself, removed with the value returned.
pub(super) struct Opened {
    pub path: PathBuf,
    _scaffold: Scaffold,
}

/// Open the sealed `archive` with the first of `passphrases` that opens it,
/// into a hidden file beside it. `Ok(None)` when none does, or none was given.
pub(super) fn opened_copy(
    archive: &Path,
    passphrases: &[&SecretString],
) -> AppResult<Option<Opened>> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let path = archive.with_file_name(format!(".{name}.opened"));
    let scaffold = Scaffold(vec![path.clone()]);
    for passphrase in passphrases {
        if open(archive, &path, passphrase)? {
            return Ok(Some(Opened { path, _scaffold: scaffold }));
        }
    }
    Ok(None)
}

/// What bringing the archives to a new passphrase did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resealed {
    /// Archives sealed, sealed again or opened.
    pub changed: usize,
    /// Sealed archives neither passphrase opens, left as they are.
    pub left: usize,
}

/// Bring every archive on disk to the passphrase now set: seal those in the
/// clear, seal again those `old` sealed, and open them all when the
/// passphrase is removed. An archive neither passphrase opens, sealed with an
/// older one, is left as it is and counted.
///
/// After any backup already running, under the same lock: an archive taken
/// meanwhile would be missed, or found half written.
pub async fn reseal(
    state: &AppState,
    old: Option<SecretString>,
    new: Option<SecretString>,
    by: &Attribution,
) -> AppResult<Resealed> {
    let Some(_lock) = state.jobs.lock_within("backup", std::time::Duration::from_secs(3600)).await
    else {
        return Err(AppError::Conflict("A backup held the folder for an hour".into()));
    };
    let job =
        state.jobs.start(JobKind::Backup, by, None, Detail::new("JobDetailResealing")).await?;
    let dir = backup_dir(state);
    let names: Vec<String> = list(state).into_iter().map(|file| file.name).collect();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut resealed = Resealed::default();
        for name in names {
            match reseal_one(&dir, &name, old.as_ref(), new.as_ref())? {
                Some(true) => resealed.changed += 1,
                Some(false) => {}
                None => resealed.left += 1,
            }
        }
        Ok::<_, AppError>(resealed)
    })
    .await
    .map_err(|e| AppError::Internal(format!("the archive task failed: {e}")))?;

    match &outcome {
        Ok(resealed) => {
            info!(
                "Archives brought to the backup passphrase: {} changed, {} left as they were",
                resealed.changed, resealed.left
            );
            let detail = Detail::new("JobDetailResealed")
                .with("archives", resealed.changed)
                .with("left", resealed.left);
            job.succeed(detail).await;
        }
        Err(e) => job.fail(e).await,
    }
    outcome
}

/// Bring one archive to `new`. `Some(true)` when it changed, `Some(false)`
/// when it already was, `None` when neither passphrase opens it.
fn reseal_one(
    dir: &Path,
    name: &str,
    old: Option<&SecretString>,
    new: Option<&SecretString>,
) -> AppResult<Option<bool>> {
    let path = dir.join(name);
    let sealed = is_sealed_name(name);
    let plain_name = name.strip_suffix(SUFFIX).unwrap_or(name);
    let target = match new {
        Some(_) => dir.join(format!("{plain_name}{SUFFIX}")),
        None => dir.join(plain_name),
    };
    let partial = partial_path(&target);
    std::fs::remove_file(&partial).ok();
    let scaffold = Scaffold(vec![partial.clone()]);

    match (sealed, new) {
        (false, None) => return Ok(Some(false)),
        (false, Some(new)) => seal(&path, &partial, new)?,
        (true, _) => {
            // Already sealed for the new passphrase, as an interrupted pass
            // leaves the archives it reached.
            if let Some(new) = new
                && opens(&path, new)?
            {
                return Ok(Some(false));
            }
            let Some(old) = old else { return Ok(None) };
            match new {
                None => {
                    if !open(&path, &partial, old)? {
                        return Ok(None);
                    }
                }
                Some(new) => {
                    let Some(opened) = opened_copy(&path, &[old])? else {
                        return Ok(None);
                    };
                    seal(&opened.path, &partial, new)?;
                }
            }
        }
    }

    std::fs::rename(&partial, &target)
        .map_err(|e| AppError::Internal(format!("cannot name {}: {e}", target.display())))?;
    scaffold.keep();
    crate::crypto::sync_parent(&target);
    if target != path
        && let Err(e) = std::fs::remove_file(&path)
    {
        warn!("{} was brought to the passphrase, and the old file stays: {e}", target.display());
    }
    Ok(Some(true))
}

/// The pass that brings the archives to a passphrase changed in the settings,
/// on a task of its own: the save answers at once, and the pass shows on the
/// Tasks screen.
pub fn reseal_in_background(
    state: &AppState,
    old: Option<SecretString>,
    new: Option<SecretString>,
    by: Attribution,
) {
    let same = match (&old, &new) {
        (Some(old), Some(new)) => old.expose_secret() == new.expose_secret(),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = reseal(&state, old, new, &by).await {
            warn!("The archives could not be brought to the backup passphrase: {e}");
        }
    });
}
