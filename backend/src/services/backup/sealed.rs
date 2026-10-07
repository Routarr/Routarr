//! Archives sealed with the backup passphrase.
//!
//! An archive carries the master key, which opens every credential the
//! database holds, so a copy of the backup folder (an off-site sync, a NAS
//! share) is worth as much as the installation itself. With a passphrase set,
//! every archive is sealed for it, and worth nothing without it.
//!
//! The format, all of it read before a byte is opened:
//!
//! ```text
//! MAGIC (8) | Argon2id memory KiB, passes, lanes (3 x u32 LE) | salt (16)
//!   | STREAM nonce prefix (7) | check (16) | chunks
//! ```
//!
//! The passphrase and the salt give, through Argon2id, the AES-256-GCM key and
//! a key for `check`, an HMAC of the header before it: a passphrase that does
//! not give the same check is the wrong one, said before anything is opened.
//! The archive follows in chunks of [`CHUNK`] bytes, each sealed with the
//! STREAM construction (Hoang, Reyhanitabar, Rogaway, Vizár, 2015) and the
//! whole header as associated data, the last flagged as last: a chunk altered,
//! moved, dropped or added after the end does not open.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aead_stream::{DecryptorBE32, EncryptorBE32};
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{KeyInit, Payload};
use hmac::Mac;
use tracing::{info, warn};

use super::{Scaffold, backup_dir, list, partial_path};
use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::state::AppState;

/// A passphrase, wiped from memory when dropped.
pub type Passphrase = zeroize::Zeroizing<String>;

/// The setting that holds the passphrase, sealed with the master key.
pub const SETTING: &str = "backup_passphrase";

/// What a sealed archive's name ends with, after `.zip`.
pub const SUFFIX: &str = ".enc";

/// What every sealed archive starts with: the format, and its version.
const MAGIC: &[u8; 8] = b"RTRSEAL\x01";

const SALT_LEN: usize = 16;

/// The nonce prefix of STREAM over AES-GCM's 96-bit nonce, the rest being a
/// 32-bit counter and the last-chunk flag.
const PREFIX_LEN: usize = 7;

const CHECK_LEN: usize = 16;

const HEADER_LEN: usize = MAGIC.len() + 12 + SALT_LEN + PREFIX_LEN + CHECK_LEN;

/// How much of the archive one chunk seals.
const CHUNK: usize = 64 * 1024;

const TAG_LEN: usize = 16;

/// What an archive's key is derived with: Argon2id at 64 MiB and three
/// passes, above OWASP's floor of 19 MiB and two, paid once per archive sealed
/// or opened. Written in each archive, so a later change opens the old ones.
const SEALING_COST: Cost = Cost { memory_kib: 64 * 1024, passes: 3, lanes: 1 };

/// The most an archive may ask to be opened with: a header written by hand
/// could otherwise ask for terabytes of memory.
const OPENING_LIMIT: Cost = Cost { memory_kib: 256 * 1024, passes: 10, lanes: 4 };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cost {
    memory_kib: u32,
    passes: u32,
    lanes: u32,
}

/// Whether an archive's name says it is sealed.
pub fn is_sealed_name(name: &str) -> bool {
    name.ends_with(SUFFIX)
}

/// Whether `path` holds a sealed archive, whatever it is called.
pub(super) fn is_sealed_file(path: &Path) -> bool {
    let mut start = [0u8; MAGIC.len()];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut start)).is_ok()
        && &start == MAGIC
}

/// The passphrase the settings hold, opened. `None` when none is set, and an
/// error when one is set and cannot be opened: an archive taken in the clear
/// in its place would carry the credentials the operator asked to seal.
pub async fn passphrase(state: &AppState) -> AppResult<Option<Passphrase>> {
    let settings = state.settings().await;
    let Some(sealed) = settings.raw(SETTING).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let opened = state
        .secrets
        .open(sealed)
        .map_err(|e| AppError::Internal(format!("the backup passphrase cannot be opened: {e}")))?;
    Ok(Some(Passphrase::new(opened)))
}

/// The passphrase a database holds, for the work done before the server
/// builds its state: the backup before a migration, and the commands run
/// with the server stopped. The master key is read where it is, never
/// generated: a start that finds it missing has to refuse.
pub async fn stored_passphrase(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
) -> AppResult<Option<Passphrase>> {
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
    Ok(Some(Passphrase::new(secrets.open(&sealed)?)))
}

/// The AES-256-GCM key and the check key `passphrase` gives with `salt`.
fn keys(
    passphrase: &Passphrase,
    salt: &[u8],
    cost: Cost,
) -> AppResult<zeroize::Zeroizing<[u8; 64]>> {
    let params =
        argon2::Params::new(cost.memory_kib, cost.passes, cost.lanes, Some(64)).map_err(|e| {
            AppError::BadRequest(format!("the archive asks for a cost Argon2 refuses: {e}"))
        })?;
    let argon2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut keys = zeroize::Zeroizing::new([0u8; 64]);
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, keys.as_mut())
        .map_err(|e| AppError::Internal(format!("cannot derive the archive's key: {e}")))?;
    Ok(keys)
}

/// The check a header ends with: what tells a wrong passphrase from damage.
fn check(check_key: &[u8], header: &[u8]) -> AppResult<[u8; CHECK_LEN]> {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(check_key)
        .map_err(|e| AppError::Internal(format!("cannot check the archive's header: {e}")))?;
    mac.update(header);
    let mut check = [0u8; CHECK_LEN];
    check.copy_from_slice(&mac.finalize().into_bytes()[..CHECK_LEN]);
    Ok(check)
}

/// Read until `buffer` is full or the file ends, and say how much was read.
fn fill(source: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut read = 0;
    while read < buffer.len() {
        match source.read(&mut buffer[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(read)
}

/// Seal `plain` into `sealed` for `passphrase`, a chunk at a time: an archive
/// is the whole database. Synchronous: the caller runs it on a blocking thread.
pub(super) fn seal(plain: &Path, sealed: &Path, passphrase: &Passphrase) -> AppResult<()> {
    let failed = |e: &dyn std::fmt::Display| {
        AppError::Internal(format!("cannot seal {}: {e}", sealed.display()))
    };

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    for value in [SEALING_COST.memory_kib, SEALING_COST.passes, SEALING_COST.lanes] {
        header.extend_from_slice(&value.to_le_bytes());
    }
    let mut salt = [0u8; SALT_LEN];
    let mut prefix = [0u8; PREFIX_LEN];
    crate::crypto::random_bytes(&mut salt)?;
    crate::crypto::random_bytes(&mut prefix)?;
    header.extend_from_slice(&salt);
    header.extend_from_slice(&prefix);
    let keys = keys(passphrase, &salt, SEALING_COST)?;
    let check = check(&keys[32..], &header)?;
    header.extend_from_slice(&check);

    let cipher = Aes256Gcm::new_from_slice(&keys[..32]).map_err(|e| failed(&e))?;
    let mut encryptor = EncryptorBE32::from_aead(cipher, &prefix.into());
    let mut source = std::fs::File::open(plain).map_err(|e| failed(&e))?;
    let mut out = crate::crypto::create_private(sealed, false).map_err(|e| failed(&e))?;
    out.write_all(&header).map_err(|e| failed(&e))?;

    // A chunk is sealed as the last only once the one after it is found empty.
    let mut current = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut length = fill(&mut source, &mut current).map_err(|e| failed(&e))?;
    loop {
        let following = fill(&mut source, &mut next).map_err(|e| failed(&e))?;
        let payload = Payload { msg: &current[..length], aad: &header };
        if following == 0 {
            let chunk = encryptor.encrypt_last(payload).map_err(|e| failed(&e))?;
            out.write_all(&chunk).map_err(|e| failed(&e))?;
            break;
        }
        let chunk = encryptor.encrypt_next(payload).map_err(|e| failed(&e))?;
        out.write_all(&chunk).map_err(|e| failed(&e))?;
        std::mem::swap(&mut current, &mut next);
        length = following;
    }
    out.sync_all().map_err(|e| failed(&e))
}

/// A sealed archive read up to its first chunk, with the cipher that opens it.
struct Unsealer {
    file: std::fs::File,
    header: Vec<u8>,
    decryptor: DecryptorBE32<Aes256Gcm>,
}

/// The header of `sealed`, and the cipher `passphrase` gives for it. `None`
/// when the passphrase is not the one it was sealed with.
fn unsealer(sealed: &Path, passphrase: &Passphrase) -> AppResult<Option<Unsealer>> {
    let mut file =
        std::fs::File::open(sealed).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut header = vec![0u8; HEADER_LEN];
    if fill(&mut file, &mut header).map_err(|e| AppError::BadRequest(e.to_string()))? < HEADER_LEN
        || &header[..MAGIC.len()] != MAGIC
    {
        return Err(AppError::BadRequest("not a sealed archive this version can read".into()));
    }
    let number = |at: usize| {
        let start = MAGIC.len() + at * 4;
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&header[start..start + 4]);
        u32::from_le_bytes(bytes)
    };
    let cost = Cost { memory_kib: number(0), passes: number(1), lanes: number(2) };
    if cost.memory_kib > OPENING_LIMIT.memory_kib
        || cost.passes > OPENING_LIMIT.passes
        || cost.lanes > OPENING_LIMIT.lanes
    {
        return Err(AppError::BadRequest(format!(
            "the archive asks for a cost above what a restore accepts: {} KiB, {} passes, {} lanes",
            cost.memory_kib, cost.passes, cost.lanes
        )));
    }
    let salt_at = MAGIC.len() + 12;
    let salt = &header[salt_at..salt_at + SALT_LEN];
    let mut prefix = [0u8; PREFIX_LEN];
    prefix.copy_from_slice(&header[salt_at + SALT_LEN..salt_at + SALT_LEN + PREFIX_LEN]);
    let keys = keys(passphrase, salt, cost)?;
    let checked = HEADER_LEN - CHECK_LEN;
    let expected = check(&keys[32..], &header[..checked])?;
    if !bool::from(subtle::ConstantTimeEq::ct_eq(&expected[..], &header[checked..])) {
        return Ok(None);
    }
    let cipher = Aes256Gcm::new_from_slice(&keys[..32])
        .map_err(|e| AppError::Internal(format!("cannot open the archive: {e}")))?;
    Ok(Some(Unsealer { file, header, decryptor: DecryptorBE32::from_aead(cipher, &prefix.into()) }))
}

/// Open `sealed` into `plain` with `passphrase`. `Ok(false)` when the
/// passphrase does not open it, which only the person restoring can mend.
pub(super) fn open(sealed: &Path, plain: &Path, passphrase: &Passphrase) -> AppResult<bool> {
    let Some(Unsealer { mut file, header, mut decryptor }) = unsealer(sealed, passphrase)? else {
        return Ok(false);
    };
    let damaged =
        || AppError::BadRequest("the archive is damaged: a part of it does not open".into());
    let written =
        |e: std::io::Error| AppError::Internal(format!("cannot write {}: {e}", plain.display()));
    let mut out = crate::crypto::create_private(plain, false).map_err(written)?;

    let mut current = vec![0u8; CHUNK + TAG_LEN];
    let mut next = vec![0u8; CHUNK + TAG_LEN];
    let mut length = fill(&mut file, &mut current).map_err(|_| damaged())?;
    loop {
        let following = fill(&mut file, &mut next).map_err(|_| damaged())?;
        let payload = Payload { msg: &current[..length], aad: &header };
        if following == 0 {
            let chunk = decryptor.decrypt_last(payload).map_err(|_| damaged())?;
            out.write_all(&chunk).map_err(written)?;
            break;
        }
        let chunk = decryptor.decrypt_next(payload).map_err(|_| damaged())?;
        out.write_all(&chunk).map_err(written)?;
        std::mem::swap(&mut current, &mut next);
        length = following;
    }
    out.sync_all().map_err(written)?;
    Ok(true)
}

/// Whether `passphrase` opens `sealed`. Reads the header alone.
pub(super) fn opens(sealed: &Path, passphrase: &Passphrase) -> AppResult<bool> {
    Ok(unsealer(sealed, passphrase)?.is_some())
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
    passphrases: &[&Passphrase],
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
    old: Option<Passphrase>,
    new: Option<Passphrase>,
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
    old: Option<&Passphrase>,
    new: Option<&Passphrase>,
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
    old: Option<Passphrase>,
    new: Option<Passphrase>,
    by: Attribution,
) {
    let same = match (&old, &new) {
        (Some(old), Some(new)) => old.as_str() == new.as_str(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn passphrase(words: &str) -> Passphrase {
        Passphrase::new(words.to_string())
    }

    /// `content` sealed and opened again, in a directory of its own.
    fn round_trip(label: &str, content: &[u8]) -> Vec<u8> {
        let dir = crate::tests::TempDir::new(label);
        let (plain, sealed, opened) = (dir.join("plain"), dir.join("sealed"), dir.join("opened"));
        std::fs::write(&plain, content).unwrap();
        seal(&plain, &sealed, &passphrase("the passphrase")).unwrap();
        assert!(is_sealed_file(&sealed));
        assert!(open(&sealed, &opened, &passphrase("the passphrase")).unwrap());
        std::fs::read(opened).unwrap()
    }

    /// Nothing, less than a chunk, exactly two, and two and a bit: the last
    /// chunk is flagged as last whatever its size, an empty one included.
    #[test]
    fn what_is_sealed_opens_whole_whatever_its_size() {
        for (label, size) in
            [("empty", 0), ("short", 10), ("even", 2 * CHUNK), ("odd", 2 * CHUNK + 7)]
        {
            let content: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            assert_eq!(round_trip(&format!("seal-{label}"), &content), content, "{label}");
        }
    }

    /// A wrong passphrase is told apart from damage, before a byte is opened.
    #[test]
    fn a_wrong_passphrase_opens_nothing_and_says_so() {
        let dir = crate::tests::TempDir::new("seal-wrong");
        let (plain, sealed, opened) = (dir.join("plain"), dir.join("sealed"), dir.join("opened"));
        std::fs::write(&plain, b"a database").unwrap();
        seal(&plain, &sealed, &passphrase("the passphrase")).unwrap();

        assert!(!open(&sealed, &opened, &passphrase("another one")).unwrap());
        assert!(!opened.exists(), "a wrong passphrase wrote something");
        assert!(opens(&sealed, &passphrase("the passphrase")).unwrap());
    }

    /// Each way a sealed file can be altered after it was written: a byte
    /// flipped, the last chunk cut off, a chunk added after the end, two
    /// chunks swapped. None opens.
    #[test]
    fn an_archive_altered_in_any_way_does_not_open() {
        let dir = crate::tests::TempDir::new("seal-altered");
        let (plain, sealed) = (dir.join("plain"), dir.join("sealed"));
        std::fs::write(&plain, vec![7u8; 3 * CHUNK]).unwrap();
        seal(&plain, &sealed, &passphrase("the passphrase")).unwrap();
        let whole = std::fs::read(&sealed).unwrap();
        let sealed_chunk = CHUNK + TAG_LEN;
        let chunk = |n: usize| HEADER_LEN + n * sealed_chunk..HEADER_LEN + (n + 1) * sealed_chunk;

        let mut flipped = whole.clone();
        flipped[HEADER_LEN + 100] ^= 1;
        let cut = whole[..chunk(2).start].to_vec();
        let mut longer = whole.clone();
        longer.extend_from_slice(&whole[chunk(0)]);
        let mut swapped = whole.clone();
        swapped[chunk(0)].copy_from_slice(&whole[chunk(1)]);
        swapped[chunk(1)].copy_from_slice(&whole[chunk(0)]);

        for (label, bytes) in
            [("flipped", flipped), ("cut", cut), ("longer", longer), ("swapped", swapped)]
        {
            let altered = dir.join(label);
            std::fs::write(&altered, bytes).unwrap();
            let opened =
                open(&altered, &dir.join(format!("{label}.opened")), &passphrase("the passphrase"));
            assert!(
                matches!(opened, Err(AppError::BadRequest(ref m)) if m.contains("damaged")),
                "{label}: {opened:?}"
            );
        }
    }

    /// A header written by hand asking for more than a restore spends, and a
    /// file that is no sealed archive, are refused before any work.
    #[test]
    fn a_header_asking_too_much_or_none_at_all_is_refused() {
        let dir = crate::tests::TempDir::new("seal-header");
        let (plain, sealed) = (dir.join("plain"), dir.join("sealed"));
        std::fs::write(&plain, b"a database").unwrap();
        seal(&plain, &sealed, &passphrase("the passphrase")).unwrap();
        let mut greedy = std::fs::read(&sealed).unwrap();
        greedy[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&sealed, greedy).unwrap();
        let refused = opens(&sealed, &passphrase("the passphrase"));
        assert!(
            matches!(refused, Err(AppError::BadRequest(ref m)) if m.contains("cost")),
            "{refused:?}"
        );

        assert!(!is_sealed_file(&plain));
        let refused = opens(&plain, &passphrase("the passphrase"));
        assert!(matches!(refused, Err(AppError::BadRequest(_))), "{refused:?}");
    }
}
