//! Scheduled, restorable backups.
//!
//! The database alone is useless: the Arr credentials in it are sealed with a
//! key that lives in a separate file, which a "copy the database" backup leaves
//! behind.
//!
//! Three properties are load-bearing:
//!
//! * **Consistent without stopping the server.** `VACUUM INTO` snapshots a live
//!   database at one point in time, WAL included.
//! * **Self-sufficient.** Database, master key and API key travel together,
//!   which makes the archive as sensitive as the master key itself.
//! * **A restore never half-applies.** The bundle is validated and *staged*,
//!   and the swap happens at the next startup, before the pool opens. Replacing a
//!   database under an open pool is how a restore destroys what it recovers.
//!   Nothing is marked pending until every file is written and the database
//!   has been opened and checked, and the start checks it again.

mod sealed;

use sqlx::AssertSqlSafe;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

pub use sealed::{
    MIN_LENGTH as MIN_PASSPHRASE_LENGTH, Passphrase, SETTING as PASSPHRASE_SETTING, is_sealed_file,
    passphrase_of, resume as resume_sealing, set as set_passphrase, stored_passphrase,
};

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::state::AppState;

/// Written into every archive so a restore can refuse what it cannot honour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// The Routarr version that produced it, for diagnosis.
    pub version: String,
    /// The last migration applied when it was taken. A bundle from a *newer*
    /// schema is refused rather than restored into an older binary, which would
    /// leave the database ahead of the code that has to read it.
    pub schema: String,
    pub created_at: String,
    /// Whether the master key travelled with it.
    pub includes_master_key: bool,
}

/// One archive on disk.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct BackupFile {
    pub name: String,
    pub size_bytes: u64,
    pub created_at: String,
    /// Sealed with the backup passphrase: a restore asks for it, and
    /// `routarr decrypt-backup` opens it by hand.
    pub encrypted: bool,
}

/// Entry names inside the archive. Fixed, so a restore knows what to look for.
const DB_ENTRY: &str = "routarr.db";
const MASTER_KEY_ENTRY: &str = "routarr.key";
const API_KEY_ENTRY: &str = "routarr.api_key";
const MANIFEST_ENTRY: &str = "manifest.json";

/// Marker telling the next startup that a restore is pending.
const PENDING_SUFFIX: &str = ".restore-pending";
/// What a restore sets the replaced file aside as, one generation.
const PRE_RESTORE: &str = ".pre-restore";

/// A restore being written. Never applied: only a complete set whose database
/// has been checked is renamed to the pending suffix.
const STAGING_SUFFIX: &str = ".restore-staging";

/// Files that exist only until the work writing them succeeds.
///
/// Removed on drop unless kept, so every early return takes them away. Left
/// behind, a half-written archive under a backup name is listed, counted by the
/// retention and restorable, and a half-written restore is applied at the next
/// start.
struct Scaffold(Vec<PathBuf>);

impl Scaffold {
    fn keep(mut self) {
        self.0.clear();
    }

    fn holds(&self, path: &Path) -> bool {
        self.0.iter().any(|held| held == path)
    }
}

impl Drop for Scaffold {
    fn drop(&mut self) {
        for path in &self.0 {
            std::fs::remove_file(path).ok();
        }
    }
}

/// Where archives live: a `backups` directory beside the database.
pub fn backup_dir(state: &AppState) -> PathBuf {
    backups_in(&state.config.data_dir)
}

fn backups_in(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// A name Routarr generated, and nothing else.
///
/// The download and delete endpoints take this from the URL, so it is the one
/// place a caller could try to walk out of the directory. Rejecting anything
/// but the exact generated shape is simpler to defend than sanitising: no
/// separators, no `..`, no absolute paths, because none of them can match.
pub fn is_valid_backup_name(name: &str) -> bool {
    name.len() <= 64
        && stamp_of(name).is_some()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..")
}

/// What a name holds between `routarr-backup-` and `.zip`, sealed or not.
fn stamp_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("routarr-backup-")?;
    rest.strip_suffix(sealed::SUFFIX).unwrap_or(rest).strip_suffix(".zip")
}

/// What an archive an application key took is named with. Its archives are
/// kept under a count of their own, so taking backups in a loop removes only
/// its own, never the owner's or the schedule's.
const APPLICATION_SUFFIX: &str = "-app";

/// How long after the newest archive an application key may take another.
const APPLICATION_GAP: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Take a backup now.
pub async fn create(state: &AppState, by: &Attribution) -> AppResult<BackupFile> {
    let by_application = by.key.is_some();
    if by_application
        && let Some(left) = list(state)
            .first()
            .and_then(taken_at)
            .map(|taken| (chrono::Utc::now() - taken).to_std().unwrap_or_default())
            .and_then(|age| APPLICATION_GAP.checked_sub(age))
            .filter(|left| !left.is_zero())
    {
        let seconds = left.as_secs() + 1;
        let message = state
            .localizer()
            .await
            .translate("ErrorBackupTooSoon", &[("seconds", &seconds.to_string())]);
        return Err(AppError::TooManyRequests { message, retry_after: seconds });
    }
    let Some(_lock) = state.jobs.try_lock("backup") else {
        return Err(AppError::Conflict("A backup is already running".into()));
    };

    let job =
        state.jobs.start(JobKind::Backup, by, None, Detail::new("JobDetailBackingUp")).await?;
    let suffix = if by_application { APPLICATION_SUFFIX } else { "" };
    let outcome = match sealed::passphrase(state).await {
        Ok(passphrase) => {
            write_archive(&state.config, &state.pool, suffix, passphrase, Some(&job.id)).await
        }
        Err(e) => Err(e),
    };

    match &outcome {
        Ok(file) => job.succeed(Detail::new("JobDetailBackedUp").with("file", &file.name)).await,
        Err(e) => job.fail(e).await,
    }

    if outcome.is_ok() {
        // Pruning is not part of the backup's success: a full disk must not
        // make the archive that was just written look like a failure.
        if let Err(e) = prune(state).await {
            warn!("Could not prune old backups: {e}");
        }
    }

    outcome
}

/// Write the backup's own job into its snapshot as the success it is once the
/// archive exists. Taken while the job runs, the row would come back running
/// with every restore, and the first start after it would mark it interrupted:
/// a failed backup shown for the very archive just restored.
async fn settle_in_snapshot(snapshot: &Path, job: &str, name: &str) -> AppResult<()> {
    use sqlx::{ConnectOptions, Connection};
    let detail = crate::jobs::Detail::new("JobDetailBackedUp").with("file", name);
    // A rollback journal: the snapshot is zipped alone, and a log beside it
    // would be left out with this write in it.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(snapshot)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .connect()
        .await?;
    let settled = sqlx::query(
        "UPDATE jobs SET status = 'success', detail = ?, detail_key = ?, detail_params = ?,
                finished_at = datetime('now')
          WHERE id = ?",
    )
    .bind(detail.english())
    .bind("JobDetailBackedUp")
    .bind(detail.stored_params())
    .bind(job)
    .execute(&mut connection)
    .await;
    connection.close().await.ok();
    settled?;
    Ok(())
}

/// Take a backup of a database the server has not opened yet, before a start
/// migrates it: the archives kept afterwards would all be of the new schema,
/// which the release before cannot open.
pub async fn before_migrating(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
) -> AppResult<BackupFile> {
    let passphrase = sealed::stored_passphrase(config, pool).await?;
    write_archive(config, pool, "", passphrase, None).await
}

/// Write an archive, sealed for `passphrase` when there is one, for the job
/// `job` when one runs it.
async fn write_archive(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
    suffix: &str,
    passphrase: Option<Passphrase>,
    job: Option<&str>,
) -> AppResult<BackupFile> {
    let dir = backups_in(&config.data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", dir.display())))?;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let plain_name = format!("routarr-backup-{stamp}{suffix}.zip");
    let name = match passphrase {
        Some(_) => format!("{plain_name}{}", sealed::SUFFIX),
        None => plain_name.clone(),
    };
    let path = dir.join(&name);
    // Named to the second: a name already taken is refused before a copy of
    // the whole database is written for nothing. Checked again at the end,
    // since the name can be taken meanwhile.
    if path.exists() || dir.join(&plain_name).exists() {
        return Err(AppError::Conflict(
            "A backup was taken this second. Try again in a moment.".into(),
        ));
    }

    // A consistent snapshot of the live database, WAL included, in the work
    // directory, which the container can write and nobody else reads.
    let snapshot = work_file(&config.data_dir, "snapshot.db")?;
    // A copy of the whole database, sealed credentials included, and twice
    // the space a backup costs: gone on every way out of this function. It
    // moves into the archive task below, which runs to its end whatever
    // happens to the future awaiting it, so a cancelled caller does not
    // remove it under the task that reads it.
    let snapshot_scaffold = Scaffold(vec![snapshot.clone()]);
    vacuum_into(pool, &snapshot).await?;
    if let Some(job) = job {
        settle_in_snapshot(&snapshot, job, &name).await?;
    }

    let schema: String =
        sqlx::query_scalar("SELECT name FROM _migrations ORDER BY id DESC LIMIT 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| "unknown".to_string());

    // With the key in `ROUTARR_SECRET_KEY`, a `routarr.key` beside the database
    // is a stale file, not what anything is sealed with: carried and announced,
    // it would make a restore elsewhere open a database nothing can read,
    // without the warning a missing key gets.
    let master_key =
        Some(config.secret_key_path()).filter(|path| config.secret_key.is_none() && path.exists());
    let manifest = BackupManifest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema,
        created_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
        includes_master_key: master_key.is_some(),
    };

    // Off the runtime, and the whole of it: deflating a library-sized database
    // holds a worker for as long as it takes. Everything the closure needs is
    // taken by value first, so nothing is borrowed across the boundary.
    let (archive, snapshot_path) = (path.clone(), snapshot.clone());
    let keys = [master_key, Some(config.api_key_path())];
    let manifest_for_zip = manifest.clone();
    let zipped = match passphrase {
        Some(_) => Some(work_file(&config.data_dir, "archive.zip")?),
        None => None,
    };
    let result = tokio::task::spawn_blocking(move || {
        let _snapshot = snapshot_scaffold;
        let sealing = passphrase.as_ref().zip(zipped.as_deref());
        build_zip(&archive, &snapshot_path, &manifest_for_zip, &keys, sealing)
    })
    .await
    .map_err(|e| AppError::Internal(format!("the archive task failed: {e}")))?;
    result?;

    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    crate::crypto::restrict_permissions(&path);

    info!("Backup written to {} ({size_bytes} bytes)", path.display());
    let encrypted = sealed::is_sealed_file(&path);
    Ok(BackupFile { name, size_bytes, created_at: manifest.created_at, encrypted })
}

/// `VACUUM INTO` against the live pool.
async fn vacuum_into(pool: &sqlx::SqlitePool, target: &Path) -> AppResult<()> {
    // Left over from an interrupted run, it would make VACUUM INTO fail: the
    // statement refuses to overwrite an existing file.
    std::fs::remove_file(target).ok();

    // `VACUUM INTO` takes no bind parameter for its destination, so the path
    // goes into the statement text. Two things keep that safe and both must
    // stay true: `target` is composed by `write_archive` from the configured
    // data directory and a timestamp, reachable by no request, and the single
    // quote is doubled. A caller passing a name from elsewhere would break the
    // first without breaking the build.
    let quoted = target.to_string_lossy().replace('\'', "''");
    sqlx::query(AssertSqlSafe(format!("VACUUM INTO '{quoted}'"))).execute(pool).await?;

    // `VACUUM INTO` reports success against an in-memory database and writes
    // nothing. One stat call turns that into a message that says so.
    if !target.exists() {
        return Err(AppError::Internal(format!(
            "the database snapshot was not written to {}",
            target.display()
        )));
    }
    // `VACUUM INTO` creates the file under the umask, and it is the whole
    // database, sealed credentials included, until the zip is written and
    // it is removed.
    crate::crypto::restrict_permissions(target);

    Ok(())
}

/// Write the archive, streaming the database rather than holding it.
///
/// Synchronous on purpose: the caller runs it on a blocking thread. Nothing
/// here may read a file whose size follows the library into memory:
/// `api/backup.rs` streams the download for exactly that reason, and an
/// archive is the whole database.
///
/// `sealing` is the passphrase to seal it for, and where the zip is written in
/// the clear before: a zip goes back to write its directory, which a sealed
/// stream cannot. That file is removed whatever happens.
fn build_zip(
    path: &Path,
    snapshot: &Path,
    manifest: &BackupManifest,
    keys: &[Option<PathBuf>; 2],
    sealing: Option<(&Passphrase, &Path)>,
) -> AppResult<()> {
    // Written under a name `list` does not show, and given its own only once
    // whole: a zip is readable as soon as it is finished, and `ZipWriter`
    // finishes it on drop, early return included.
    let partial = partial_path(path);
    std::fs::remove_file(&partial).ok();
    let mut scaffold = Scaffold(vec![partial.clone()]);
    let zipped = match sealing {
        Some((_, plain)) => {
            scaffold.0.push(plain.to_path_buf());
            plain.to_path_buf()
        }
        None => partial.clone(),
    };

    // The zip carries the master key and the API key in clear: private from
    // its first byte.
    let file = crate::crypto::create_private(&zipped, true)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", zipped.display())))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // A function rather than a closure capturing `zip`: the database entry
    // below writes into the same writer, and a closure holding it would own
    // the only mutable borrow.
    fn add(
        zip: &mut zip::ZipWriter<std::fs::File>,
        options: zip::write::FileOptions<'_, ()>,
        name: &str,
        bytes: &[u8],
    ) -> AppResult<()> {
        zip.start_file(name, options)
            .map_err(|e| AppError::Internal(format!("cannot add {name} to the archive: {e}")))?;
        zip.write_all(bytes)
            .map_err(|e| AppError::Internal(format!("cannot write {name}: {e}")))?;
        Ok(())
    }

    add(&mut zip, options, MANIFEST_ENTRY, serde_json::to_string_pretty(manifest)?.as_bytes())?;

    // Copied through, never materialised: read whole, the database would sit
    // in memory twice, once as bytes and once in the deflater, and it grows
    // with somebody's library.
    zip.start_file(DB_ENTRY, options)
        .map_err(|e| AppError::Internal(format!("cannot add {DB_ENTRY} to the archive: {e}")))?;
    let mut source = std::fs::File::open(snapshot)
        .map_err(|e| AppError::Internal(format!("cannot read the snapshot: {e}")))?;
    std::io::copy(&mut source, &mut zip)
        .map_err(|e| AppError::Internal(format!("cannot write {DB_ENTRY}: {e}")))?;

    // Without the master key the restored database still opens, but every Arr
    // credential in it is undecryptable, and a restore that silently loses
    // them is worse than one that refuses.
    // These two are a handful of bytes each, so reading them whole is not the
    // same question as the database above.
    for (entry, source) in [MASTER_KEY_ENTRY, API_KEY_ENTRY].iter().zip(keys) {
        if let Some(Ok(bytes)) = source.as_ref().map(std::fs::read) {
            add(&mut zip, options, entry, &bytes)?;
        }
    }

    let file =
        zip.finish().map_err(|e| AppError::Internal(format!("cannot finish the archive: {e}")))?;
    // On disk before it takes a backup name: renamed first, a power cut can
    // leave an empty file under that name, listed and counted by the retention.
    file.sync_all()
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", zipped.display())))?;
    if let Some((passphrase, _)) = sealing {
        sealed::seal(&zipped, &partial, passphrase)?;
        std::fs::remove_file(&zipped).ok();
    }

    // Never over an archive that already exists, which `rename` would replace.
    if path.exists() {
        return Err(AppError::Internal(format!("{} already exists", path.display())));
    }
    std::fs::rename(&partial, path)
        .map_err(|e| AppError::Internal(format!("cannot name {}: {e}", path.display())))?;
    scaffold.keep();
    Ok(())
}

fn partial_path(archive: &Path) -> PathBuf {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    archive.with_file_name(format!(".{name}.partial"))
}

/// Where a backup, a restore and a conversion write what they hold in the
/// clear: the snapshot, an archive zipped before it is sealed, a sealed one
/// opened. Beside the database, which holds the same in the clear, and never
/// in the backup folder, the one a passphrase protects when it is copied
/// away. Private to the server's user, so a file another writes under the
/// umask inside it, as `VACUUM INTO` does, is read by nobody else.
fn work_dir(data_dir: &Path) -> AppResult<PathBuf> {
    let dir = data_dir.join(".backup-work");
    let refused =
        |e: std::io::Error| AppError::Internal(format!("cannot prepare {}: {e}", dir.display()));
    std::fs::create_dir_all(&dir).map_err(refused)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(refused)?;
    }
    Ok(dir)
}

/// A file of the work directory no other run writes: two openings of one
/// archive at once each get their own.
fn work_file(data_dir: &Path, label: &str) -> AppResult<PathBuf> {
    let mut tag = [0u8; 8];
    crate::crypto::random_bytes(&mut tag)?;
    let tag: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(work_dir(data_dir)?.join(format!("{tag}-{label}")))
}

/// When the newest archive the owner or the schedule took was taken, as an
/// instant of this process: what the schedule counts its interval from. An
/// application key's archives are left out, or one taken every few minutes
/// would hold the schedule off for good.
///
/// Read from the name, which `write_archive` stamps, rather than from the
/// file's modification time, which a copy or a restore of the folder resets.
pub fn newest_taken(state: &AppState) -> Option<tokio::time::Instant> {
    let taken = last_taken_at(state)?;
    let age = (chrono::Utc::now() - taken).to_std().unwrap_or_default();
    tokio::time::Instant::now().checked_sub(age)
}

/// When the newest archive the schedule or the owner took was written. An
/// application's archives do not stand for one: a script taking them in a loop
/// would hide a schedule that stopped.
pub fn last_taken_at(state: &AppState) -> Option<chrono::DateTime<chrono::Utc>> {
    list(state).iter().find(|file| !taken_by_a_key(file)).and_then(taken_at)
}

fn taken_by_a_key(file: &BackupFile) -> bool {
    stamp_of(&file.name).is_some_and(|stamp| stamp.ends_with(APPLICATION_SUFFIX))
}

fn taken_at(file: &BackupFile) -> Option<chrono::DateTime<chrono::Utc>> {
    stamped_at(&file.name)
}

/// When the archive `name` was taken, by the UTC stamp in its name, which a
/// copy of the folder keeps and its file dates do not.
fn stamped_at(name: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let stamp = stamp_of(name)?;
    let stamp = stamp.strip_suffix(APPLICATION_SUFFIX).unwrap_or(stamp);
    Some(chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").ok()?.and_utc())
}

/// Every archive on disk, newest first.
pub fn list(state: &AppState) -> Vec<BackupFile> {
    list_in(&backup_dir(state))
}

fn list_in(dir: &Path) -> Vec<BackupFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<BackupFile> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(is_valid_backup_name))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // The file's date only for a name that carries no stamp.
            let created = stamped_at(&name)
                .or_else(|| metadata.modified().ok().map(chrono::DateTime::<chrono::Utc>::from))
                .map(crate::services::routing::format_timestamp)
                .unwrap_or_default();
            Some(BackupFile {
                encrypted: sealed::is_sealed_file(&entry.path()),
                name,
                size_bytes: metadata.len(),
                created_at: created,
            })
        })
        .collect();

    // The name carries a sortable timestamp, so this orders by age without
    // trusting the filesystem's clock.
    files.sort_by(|a, b| b.name.cmp(&a.name));
    files
}

/// Delete everything past the retention count, oldest first: the archives
/// application keys took counted apart from the others.
pub async fn prune(state: &AppState) -> AppResult<usize> {
    // Read as stored, a value above the ceiling included: lowering a retention
    // count removes archives, so only the operator's own save does (see
    // `settings::Kind::Retention`).
    let keep: usize = state.bounding_setting("backup_retention_count", 7usize).await?.max(1);
    let (taken_by_keys, others): (Vec<BackupFile>, Vec<BackupFile>) =
        list(state).into_iter().partition(taken_by_a_key);

    let dir = backup_dir(state);
    let mut removed = 0;
    for file in [taken_by_keys, others].into_iter().flat_map(|files| files.into_iter().skip(keep)) {
        if std::fs::remove_file(dir.join(&file.name)).is_ok() {
            removed += 1;
        }
    }

    if removed > 0 {
        info!("Removed {removed} backup(s) past the retention count of {keep}");
    }
    Ok(removed)
}

pub fn delete(state: &AppState, name: &str) -> AppResult<()> {
    if !is_valid_backup_name(name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let path = backup_dir(state).join(name);
    std::fs::remove_file(&path).map_err(|_| AppError::NotFound("Unknown backup".into()))
}

/// Read an archive's manifest without extracting anything.
pub fn read_manifest(archive: &Path) -> AppResult<BackupManifest> {
    let file =
        std::fs::File::open(archive).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::BadRequest(format!("not a readable archive: {e}")))?;

    let mut entry = zip
        .by_name(MANIFEST_ENTRY)
        .map_err(|_| AppError::BadRequest("the archive carries no manifest".into()))?;

    let mut raw = String::new();
    std::io::Read::read_to_string(&mut entry, &mut raw)
        .map_err(|e| AppError::BadRequest(format!("unreadable manifest: {e}")))?;

    serde_json::from_str(&raw).map_err(|e| AppError::BadRequest(format!("invalid manifest: {e}")))
}

/// Validate an archive and stage it for the next startup.
///
/// Nothing is swapped here on purpose. Replacing the database file under an
/// open connection pool is how a restore destroys what it was recovering, so
/// the files are written beside their targets and `main` moves them into place
/// before it opens anything. Each is written under a staging name and renamed
/// to its pending name only once every file is complete and the database has
/// been opened and checked, the database last: its pending file is what a
/// start takes for a restore.
///
/// One staging at a time, run to its end on a task of its own. The stagings
/// write the same files, and a request dropped halfway, by a proxy giving up
/// on a large database, would release its lock while its copy goes on: a
/// retry beside it would then see its checked files removed or overwritten.
///
/// A sealed archive opens with `given`, or else with the passphrase the
/// settings hold, which an archive sealed before the passphrase changed does
/// not open.
pub async fn stage_restore(
    state: &AppState,
    name: &str,
    given: Option<Passphrase>,
) -> AppResult<BackupManifest> {
    if !is_valid_backup_name(name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let Some(lock) = state.jobs.try_lock("restore") else {
        return Err(AppError::Conflict("A restore is already being staged".into()));
    };

    let state = state.clone();
    let name = name.to_string();
    tokio::spawn(async move {
        let _lock = lock;
        let archive = backup_dir(&state).join(&name);
        let stored = sealed::passphrase(&state).await.ok().flatten();
        let localizer = state.localizer().await;
        let opened =
            opened_if_sealed(&state.config.data_dir, &archive, given, stored, &localizer).await?;
        let source = opened.as_ref().map_or(archive.as_path(), |opened| opened.path.as_path());
        stage(&state.config, Some(&state.pool), source, &name).await
    })
    .await
    .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))?
}

/// `archive` opened beside itself when it is sealed, with `given` or else
/// `stored`. Refused with `passphrase_required` when neither opens it, which
/// says whether a passphrase was given.
///
/// An archive named as sealed whose content is not is refused: in a folder
/// where the archives are sealed, a zip in the clear under a sealed name is
/// one somebody put there, and restoring it would bring back whatever it holds.
async fn opened_if_sealed(
    data_dir: &Path,
    archive: &Path,
    given: Option<Passphrase>,
    stored: Option<Passphrase>,
    localizer: &crate::localization::Localizer,
) -> AppResult<Option<sealed::Opened>> {
    let named_sealed =
        archive.file_name().is_some_and(|name| sealed::is_sealed_name(&name.to_string_lossy()));
    if !sealed::is_sealed_file(archive) {
        if named_sealed {
            return Err(AppError::BadRequest(
                "this archive is named as encrypted and is not: it is not restored".into(),
            ));
        }
        return Ok(None);
    }
    let refusal = if given.is_some() { "ErrorPassphraseWrong" } else { "ErrorPassphraseNeeded" };
    let tried: Vec<Passphrase> = given.into_iter().chain(stored).collect();
    let (data_dir, path) = (data_dir.to_path_buf(), archive.to_path_buf());
    let opened = tokio::task::spawn_blocking(move || {
        sealed::opened_copy(&data_dir, &path, &tried.iter().collect::<Vec<_>>())
    })
    .await
    .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))??;
    match opened {
        Some(opened) => Ok(Some(opened)),
        None => Err(AppError::PassphraseRequired(localizer.translate(refusal, &[]))),
    }
}

/// A credential the owner can withdraw outright, which a restore must not
/// bring back while it stays withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credential {
    MasterApiKey,
    SigningSecret,
}

impl Credential {
    /// Its row in `withdrawn_credentials`.
    fn name(self) -> &'static str {
        match self {
            Self::MasterApiKey => "master_api_key",
            Self::SigningSecret => "signing_secret",
        }
    }
}

/// Record `credential` as withdrawn, or as replaced by a new one.
pub async fn set_withdrawn<'e>(
    executor: impl sqlx::SqliteExecutor<'e>,
    credential: Credential,
    withdrawn: bool,
) -> AppResult<()> {
    let statement = if withdrawn {
        "INSERT INTO withdrawn_credentials (name) VALUES (?) ON CONFLICT(name) DO NOTHING"
    } else {
        "DELETE FROM withdrawn_credentials WHERE name = ?"
    };
    sqlx::query(statement).bind(credential.name()).execute(executor).await?;
    Ok(())
}

async fn is_withdrawn(live: &sqlx::SqlitePool, credential: Credential) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM withdrawn_credentials WHERE name = ?)")
        .bind(credential.name())
        .fetch_one(live)
        .await?)
}

/// Stage `archive`, carrying into it what `live`, the database it replaces,
/// has withdrawn since.
async fn stage(
    config: &crate::config::Config,
    live: Option<&sqlx::SqlitePool>,
    archive: &Path,
    name: &str,
) -> AppResult<BackupManifest> {
    let archive = archive.to_path_buf();
    let manifest = read_manifest(&archive)?;

    // A bundle from a newer schema would leave the database ahead of the binary
    // that has to read it. Refusing is the only honest answer: migrations only
    // ever go forward.
    if crate::db::is_newer_schema(&manifest.schema) {
        return Err(AppError::BadRequest(format!(
            "this backup was taken at schema '{}', which this version of Routarr does not know. Upgrade Routarr first.",
            manifest.schema
        )));
    }

    // Off the runtime for the same reason the archive is written there: the
    // database entry is copied out whole, and its size follows the library.
    let targets = [
        (DB_ENTRY, config.db_path.clone()),
        (MASTER_KEY_ENTRY, config.secret_key_path()),
        (API_KEY_ENTRY, config.api_key_path()),
    ];
    let paths = targets.clone().map(|(_, target)| target);

    let named = name.to_string();
    let listed_key = manifest.includes_master_key;
    let staged =
        tokio::task::spawn_blocking(move || extract_staged(&archive, &targets, &named, listed_key))
            .await
            .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))??;

    // A zero-byte or truncated file opens as a database, and would be moved
    // over the live one.
    let staged_database = staging_path(&config.db_path);
    if let Err(reason) = check_database(&staged_database).await {
        return Err(AppError::BadRequest(format!(
            "the archive's database cannot be restored: {reason}"
        )));
    }
    let key_withdrawn = match live {
        Some(live) => is_withdrawn(live, Credential::MasterApiKey).await?,
        None => false,
    };
    if let Some(live) = live {
        keep_withdrawn_credentials(live, &staged_database).await?;
    }
    opened_by_the_next_start(config, &staged_database).await?;

    // A new restore replaces an earlier one entirely, and only once it is
    // known to be one: a file the new archive does not carry would otherwise
    // be applied beside it, from the old one, and a refused archive would take
    // the earlier restore with it.
    discard_pending(&paths);

    // The database last: its pending file is what a start takes for the
    // restore, so a set interrupted before it is rejected whole.
    for target in paths.iter().rev() {
        let staging = staging_path(target);
        if !staged.holds(&staging) {
            continue;
        }
        // The API key in use stays: whoever restores holds it, and one rotated
        // because it leaked must not come back with the archive. Nor does one
        // withdrawn since, or one `ROUTARR_API_KEY` stands in for: unused
        // today, it would answer again once the variable is lifted.
        let kept = target.exists() || key_withdrawn || config.api_key.is_some();
        if *target == config.api_key_path() && kept {
            std::fs::remove_file(&staging).ok();
            continue;
        }
        if let Err(e) = std::fs::rename(&staging, pending_path(target)) {
            discard_pending(&paths);
            return Err(AppError::Internal(format!("cannot stage {}: {e}", target.display())));
        }
        crate::crypto::sync_parent(target);
    }
    staged.keep();

    info!("Backup {name} staged. It is applied on the next start");
    Ok(manifest)
}

/// Stage an archive with the server stopped, as `routarr restore` does: the
/// way back when a start refuses the database, since restoring otherwise needs
/// the server running. `archive` is a name from the backup folder, or a path.
///
/// A sealed archive opens with `given`, or else with the passphrase the
/// database in place holds.
pub async fn stage_offline(
    config: &crate::config::Config,
    archive: &str,
    given: Option<Passphrase>,
) -> AppResult<BackupManifest> {
    let path = archive_named(config, archive)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    // What the database in place has withdrawn since is carried over, as an
    // online restore does.
    let live = database_in_place(config).await?;
    let staged = async {
        let stored = match &live {
            Some(live) => sealed::stored_passphrase(config, live).await.ok().flatten(),
            None => None,
        };
        let localizer = crate::localization::Localizer::new("en");
        let opened = opened_if_sealed(&config.data_dir, &path, given, stored, &localizer).await?;
        let source = opened.as_ref().map_or(path.as_path(), |opened| opened.path.as_path());
        stage(config, live.as_ref(), source, &name).await
    }
    .await;
    if let Some(live) = live {
        live.close().await;
    }
    staged
}

/// `archive` as a command names it: a name from the backup folder, or a path.
fn archive_named(config: &crate::config::Config, archive: &str) -> AppResult<PathBuf> {
    let in_folder = backups_in(&config.data_dir).join(archive);
    let path = if is_valid_backup_name(archive) && in_folder.exists() {
        in_folder
    } else {
        archive.into()
    };
    if !path.is_file() {
        return Err(AppError::NotFound(format!("{} is not an archive", path.display())));
    }
    Ok(path)
}

/// The database a command run with the server stopped reads, when there is
/// one. Opened as it is, never migrated: it may be the very database a start
/// refuses.
async fn database_in_place(config: &crate::config::Config) -> AppResult<Option<sqlx::SqlitePool>> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    if !config.db_path.is_file() {
        return Ok(None);
    }
    let options = crate::db::with_paths(SqliteConnectOptions::new().filename(&config.db_path));
    Ok(Some(SqlitePoolOptions::new().max_connections(1).connect_with(options).await?))
}

/// Write the sealed `archive` opened, as a zip at `out`: `routarr
/// decrypt-backup`, to read an archive by hand. Opens with `given`, or else
/// the passphrase the database in place holds. The zip carries the master key
/// in the clear, so it is written private, never over a file that exists, and
/// never in the backup folder, where the server would list it, serve it and
/// a sync would copy it away.
pub async fn decrypt_offline(
    config: &crate::config::Config,
    archive: &str,
    out: &str,
    given: Option<Passphrase>,
) -> AppResult<PathBuf> {
    let path = archive_named(config, archive)?;
    if !sealed::is_sealed_file(&path) {
        return Err(AppError::BadRequest(format!(
            "{} is not encrypted: it opens as a zip as it is",
            path.display()
        )));
    }
    let out = PathBuf::from(out);
    if out.exists() {
        return Err(AppError::Conflict(format!("{} exists already", out.display())));
    }
    let folder = |path: &Path| path.parent().and_then(|parent| std::fs::canonicalize(parent).ok());
    if folder(&out).is_some_and(|parent| {
        Some(parent) == std::fs::canonicalize(backups_in(&config.data_dir)).ok()
    }) {
        return Err(AppError::BadRequest(
            "an archive opened is not written into the backup folder, which a sync copies away"
                .into(),
        ));
    }
    let stored = match database_in_place(config).await? {
        Some(live) => {
            let stored = sealed::stored_passphrase(config, &live).await.ok().flatten();
            live.close().await;
            stored
        }
        None => None,
    };
    let localizer = crate::localization::Localizer::new("en");
    let opened = opened_if_sealed(&config.data_dir, &path, given, stored, &localizer).await?;
    let Some(opened) = opened else {
        return Err(AppError::BadRequest(format!("{} is not encrypted", path.display())));
    };
    std::fs::rename(&opened.path, &out)
        .or_else(|_| std::fs::copy(&opened.path, &out).map(|_| ()))
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", out.display())))?;
    crate::crypto::restrict_permissions(&out);
    Ok(out)
}

/// The newest archive in the backup folder this build can restore, by name.
/// A sealed one counts when `passphrase` opens it.
pub fn newest_openable(
    config: &crate::config::Config,
    passphrase: Option<&Passphrase>,
) -> Option<String> {
    let dir = backups_in(&config.data_dir);
    list_in(&dir).into_iter().map(|file| file.name).find(|name| {
        let archive = dir.join(name);
        let manifest = match (sealed::is_sealed_file(&archive), passphrase) {
            (false, _) => read_manifest(&archive).ok(),
            (true, None) => None,
            (true, Some(passphrase)) => sealed::manifest(&archive, passphrase).ok().flatten(),
        };
        manifest.is_some_and(|m| !crate::db::is_newer_schema(&m.schema))
    })
}

/// Copy the three known entries beside their targets, under the staging suffix.
///
/// Synchronous on purpose: the caller runs it on a blocking thread. The entry
/// names are literals and the destinations are computed here, so no name out of
/// the archive ever reaches a path. The files come back as a scaffold, removed
/// unless the caller keeps them.
fn extract_staged(
    archive: &Path,
    targets: &[(&str, PathBuf); 3],
    name: &str,
    listed_key: bool,
) -> AppResult<Scaffold> {
    let file =
        std::fs::File::open(archive).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::BadRequest(format!("not a readable archive: {e}")))?;

    let mut scaffold = Scaffold(Vec::new());
    for (entry, target) in targets {
        let mut source = match zip.by_name(entry) {
            Ok(source) => source,
            Err(zip::result::ZipError::FileNotFound) => {
                // The keys alone, restored under a database they were not
                // sealed for, are not a restore.
                if *entry == DB_ENTRY {
                    return Err(AppError::BadRequest("the archive carries no database".into()));
                }
                // The manifest records whether the key was there when the
                // archive was written, so a key it lists and the archive lacks
                // has been lost, and the restore would leave every sealed
                // credential unreadable.
                if *entry == MASTER_KEY_ENTRY && listed_key {
                    return Err(AppError::BadRequest(
                        "the archive lists its master key and does not carry it".into(),
                    ));
                }
                // Said out loud. An archive without the master key restores a
                // database whose every sealed credential this installation
                // cannot open, and the only other trace is `reseal_secrets`
                // reporting them one by one at the next start. The manifest
                // carries the same answer to the caller, which is what the
                // interface warns on.
                warn!(
                    "Backup {name} carries no '{entry}', so the restore leaves the current one in place"
                );
                continue;
            }
            // Present and unreadable is a damaged archive, not a missing entry:
            // taken for an absent key, the restore would go ahead without it.
            Err(e) => {
                return Err(AppError::BadRequest(format!(
                    "the archive is damaged: {entry} cannot be read ({e})"
                )));
            }
        };
        let staging = staging_path(target);
        scaffold.0.push(staging.clone());
        let mut out = crate::crypto::create_private(&staging, false)
            .map_err(|e| AppError::Internal(format!("cannot stage {}: {e}", staging.display())))?;
        std::io::copy(&mut source, &mut out).map_err(|e| copy_error(entry, e))?;
        // On disk before a rename marks it pending: after a power cut, a
        // pending file must hold what was checked, not what was cached.
        out.sync_all().map_err(|e| AppError::Internal(format!("cannot stage {entry}: {e}")))?;
    }

    Ok(scaffold)
}

/// A read that fails on the archive's side is a damaged archive, which the
/// operator chose and can replace, so it is said as such rather than as an
/// internal error. The CRC of an entry is checked only at its end, so this is
/// found after part of it was copied.
fn copy_error(entry: &str, e: std::io::Error) -> AppError {
    use std::io::ErrorKind::{InvalidData, InvalidInput, UnexpectedEof};
    match e.kind() {
        InvalidData | InvalidInput | UnexpectedEof => {
            AppError::BadRequest(format!("the archive is damaged: {entry} cannot be read ({e})"))
        }
        _ => AppError::Internal(format!("cannot stage {entry}: {e}")),
    }
}

fn with_suffix(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn staging_path(target: &Path) -> PathBuf {
    with_suffix(target, STAGING_SUFFIX)
}

fn pending_path(target: &Path) -> PathBuf {
    with_suffix(target, PENDING_SUFFIX)
}

/// Remove every pending file of a restore.
fn discard_pending(targets: &[PathBuf]) {
    for target in targets {
        std::fs::remove_file(pending_path(target)).ok();
    }
}

/// Remove what an interrupted backup or staging leaves behind.
///
/// Called from `main` before anything runs, the one moment none of these files
/// can belong to work in progress. A snapshot is a copy of the whole database
/// and a partial archive holds the master key in clear, and no later pass
/// takes either away. A staged file is never applied.
pub fn sweep_leftovers(config: &crate::config::Config) {
    for target in [&config.db_path, &config.secret_key_path(), &config.api_key_path()] {
        std::fs::remove_file(staging_path(target)).ok();
    }
    // Everything in the work directory is a run's, and none is running yet.
    if let Ok(entries) = std::fs::read_dir(config.data_dir.join(".backup-work")) {
        for entry in entries.filter_map(Result::ok) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
    let Ok(entries) = std::fs::read_dir(backups_in(&config.data_dir)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_str().is_some_and(is_leftover) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// A name only an interrupted run leaves in the backup folder: a partial
/// archive, or a snapshot an earlier release took there.
fn is_leftover(name: &str) -> bool {
    let Some(hidden) = name.strip_prefix('.') else {
        return false;
    };
    let partial = hidden.strip_suffix(".partial").is_some_and(is_valid_backup_name);
    let snapshot = hidden.strip_suffix(".db").is_some_and(|stamp| {
        stamp.len() == 15
            && stamp.char_indices().all(|(i, c)| if i == 8 { c == '-' } else { c.is_ascii_digit() })
    });
    partial || snapshot
}

/// Carry into a staged database what the installation has withdrawn since
/// the backup was taken.
///
/// A restore undoes damage by going back to a day before it, and a key
/// revoked because it leaked is part of the damage, not of the library: it
/// stays revoked. The account's password is today's and no session survives.
/// The signing secrets follow today's too, replaced ones included, so a
/// receiver already given the new secret keeps accepting, and none comes back
/// once they were withdrawn (`withdrawn_credentials`). A key the live
/// database never held, as on a new host, comes back as the backup has it,
/// and so do the backup's account and secrets when the live database holds
/// none and withdrew none.
async fn keep_withdrawn_credentials(live: &sqlx::SqlitePool, staged: &Path) -> AppResult<()> {
    use sqlx::{ConnectOptions, Connection};

    let revoked: Vec<(String, String)> =
        sqlx::query_as("SELECT id, revoked_at FROM api_keys WHERE revoked_at IS NOT NULL")
            .fetch_all(live)
            .await?;
    let secrets: Vec<(String, String)> =
        sqlx::query_as("SELECT secret, created_at FROM webhook_secrets").fetch_all(live).await?;
    let accounts: Vec<(String, String, String, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash, created_at, updated_at FROM users")
            .fetch_all(live)
            .await?;
    let sealing: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(sealed::SETTING)
        .fetch_optional(live)
        .await?;
    let withdrawn: Vec<(String, String)> =
        sqlx::query_as("SELECT name, withdrawn_at FROM withdrawn_credentials")
            .fetch_all(live)
            .await?;
    let secrets_withdrawn =
        withdrawn.iter().any(|(name, _)| name == Credential::SigningSecret.name());

    // A rollback journal, not a write-ahead log: the staged file is renamed
    // alone, and a log beside it would be left behind with these writes in it.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(staged)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .connect()
        .await?;
    let result: AppResult<()> = async {
        let mut tx = connection.begin().await?;
        // An archive from before a table existed has nothing to withdraw in
        // it: the migration that creates the table starts it empty.
        let holds = |table: &'static str| {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
            )
            .bind(table)
        };
        if holds("api_keys").fetch_one(&mut *tx).await? {
            for (id, revoked_at) in &revoked {
                sqlx::query(
                    "UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
                )
                .bind(revoked_at)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        // The password of today, and nobody signed in: a password changed
        // because it leaked closed every session, and restoring the archive's
        // would open them again.
        if !accounts.is_empty() {
            sqlx::query("DELETE FROM users").execute(&mut *tx).await?;
            for (id, username, hash, created_at, updated_at) in &accounts {
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(username)
                .bind(hash)
                .bind(created_at)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query("DELETE FROM sessions").execute(&mut *tx).await?;
        // The backup passphrase of today: an archive taken before it was set,
        // or changed, would otherwise bring back the old one, and every
        // archive after the restore would be sealed with it, or not at all.
        if let Some(sealing) = &sealing {
            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(sealed::SETTING)
            .bind(sealing)
            .execute(&mut *tx)
            .await?;
        }
        // An archive from before the signing secrets were stored gets their
        // table, as the migration that creates it would, or today's secrets
        // would be dropped and every notification go out unsigned.
        if !secrets.is_empty() {
            if !holds("webhook_secrets").fetch_one(&mut *tx).await? {
                sqlx::raw_sql(include_str!("../../../migrations/011_webhook_secrets.sql"))
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("DELETE FROM webhook_secrets").execute(&mut *tx).await?;
            for (secret, created_at) in &secrets {
                sqlx::query("INSERT INTO webhook_secrets (secret, created_at) VALUES (?, ?)")
                    .bind(secret)
                    .bind(created_at)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        // Secrets withdrawn since stay withdrawn, and the record goes with the
        // restored database, so a restore of an older archive still finds it.
        if secrets_withdrawn && holds("webhook_secrets").fetch_one(&mut *tx).await? {
            sqlx::query("DELETE FROM webhook_secrets").execute(&mut *tx).await?;
        }
        if !withdrawn.is_empty() {
            sqlx::raw_sql(include_str!("../../../migrations/038_withdrawn_credentials.sql"))
                .execute(&mut *tx)
                .await?;
            for (name, withdrawn_at) in &withdrawn {
                sqlx::query(
                    "INSERT INTO withdrawn_credentials (name, withdrawn_at) VALUES (?, ?)
                     ON CONFLICT(name) DO NOTHING",
                )
                .bind(name)
                .bind(withdrawn_at)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    connection.close().await.ok();
    result
}

/// Refuse a restore whose credentials the next start could not open.
///
/// `ROUTARR_SECRET_KEY` wins over any `routarr.key` at start, the one a
/// restore brings back included. An archive sealed under its key file, staged
/// on a host that sets the variable, would open with every Arr, source and
/// signing credential unreadable. Without the variable the next start opens
/// with the key file the archive brings back, or the one in place when it
/// brings none, and that file has to be the one its credentials were sealed
/// with.
async fn opened_by_the_next_start(config: &crate::config::Config, staged: &Path) -> AppResult<()> {
    use sqlx::{ConnectOptions, Connection};

    let brought_back = staging_path(&config.secret_key_path());
    let key_file = if brought_back.exists() { brought_back } else { config.secret_key_path() };
    let Some(key) = key_in_place(config, &key_file) else {
        return Ok(());
    };
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(staged)
        .read_only(true)
        .connect()
        .await?;
    // An archive from before the salt holds none, and its passphrase seals
    // are `enc:v1:`, which need none.
    let salt: Option<String> = sqlx::query_scalar("SELECT salt FROM secret_salt LIMIT 1")
        .fetch_optional(&mut connection)
        .await
        .ok()
        .flatten();
    let next_start = crate::crypto::SecretBox::load(
        Some(&key),
        config.previous_secret_key.as_deref(),
        &key_file,
        salt.as_deref().map(str::as_bytes),
    )?;
    let sealed: Option<String> =
        sqlx::query_scalar("SELECT api_key FROM instances WHERE api_key LIKE 'enc:%' LIMIT 1")
            .fetch_optional(&mut connection)
            .await?;
    connection.close().await.ok();

    let refusal = if config.secret_key.is_some() {
        "the credentials in this archive cannot be opened with ROUTARR_SECRET_KEY, which the next \
         start uses instead of the archive's routarr.key. Set ROUTARR_SECRET_KEY to the key the \
         archive was taken with, or unset it, then restore again."
    } else {
        "the credentials in this archive cannot be opened with the routarr.key the next start \
         would use. Set ROUTARR_SECRET_KEY to the key the archive was taken with, then restore \
         again."
    };
    match sealed {
        Some(sealed) if next_start.open(&sealed).is_err() => {
            Err(AppError::BadRequest(refusal.into()))
        }
        _ => Ok(()),
    }
}

/// The master key a start would use: `ROUTARR_SECRET_KEY`, or else what
/// `key_file` holds. Read here rather than by `SecretBox::load`, which writes
/// a new key where it finds none: whoever asks must leave the files as found.
fn key_in_place(config: &crate::config::Config, key_file: &Path) -> Option<String> {
    let from_file = std::fs::read_to_string(key_file)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    config.secret_key.clone().or(from_file)
}

/// Whether `passphrase` opens the sealed `archive`, as a restore would.
#[cfg(test)]
pub(crate) fn sealed_with(archive: &Path, passphrase: &str) -> bool {
    sealed::opens(archive, &Passphrase::new(passphrase.to_string())).unwrap_or(false)
}

/// Whether a file is a Routarr database this build can open.
///
/// Opened read-only and immutable, so the check writes nothing beside the file.
/// `integrity_check` finds a truncated or damaged file, and the migrations table
/// an empty one: SQLite opens a zero-byte file as a valid, empty database. The
/// last migration it records has to be one this build knows, since migrations
/// only go forward and a start is not always made by the build that staged it.
async fn check_database(path: &Path) -> Result<(), String> {
    use sqlx::{ConnectOptions, Connection};

    let options =
        sqlx::sqlite::SqliteConnectOptions::new().filename(path).read_only(true).immutable(true);
    let mut connection = options.connect().await.map_err(|e| e.to_string())?;

    let verdict: Result<Result<(), String>, sqlx::Error> = async {
        let integrity: String =
            sqlx::query_scalar("PRAGMA integrity_check").fetch_one(&mut connection).await?;
        if integrity != "ok" {
            return Ok(Err(integrity));
        }
        let migrated: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations')",
        )
        .fetch_one(&mut connection)
        .await?;
        if !migrated {
            return Ok(Err("it holds no Routarr schema".into()));
        }
        let schema: Option<String> =
            sqlx::query_scalar("SELECT name FROM _migrations ORDER BY id DESC LIMIT 1")
                .fetch_optional(&mut connection)
                .await?;
        if let Some(newer) = crate::db::opened_ahead(&mut connection).await? {
            return Ok(Err(format!(
                "it was opened by Routarr v{newer}, which migrated it further than this \
                 version knows"
            )));
        }
        Ok(match schema {
            None => Err("it holds no Routarr schema".into()),
            Some(schema) if crate::db::is_newer_schema(&schema) => Err(format!(
                "it is at schema '{schema}', which this version of Routarr does not know"
            )),
            Some(_) => Ok(()),
        })
    }
    .await;
    connection.close().await.ok();

    verdict.map_err(|e| e.to_string())?
}

/// Apply a staged restore, if one is pending.
///
/// Called from `main` **before** the pool is opened. Each file is moved into
/// place with a rename, which is atomic within a filesystem, so an interruption
/// leaves either the old file or the new one, never half of either. The
/// database goes last, as it was staged last: a pending database means its keys
/// are pending or applied, and keys pending without one are an interrupted
/// staging.
///
/// The database is checked again first. A pending set damaged since it was
/// staged would otherwise replace the live database on the next start, which
/// is also the start that follows an upgrade or a downgrade. Rejected, it is
/// removed rather than refused again on every start.
pub async fn apply_pending_restore(config: &crate::config::Config) -> AppResult<bool> {
    let targets = [config.db_path.clone(), config.secret_key_path(), config.api_key_path()];

    if !targets.iter().any(|target| pending_path(target).exists()) {
        return Ok(false);
    }

    let database = pending_path(&config.db_path);
    let verdict = if database.exists() {
        check_database(&database).await
    } else {
        Err("it carries no database".to_string())
    };
    if let Err(reason) = verdict {
        error!("A pending restore was rejected and the current database kept: {reason}");
        discard_pending(&targets);
        return Ok(false);
    }

    info!("A restore is pending, applying it before opening the database");

    for target in targets.into_iter().rev() {
        let staged = pending_path(&target);
        if !staged.exists() {
            continue;
        }

        // The file replaced is kept one generation, as `<target>.pre-restore`:
        // the wrong line picked in the list would otherwise lose everything
        // written since that archive. Its WAL goes with it under the name
        // SQLite looks for beside the copy, since it holds the latest writes.
        // Left in place next to the restored file, SQLite would replay it over
        // the restore. The shared-memory file is rebuilt, and goes.
        if target.exists() {
            let kept = with_suffix(&target, PRE_RESTORE);
            std::fs::rename(&target, &kept).map_err(|e| {
                AppError::Config(format!("cannot set {} aside: {e}", target.display()))
            })?;
            let wal = with_suffix(&target, "-wal");
            if wal.exists() {
                std::fs::rename(&wal, with_suffix(&kept, "-wal")).ok();
            } else {
                std::fs::remove_file(with_suffix(&kept, "-wal")).ok();
            }
            info!("The file restored over is kept at {}", kept.display());
        }
        std::fs::remove_file(with_suffix(&target, "-shm")).ok();

        std::fs::rename(&staged, &target)
            .map_err(|e| AppError::Config(format!("cannot restore {}: {e}", target.display())))?;
        crate::crypto::sync_parent(&target);
        crate::crypto::restrict_permissions(&target);
        info!("Restored {}", target.display());
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_name_routarr_generated_is_accepted() {
        assert!(is_valid_backup_name("routarr-backup-20260823-120000.zip"));
        assert!(is_valid_backup_name("routarr-backup-20260823-120000-app.zip.enc"));
        assert!(!is_valid_backup_name("routarr-backup-20260823-120000.enc"));

        // The download and delete endpoints take this from the URL: every way
        // out of the directory has to miss.
        assert!(!is_valid_backup_name("../routarr.key"));
        assert!(!is_valid_backup_name("routarr-backup-../../etc/passwd.zip"));
        assert!(!is_valid_backup_name("/etc/passwd"));
        assert!(!is_valid_backup_name("routarr-backup-a/b.zip"));
        assert!(!is_valid_backup_name("routarr.db"));
        assert!(!is_valid_backup_name("backup.zip"));
        assert!(!is_valid_backup_name(""));
    }

    fn manifest() -> BackupManifest {
        BackupManifest {
            version: "0.0.0".into(),
            schema: "001_initial_schema".into(),
            created_at: String::new(),
            includes_master_key: false,
        }
    }

    #[test]
    fn an_archive_is_never_written_over_one_that_exists() {
        let dir = crate::tests::TempDir::new("zip-exists");
        let archive = dir.join("routarr-backup-20260101-000000.zip");
        std::fs::write(&archive, b"an earlier archive").unwrap();
        let snapshot = dir.join(".20260101-000000.db");
        std::fs::write(&snapshot, b"a database").unwrap();

        let outcome = build_zip(
            &archive,
            &snapshot,
            &manifest(),
            &[Some(dir.join("routarr.key")), Some(dir.join("routarr.api_key"))],
            None,
        );

        assert!(outcome.is_err(), "an archive was written over another");
        assert_eq!(std::fs::read(&archive).unwrap(), b"an earlier archive");
        assert!(!partial_path(&archive).exists(), "the refused archive was left behind");
    }

    /// A sealed archive is zipped in the clear first: when the sealing fails,
    /// that zip goes too, master key and all.
    #[test]
    fn a_sealing_that_fails_leaves_no_zip_in_the_clear() {
        let dir = crate::tests::TempDir::new("zip-sealing-fails");
        let snapshot = dir.join("snapshot.db");
        std::fs::write(&snapshot, b"a database").unwrap();
        let zipped = dir.join("archive.zip");
        let nowhere = dir.join("missing").join("routarr-backup-20260101-000000.zip.enc");
        let passphrase = Passphrase::new("the passphrase".to_string());

        let outcome = build_zip(
            &nowhere,
            &snapshot,
            &manifest(),
            &[Some(dir.join("routarr.key")), Some(dir.join("routarr.api_key"))],
            Some((&passphrase, &zipped)),
        );

        assert!(outcome.is_err(), "an archive was sealed where it cannot be");
        assert!(!zipped.exists(), "the zip in the clear was left behind");
    }

    #[test]
    fn an_archive_that_fails_midway_is_not_left_under_a_backup_name() {
        let dir = crate::tests::TempDir::new("zip-fails");

        // The snapshot is gone by the time the database entry is copied, so
        // the archive fails after it was opened.
        let outcome = build_zip(
            &dir.join("routarr-backup-20260101-000000.zip"),
            &dir.join(".missing.db"),
            &manifest(),
            &[Some(dir.join("routarr.key")), Some(dir.join("routarr.api_key"))],
            None,
        );

        assert!(outcome.is_err(), "the archive was written without its database");
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        // A file under a backup name is listed, counted by the retention,
        // and restorable: an empty database the next start would move in.
        assert_eq!(left, Vec::<String>::new(), "a failed archive was left behind");
    }
}

#[cfg(test)]
mod archive_shape {
    /// The archive is written off the runtime, and the database is streamed.
    ///
    /// Neither property is visible to a behavioural test: a backup of the
    /// two-page database a test seeds blocks for microseconds and fits in a
    /// cache line, so the suite is green whichever way this is written. What
    /// makes it matter is the machine this runs on (the commentary through
    /// this crate designs for a two-core NAS) and the cadence, since
    /// `backup_enabled` defaults to true and the scheduler takes one a day.
    ///
    /// `api/backup.rs` streams the *download*, since reading an archive into
    /// memory first doubles the process's footprint. The write path is the
    /// same file, and it is the one nobody asks for.
    #[test]
    fn the_database_is_never_read_whole_and_never_deflated_on_the_runtime() {
        const SOURCE: &str = include_str!("mod.rs");

        let build = SOURCE
            .split_once("fn build_zip(")
            .expect("build_zip")
            .1
            .split_once("\n}")
            .expect("its closing brace")
            .0;
        // The guard on the parser: stop matching and every assertion below
        // passes having read nothing.
        assert!(build.len() > 400, "only {} bytes of build_zip parsed", build.len());

        assert!(
            build.contains("std::io::copy"),
            "the database entry must be copied through, not materialised"
        );
        assert!(
            !build.contains("std::fs::read(snapshot)"),
            "the snapshot is read whole into memory again"
        );

        let write = SOURCE
            .split_once("async fn write_archive(")
            .expect("write_archive")
            .1
            .split_once("\n}")
            .expect("its closing brace")
            .0;
        assert!(write.len() > 400, "only {} bytes of write_archive parsed", write.len());
        assert!(
            write.contains("spawn_blocking"),
            "deflating a library-sized database must not hold a runtime worker"
        );
    }
}
