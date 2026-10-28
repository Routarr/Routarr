//! Scheduled, restorable backups. An archive carries the master key and the API key with the
//! database, so it is as sensitive as the master key. A restore is staged and applied by the
//! next start before the pool opens, as replacing a database under an open pool destroys it.

mod copy;
mod sealed;

use sqlx::AssertSqlSafe;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

pub use copy::{
    BackupCopy, adopt as adopt_copy_folder, in_background as copy_in_background, last_copied_at,
    now as copy_now, status as copy_status,
};
pub use sealed::{
    MIN_LENGTH as MIN_PASSPHRASE_LENGTH, Passphrase, SETTING as PASSPHRASE_SETTING, is_sealed_file,
    passphrase_of, resume as resume_sealing, set as set_passphrase, stored_passphrase,
};

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::localization::{DEFAULT_LANGUAGE, Localizer};
use crate::state::AppState;

/// Written into every archive so a restore can refuse what it cannot honour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub version: String,
    /// The last migration applied. A newer one is refused, since restoring it would leave the
    /// database ahead of the code that reads it.
    pub schema: String,
    pub created_at: String,
    pub includes_master_key: bool,
}

/// One archive on disk.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct BackupFile {
    pub name: String,
    pub size_bytes: u64,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
    /// Sealed with the backup passphrase: a restore asks for it, and
    /// `routarr decrypt-backup` opens it by hand.
    pub encrypted: bool,
    /// In the copy folder as its last good copy left it, `ROUTARR_BACKUP_COPY_DIR`. False
    /// without one.
    pub copied: bool,
}

/// Entry names inside the archive. Renamed, every archive already taken stops restoring.
const DB_ENTRY: &str = "routarr.db";
const MASTER_KEY_ENTRY: &str = "routarr.key";
const API_KEY_ENTRY: &str = "routarr.api_key";
const MANIFEST_ENTRY: &str = "manifest.json";

/// Marker telling the next startup that a restore is pending.
const PENDING_SUFFIX: &str = ".restore-pending";
/// Beside the pending database: which archive it is and when it was staged.
const NOTE_SUFFIX: &str = ".restore-pending.json";
/// What a restore sets the replaced file aside as, one generation.
const PRE_RESTORE: &str = ".pre-restore";

/// A restore being written. Never applied: only a complete set whose database
/// has been checked is renamed to the pending suffix.
const STAGING_SUFFIX: &str = ".restore-staging";

/// Files removed on drop unless kept, so every early return takes them away. Left behind, a
/// half-written archive would be listed and restorable, and a half-written restore applied.
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

pub fn backup_dir(state: &AppState) -> PathBuf {
    backups_in(&state.config.data_dir)
}

pub(crate) fn backups_in(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// A name Routarr generated, and nothing else. The download and delete routes take it from the
/// URL, so only the exact generated shape may pass, which no path out of the folder matches.
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

/// Marks an archive an application key took. Those are retained under a count of their own,
/// so a script taking backups in a loop never prunes the owner's or the schedule's.
const APPLICATION_SUFFIX: &str = "-app";

/// Marks the archive a start takes before migrating, then this release's version: the way back
/// to the release before, which no retention count prunes. The release it leads back to must
/// still list it, so the name keeps to `[A-Za-z0-9._-]`.
const BEFORE_MIGRATING_SUFFIX: &str = "-pre-";

fn before_migrating_suffix() -> String {
    let version = env!("CARGO_PKG_VERSION")
        .replace(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'), "-");
    format!("{BEFORE_MIGRATING_SUFFIX}{version}")
}

/// What a name holds after its UTC stamp: `-app`, `-pre-<version>`, or nothing.
fn kind_of(name: &str) -> &str {
    stamp_of(name).and_then(|stamp| stamp.get(STAMP.len()..)).unwrap_or_default()
}

/// The shape of the stamp every generated name starts with.
const STAMP: &str = "20260101-000000";

/// How long after the newest archive an application key may take another.
const APPLICATION_GAP: std::time::Duration = std::time::Duration::from_secs(10 * 60);

pub async fn create(state: &AppState, by: &Attribution) -> AppResult<BackupFile> {
    let localizer = state.localizer().await;
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
        let message =
            localizer.translate("ErrorBackupTooSoon", &[("seconds", &seconds.to_string())]);
        return Err(AppError::TooManyRequests { message, retry_after: seconds });
    }
    let Some(_lock) = state.jobs.try_lock("backup") else {
        return Err(AppError::InProgress {
            reason: "backup_running",
            message: localizer.translate("ErrorBackupRunning", &[]),
        });
    };

    let mut job =
        state.jobs.start(JobKind::Backup, by, None, Detail::new("JobDetailBackingUp")).await?;
    crate::race::checked("backup::create", "").await;
    let suffix = if by_application { APPLICATION_SUFFIX } else { "" };
    let outcome = match sealed::passphrase(state).await {
        Ok(passphrase) => {
            let job = Some(job.id.as_str());
            let (config, pool) = (&state.config, &state.pool);
            write_archive(config, pool, suffix, passphrase, job, &localizer, free_space).await
        }
        Err(e) => Err(e),
    };
    let outcome = outcome.map_err(|e| match e {
        AppError::InProgress { reason: TAKEN_THIS_SECOND, .. } => AppError::InProgress {
            reason: TAKEN_THIS_SECOND,
            message: localizer.translate("ErrorBackupThisSecond", &[]),
        },
        other => other,
    });

    match &outcome {
        Ok(file) => {
            job.report(file);
            job.succeed(Detail::new("JobDetailBackedUp").with("file", &file.name)).await
        }
        Err(e) => job.fail(e).await,
    }

    if outcome.is_ok() {
        // A failed prune must not make the archive just written look like a failure.
        if let Err(e) = prune(state).await {
            warn!("Could not prune old backups: {e}");
        }
        copy::in_background(state, by.clone());
    }

    outcome
}

/// Leave TMDB's answers out of the snapshot, the pages they held zeroed: its terms allow six
/// months of them and an archive is kept longer. Mark the backup's own job a success inside it,
/// or every restore would bring it back running, for the next start to mark interrupted.
async fn trim_snapshot(snapshot: &Path, job: Option<&str>, name: &str) -> AppResult<()> {
    use sqlx::{ConnectOptions, Connection};
    // A rollback journal: the snapshot is zipped alone, so a log beside it would be lost.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(snapshot)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .pragma("secure_delete", "ON")
        .connect()
        .await?;
    let trimmed = async {
        sqlx::query("DELETE FROM metadata_cache WHERE source = ?")
            .bind(crate::services::metadata::TMDB)
            .execute(&mut connection)
            .await?;
        if let Some(job) = job {
            let detail = crate::jobs::Detail::new("JobDetailBackedUp").with("file", name);
            sqlx::query(
                "UPDATE jobs SET status = 'success', detail = ?, detail_key = ?, detail_params = ?,
                        finished_at = datetime('now')
                  WHERE id = ?",
            )
            .bind(detail.english())
            .bind("JobDetailBackedUp")
            .bind(detail.stored_params())
            .bind(job)
            .execute(&mut connection)
            .await?;
        }
        Ok::<_, sqlx::Error>(())
    }
    .await;
    connection.close().await.ok();
    trimmed?;
    Ok(())
}

/// What a start that migrates did to keep the way back.
pub enum BeforeMigrating {
    /// The archive it took.
    Taken(BackupFile),
    /// The archive an earlier attempt of this release took of the database as it stands, a
    /// start whose migration failed being retried.
    Kept(String),
}

/// Back up a database before a start migrates it: every later archive is of the new schema,
/// which the release before cannot open. Once per release, schema and state of the database,
/// however often the start is retried.
pub async fn before_migrating(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
) -> AppResult<BeforeMigrating> {
    let passphrase = sealed::stored_passphrase(config, pool).await?;
    let english = Localizer::new(DEFAULT_LANGUAGE);
    let schema = last_migration(pool).await?;
    let suffix = before_migrating_suffix();
    let dir = backups_in(&config.data_dir);
    let kept =
        list_in(&dir).into_iter().filter(|file| kind_of(&file.name) == suffix).find_map(|file| {
            let manifest = manifest_of(&dir.join(&file.name), passphrase.as_ref(), &english)?;
            (manifest.schema == schema).then_some((file.name, manifest.created_at))
        });
    let worked_on = last_worked_on(pool).await?;
    let now = crate::services::routing::format_timestamp(chrono::Utc::now());
    // A stamp ahead of the clock orders nothing, and a looping start must take one archive.
    let unchanged_since = |taken_at: &str| {
        worked_on
            .as_deref()
            .is_none_or(|worked_on| worked_on <= taken_at || worked_on > now.as_str())
    };
    if let Some((name, taken_at)) = kept
        && unchanged_since(&taken_at)
    {
        return Ok(BeforeMigrating::Kept(name));
    }
    let file = write_archive(config, pool, &suffix, passphrase, None, &english, free_space).await?;
    Ok(BeforeMigrating::Taken(file))
}

/// When a release last worked on the database, as a stored timestamp: the newest job, since
/// every release records its hourly maintenance as one, and every schema holds `jobs`.
async fn last_worked_on(pool: &sqlx::SqlitePool) -> AppResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT MAX(COALESCE(finished_at, started_at)) FROM jobs")
        .fetch_one(pool)
        .await?)
}

async fn last_migration(pool: &sqlx::SqlitePool) -> AppResult<String> {
    Ok(sqlx::query_scalar("SELECT name FROM _migrations ORDER BY id DESC LIMIT 1")
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| "unknown".to_string()))
}

/// The bytes left on the disk of `dir`.
fn free_space(dir: &Path) -> std::io::Result<u64> {
    fs4::available_space(dir)
}

/// What a backup of a `database` this size needs on its disk, the database's growth included:
/// twice the database and 100 MiB.
pub(crate) fn backup_needs(database: u64) -> u64 {
    database.saturating_mul(2).saturating_add(100 * 1024 * 1024)
}

/// Fewer bytes `free` than a backup of `database` needs.
pub(crate) fn disk_is_low(free: u64, database: u64) -> bool {
    free < backup_needs(database)
}

async fn write_archive(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
    suffix: &str,
    passphrase: Option<Passphrase>,
    job: Option<&str>,
    localizer: &Localizer,
    free_space: fn(&Path) -> std::io::Result<u64>,
) -> AppResult<BackupFile> {
    let database = std::fs::metadata(&config.db_path).map_or(0, |meta| meta.len());
    if let Ok(free) = free_space(&config.data_dir)
        && disk_is_low(free, database)
    {
        let bytes = |n: u64| {
            crate::localization::human_bytes(i64::try_from(n).unwrap_or(i64::MAX), localizer)
        };
        let (free, needed) = (bytes(free), bytes(backup_needs(database)));
        let message =
            localizer.translate("ErrorBackupNoSpace", &[("free", &free), ("needed", &needed)]);
        return Err(AppError::Conflict { reason: "disk_full", message });
    }
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
    // Named to the second: refused here before a whole copy is written, and again at the end.
    if path.exists() || dir.join(&plain_name).exists() {
        return Err(taken_this_second());
    }

    let snapshot = work_file(&config.data_dir, "snapshot.db")?;
    // The whole database, removed on any exit, by the archive task if the caller cancels.
    let snapshot_scaffold = Scaffold(vec![snapshot.clone()]);
    vacuum_into(pool, &snapshot).await?;
    trim_snapshot(&snapshot, job, &name).await?;

    let schema = last_migration(pool).await?;

    // With `ROUTARR_SECRET_KEY` set, a `routarr.key` is stale: restored, it opens nothing.
    let master_key =
        Some(config.secret_key_path()).filter(|path| config.secret_key.is_none() && path.exists());
    let manifest = BackupManifest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema,
        created_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
        includes_master_key: master_key.is_some(),
    };

    // Off the runtime: deflating a library-sized database would hold a worker throughout.
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
    Ok(BackupFile { name, size_bytes, created_at: manifest.created_at, encrypted, copied: false })
}

const TAKEN_THIS_SECOND: &str = "backup_just_taken";

/// This second's archive name is taken. In English for a start's log, translated by `create`.
fn taken_this_second() -> AppError {
    AppError::InProgress {
        reason: TAKEN_THIS_SECOND,
        message: "A backup was taken this second. Try again in a moment.".into(),
    }
}

async fn vacuum_into(pool: &sqlx::SqlitePool, target: &Path) -> AppResult<()> {
    // `VACUUM INTO` refuses to overwrite a file an interrupted run left.
    std::fs::remove_file(target).ok();

    // `VACUUM INTO` binds no path: safe only while `target` never comes from a request.
    let quoted = target.to_string_lossy().replace('\'', "''");
    sqlx::query(AssertSqlSafe(format!("VACUUM INTO '{quoted}'"))).execute(pool).await?;

    // `VACUUM INTO` succeeds against an in-memory database without writing anything.
    if !target.exists() {
        return Err(AppError::Internal(format!(
            "the database snapshot was not written to {}",
            target.display()
        )));
    }
    // Created under the umask, and it holds the whole database with its sealed credentials.
    crate::crypto::restrict_permissions(target);

    Ok(())
}

/// Write the archive on a blocking thread, streaming the database, never reading it whole.
/// With `sealing`, the zip is written in the clear at its path first, as a zip seeks back to
/// write its directory, and that file is removed whatever happens.
fn build_zip(
    path: &Path,
    snapshot: &Path,
    manifest: &BackupManifest,
    keys: &[Option<PathBuf>; 2],
    sealing: Option<(&Passphrase, &Path)>,
) -> AppResult<()> {
    // Hidden from `list` until whole: `ZipWriter` finishes a readable zip on drop, on error too.
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

    // The zip carries the master key and the API key in clear: private from its first byte.
    let file = crate::crypto::create_private(&zipped, true)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", zipped.display())))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    fn add(
        zip: &mut zip::ZipWriter<std::fs::File>,
        options: zip::write::SimpleFileOptions,
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

    // Streamed: read whole, a library-sized database would sit in memory twice.
    zip.start_file(DB_ENTRY, options)
        .map_err(|e| AppError::Internal(format!("cannot add {DB_ENTRY} to the archive: {e}")))?;
    let mut source = std::fs::File::open(snapshot)
        .map_err(|e| AppError::Internal(format!("cannot read the snapshot: {e}")))?;
    std::io::copy(&mut source, &mut zip)
        .map_err(|e| AppError::Internal(format!("cannot write {DB_ENTRY}: {e}")))?;

    // Read whole, at a few bytes each. Without the master key no restored credential opens.
    for (entry, source) in [MASTER_KEY_ENTRY, API_KEY_ENTRY].iter().zip(keys) {
        if let Some(Ok(bytes)) = source.as_ref().map(std::fs::read) {
            add(&mut zip, options, entry, &bytes)?;
        }
    }

    let file =
        zip.finish().map_err(|e| AppError::Internal(format!("cannot finish the archive: {e}")))?;
    // Synced before the rename, or a power cut can leave an empty file under a backup name.
    file.sync_all()
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", zipped.display())))?;
    if let Some((passphrase, _)) = sealing {
        sealed::seal(&zipped, &partial, passphrase)?;
        std::fs::remove_file(&zipped).ok();
    }

    // Never over an archive that already exists, which `rename` would replace.
    if path.exists() {
        return Err(taken_this_second());
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

/// Where the copies in the clear go: snapshots, zips before sealing, opened archives. Beside
/// the database, never in the backup folder a sync copies away. Mode 0700, so a file created
/// inside under the umask, as `VACUUM INTO` does, stays private.
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

/// The instant the schedule counts its interval from. An application key's archives are left
/// out, or one taken every few minutes would hold the schedule off for good.
pub fn newest_taken(state: &AppState) -> Option<tokio::time::Instant> {
    let taken = last_taken_at(state)?;
    let age = (chrono::Utc::now() - taken).to_std().unwrap_or_default();
    tokio::time::Instant::now().checked_sub(age)
}

/// When the newest archive the schedule or a person took was written: a script taking them in
/// a loop would hide a schedule that stopped.
pub fn last_taken_at(state: &AppState) -> Option<chrono::DateTime<chrono::Utc>> {
    list(state)
        .iter()
        .find(|file| !taken_by_a_key(file) && !taken_before_migrating(file))
        .and_then(taken_at)
}

fn taken_by_a_key(file: &BackupFile) -> bool {
    kind_of(&file.name) == APPLICATION_SUFFIX
}

fn taken_before_migrating(file: &BackupFile) -> bool {
    kind_of(&file.name).starts_with(BEFORE_MIGRATING_SUFFIX)
}

fn taken_at(file: &BackupFile) -> Option<chrono::DateTime<chrono::Utc>> {
    stamped_at(&file.name)
}

/// When `name` was taken, by its UTC stamp: a copy of the folder resets file dates.
fn stamped_at(name: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let stamp = stamp_of(name)?.get(..STAMP.len())?;
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
                copied: false,
            })
        })
        .collect();

    // The stamp in the name sorts by age without trusting file dates.
    files.sort_by(|a, b| b.name.cmp(&a.name));
    files
}

/// Delete everything past the retention count, oldest first: the archives
/// application keys took counted apart from the others.
pub async fn prune(state: &AppState) -> AppResult<usize> {
    let keep = kept(state).await?;
    let removed = prune_in(&backup_dir(state), keep, None);
    if removed > 0 {
        info!("Removed {removed} backup(s) past the retention count of {keep}");
    }
    Ok(removed)
}

/// Prune both folders to a retention count just set.
pub async fn retention_changed(state: &AppState, by: Attribution) {
    if let Err(e) = prune(state).await {
        warn!("Could not prune backups after the retention changed: {e}");
    }
    copy::in_background(state, by);
}

/// The retention count, read as stored, above the ceiling too: only the operator's own save may
/// lower it.
async fn kept(state: &AppState) -> AppResult<usize> {
    Ok(state.bounding_setting::<usize>("backup_retention_count").await?.max(1))
}

/// Delete the archives of `dir` past `keep`, oldest first, those application keys took counted
/// apart, and say how many went. An archive taken before migrating stays until deleted by hand.
/// With `removable`, only the archives it names count and go.
fn prune_in(
    dir: &Path,
    keep: usize,
    removable: Option<&std::collections::HashSet<String>>,
) -> usize {
    let (taken_by_keys, others): (Vec<BackupFile>, Vec<BackupFile>) = list_in(dir)
        .into_iter()
        .filter(|file| !taken_before_migrating(file))
        .filter(|file| removable.is_none_or(|names| names.contains(&file.name)))
        .partition(taken_by_a_key);
    [taken_by_keys, others]
        .into_iter()
        .flat_map(|files| files.into_iter().skip(keep))
        .filter(|file| std::fs::remove_file(dir.join(&file.name)).is_ok())
        .count()
}

/// Delete an archive, and its copy behind it.
pub fn delete(state: &AppState, name: &str) -> AppResult<()> {
    if !is_valid_backup_name(name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let path = backup_dir(state).join(name);
    std::fs::remove_file(&path).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    copy::forget_in_background(state, name);
    Ok(())
}

/// Read an archive's manifest without extracting anything.
pub fn read_manifest(archive: &Path, localizer: &Localizer) -> AppResult<BackupManifest> {
    let file =
        std::fs::File::open(archive).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| unreadable(archive, &e, localizer))?;

    let mut entry = zip
        .by_name(MANIFEST_ENTRY)
        .map_err(|_| AppError::BadRequest(localizer.translate("ErrorBackupNoManifest", &[])))?;

    let mut raw = String::new();
    std::io::Read::read_to_string(&mut entry, &mut raw)
        .map_err(|e| manifest_unreadable(archive, &e, localizer))?;

    serde_json::from_str(&raw).map_err(|e| manifest_unreadable(archive, &e, localizer))
}

/// `archive` is no zip this build reads. What the reader made of it goes to
/// the log, since it names offsets and signatures nobody restoring can use.
fn unreadable(archive: &Path, e: &dyn std::fmt::Display, localizer: &Localizer) -> AppError {
    warn!("{} cannot be read as an archive: {e}", archive.display());
    AppError::BadRequest(localizer.translate("ErrorBackupUnreadable", &[]))
}

/// The manifest of `archive` is there and cannot be read.
pub(super) fn manifest_unreadable(
    archive: &Path,
    e: &dyn std::fmt::Display,
    localizer: &Localizer,
) -> AppError {
    warn!("The manifest of {} cannot be read: {e}", archive.display());
    AppError::BadRequest(localizer.translate("ErrorBackupManifestUnreadable", &[]))
}

/// Validate an archive and stage it beside its targets for the next start to apply. One at a
/// time, on a task of its own: a request dropped halfway must not free the lock while its copy
/// goes on. A sealed archive opens with `given`, or else the stored passphrase.
pub async fn stage_restore(
    state: &AppState,
    name: &str,
    given: Option<Passphrase>,
) -> AppResult<BackupManifest> {
    if !is_valid_backup_name(name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let localizer = state.localizer().await;
    let Some(lock) = state.jobs.try_lock("restore") else {
        return Err(AppError::InProgress {
            reason: "restore_staging",
            message: localizer.translate("ErrorRestoreStaging", &[]),
        });
    };

    let state = state.clone();
    let name = name.to_string();
    tokio::spawn(async move {
        let _lock = lock;
        let archive = backup_dir(&state).join(&name);
        let stored = sealed::passphrase(&state).await.ok().flatten();
        let opened =
            opened_if_sealed(&state.config.data_dir, &archive, given, stored, &localizer).await?;
        let source = opened.as_ref().map_or(archive.as_path(), |opened| opened.path.as_path());
        let live = Some((&state.pool, Some(&state.secrets)));
        stage(&state.config, live, source, &name, &localizer).await
    })
    .await
    .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))?
}

/// `archive` opened in the work directory when sealed, with `given` or else `stored`. A zip in
/// the clear under a sealed name is refused: somebody put it there, and restoring it would
/// bring back whatever it holds.
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
            return Err(AppError::BadRequest(localizer.translate("ErrorBackupNotSealed", &[])));
        }
        return Ok(None);
    }
    let refusal = if given.is_some() { "ErrorPassphraseWrong" } else { "ErrorPassphraseNeeded" };
    let tried: Vec<Passphrase> = given.into_iter().chain(stored).collect();
    let (data_dir, path) = (data_dir.to_path_buf(), archive.to_path_buf());
    let opening = localizer.clone();
    let opened = tokio::task::spawn_blocking(move || {
        sealed::opened_copy(&data_dir, &path, &tried.iter().collect::<Vec<_>>(), &opening)
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

/// Stage `archive`, carrying into it what `live`, the database it replaces, must keep of
/// today, what `live_secrets` sealed resealed for the key the next start opens it with.
async fn stage(
    config: &crate::config::Config,
    live: Option<(&sqlx::SqlitePool, Option<&crate::crypto::SecretBox>)>,
    archive: &Path,
    name: &str,
    localizer: &Localizer,
) -> AppResult<BackupManifest> {
    let archive = archive.to_path_buf();
    let manifest = read_manifest(&archive, localizer)?;

    // Migrations only go forward, so a newer schema would sit ahead of this binary.
    if crate::db::is_newer_schema(&manifest.schema) {
        return Err(AppError::BadRequest(
            localizer.translate("ErrorBackupNewerSchema", &[("schema", &manifest.schema)]),
        ));
    }
    // Nor does a schema older than this build's first migration: a start would refuse it.
    if crate::db::is_before_0_1_5(&manifest.schema) {
        return Err(AppError::BadRequest(
            localizer.translate("ErrorBackupBefore015", &[("version", &manifest.version)]),
        ));
    }

    // Off the runtime: the database entry is copied out whole, at the library's size.
    let targets = [
        (DB_ENTRY, config.db_path.clone()),
        (MASTER_KEY_ENTRY, config.secret_key_path()),
        (API_KEY_ENTRY, config.api_key_path()),
    ];
    let paths = targets.clone().map(|(_, target)| target);

    let named = name.to_string();
    let listed_key = manifest.includes_master_key;
    let extracting = localizer.clone();
    let staged = tokio::task::spawn_blocking(move || {
        extract_staged(&archive, &targets, &named, listed_key, &extracting)
    })
    .await
    .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))??;

    // A zero-byte or truncated file opens as a database, and would replace the live one.
    let staged_database = staging_path(&config.db_path);
    if let Err(reason) = check_database(&staged_database).await {
        return Err(AppError::BadRequest(
            localizer.translate("ErrorBackupCannotRestore", &[("reason", &reason)]),
        ));
    }
    let key_withdrawn = match live {
        Some((live, _)) => is_withdrawn(live, Credential::MasterApiKey).await?,
        None => false,
    };
    let next_start = opened_by_the_next_start(config, &staged_database, localizer).await?;
    if let Some((live, live_secrets)) = live {
        let sealing = live_secrets.zip(next_start.as_ref());
        keep_todays_credentials(live, &staged_database, sealing).await?;
    }

    // The earlier restore goes whole, and only once this one has passed every check.
    discard_pending(&paths);
    let note = Note {
        name: name.to_string(),
        staged_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
    };
    if let Err(e) = write_note(&config.db_path, &note) {
        return Err(AppError::Internal(format!("cannot record the restore staged: {e}")));
    }

    // The database last: its pending file is what tells a start the set is complete.
    for target in paths.iter().rev() {
        let staging = staging_path(target);
        if !staged.holds(&staging) {
            continue;
        }
        // Never restore an API key over one in use, withdrawn, or set in `ROUTARR_API_KEY`.
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

/// Stage an archive with the server stopped, for `routarr restore`: the way back when a start
/// refuses the database. A sealed one opens with `given`, or else the stored passphrase.
pub async fn stage_offline(
    config: &crate::config::Config,
    archive: &str,
    given: Option<Passphrase>,
) -> AppResult<BackupManifest> {
    let path = archive_named(config, archive)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    // Withdrawn credentials carry over, as in an online restore.
    let live = database_in_place(config).await?;
    let staged = async {
        let (stored, live_secrets) = match &live {
            Some(live) => (
                sealed::stored_passphrase(config, live).await.ok().flatten(),
                secrets_in_place(config, live).await.ok().flatten(),
            ),
            None => (None, None),
        };
        let localizer = Localizer::new(DEFAULT_LANGUAGE);
        let opened = opened_if_sealed(&config.data_dir, &path, given, stored, &localizer).await?;
        let source = opened.as_ref().map_or(path.as_path(), |opened| opened.path.as_path());
        let live = live.as_ref().map(|live| (live, live_secrets.as_ref()));
        stage(config, live, source, &name, &localizer).await
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

/// The database a command run with the server stopped reads. Never migrated: it may be the
/// very database a start refuses.
async fn database_in_place(config: &crate::config::Config) -> AppResult<Option<sqlx::SqlitePool>> {
    use sqlx::sqlite::SqliteConnectOptions;

    if !config.db_path.is_file() {
        return Ok(None);
    }
    let options = crate::db::with_paths(SqliteConnectOptions::new().filename(&config.db_path));
    Ok(Some(crate::db::pool_options(1).connect_with(options).await?))
}

/// Open the sealed `archive` into a zip at `out`, for `routarr decrypt-backup`. The zip holds
/// the master key in the clear: written private, never over an existing file, and never into
/// the backup folder, which the server serves and a sync copies away.
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
        return Err(AppError::Conflict {
            reason: "file_exists",
            message: format!("{} exists already", out.display()),
        });
    }
    let folder = |path: &Path| path.parent().and_then(|parent| std::fs::canonicalize(parent).ok());
    let archive_folders = [Some(backups_in(&config.data_dir)), config.backup_copy_dir.clone()];
    if folder(&out).is_some_and(|parent| {
        archive_folders
            .iter()
            .flatten()
            .any(|dir| std::fs::canonicalize(dir).ok() == Some(parent.clone()))
    }) {
        return Err(AppError::BadRequest(
            "an archive opened is not written into a backup folder, which a sync copies away"
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
    // Read at a start, where a refusal only counts the archive out.
    let localizer = Localizer::new(DEFAULT_LANGUAGE);
    list_in(&dir).into_iter().map(|file| file.name).find(|name| {
        manifest_of(&dir.join(name), passphrase, &localizer).is_some_and(|m| {
            !crate::db::is_newer_schema(&m.schema) && !crate::db::is_before_0_1_5(&m.schema)
        })
    })
}

/// The manifest of `archive`, a sealed one read when `passphrase` opens it.
fn manifest_of(
    archive: &Path,
    passphrase: Option<&Passphrase>,
    localizer: &Localizer,
) -> Option<BackupManifest> {
    match (sealed::is_sealed_file(archive), passphrase) {
        (false, _) => read_manifest(archive, localizer).ok(),
        (true, None) => None,
        (true, Some(passphrase)) => sealed::manifest(archive, passphrase, localizer).ok().flatten(),
    }
}

/// Copy the three known entries beside their targets under the staging suffix, on a blocking
/// thread. No name read from the archive ever reaches a path. The files return as a scaffold,
/// removed unless the caller keeps them.
fn extract_staged(
    archive: &Path,
    targets: &[(&str, PathBuf); 3],
    name: &str,
    listed_key: bool,
    localizer: &Localizer,
) -> AppResult<Scaffold> {
    let refused =
        |key: &str, values: &[(&str, &str)]| AppError::BadRequest(localizer.translate(key, values));
    let file =
        std::fs::File::open(archive).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| unreadable(archive, &e, localizer))?;

    let mut scaffold = Scaffold(Vec::new());
    for (entry, target) in targets {
        let mut source = match zip.by_name(entry) {
            Ok(source) => source,
            Err(zip::result::ZipError::FileNotFound) => {
                // Keys without their database are no restore.
                if *entry == DB_ENTRY {
                    return Err(refused("ErrorBackupNoDatabase", &[]));
                }
                // Listed and missing, the key is lost, and no sealed credential would open.
                if *entry == MASTER_KEY_ENTRY && listed_key {
                    return Err(refused("ErrorBackupKeyMissing", &[]));
                }
                // Logged, since without its master key no restored credential opens here.
                warn!(
                    "Backup {name} carries no '{entry}', so the restore leaves the current one in place"
                );
                continue;
            }
            // Unreadable is damage, which must not pass for an absent key.
            Err(e) => {
                warn!("{entry} of backup {name} cannot be read: {e}");
                return Err(refused("ErrorBackupDamaged", &[("entry", entry)]));
            }
        };
        let staging = staging_path(target);
        scaffold.0.push(staging.clone());
        let mut out = crate::crypto::create_private(&staging, false)
            .map_err(|e| AppError::Internal(format!("cannot stage {}: {e}", staging.display())))?;
        std::io::copy(&mut source, &mut out).map_err(|e| copy_error(entry, e, localizer))?;
        // Synced before a rename marks it pending, so the pending file holds what is checked.
        out.sync_all().map_err(|e| AppError::Internal(format!("cannot stage {entry}: {e}")))?;
    }

    Ok(scaffold)
}

/// A read failing on the archive's side is damage the operator can mend with another archive.
/// An entry's CRC is checked at its end, once part of it is copied.
fn copy_error(entry: &str, e: std::io::Error, localizer: &Localizer) -> AppError {
    use std::io::ErrorKind::{InvalidData, InvalidInput, UnexpectedEof};
    match e.kind() {
        InvalidData | InvalidInput | UnexpectedEof => {
            warn!("{entry} cannot be read out of the archive: {e}");
            AppError::BadRequest(localizer.translate("ErrorBackupDamaged", &[("entry", entry)]))
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

fn discard_pending(targets: &[PathBuf]) {
    for target in targets {
        std::fs::remove_file(pending_path(target)).ok();
        std::fs::remove_file(with_suffix(target, NOTE_SUFFIX)).ok();
    }
}

/// What the note beside a pending database holds.
#[derive(Serialize, Deserialize)]
struct Note {
    name: String,
    staged_at: String,
}

fn write_note(database: &Path, note: &Note) -> std::io::Result<()> {
    let path = with_suffix(database, NOTE_SUFFIX);
    let mut file = crate::crypto::create_private(&path, false)?;
    file.write_all(&serde_json::to_vec(note)?)?;
    file.sync_all()
}

/// A restore staged for the next start, which applies it whenever it comes.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PendingRestore {
    /// The archive staged, or the pending file's own name when nothing records it.
    pub name: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub staged_at: String,
}

/// The restore the next start applies, `None` when none is staged.
pub fn pending_restore(config: &crate::config::Config) -> Option<PendingRestore> {
    let database = pending_path(&config.db_path);
    let written = std::fs::metadata(&database).ok()?.modified().ok();
    let note: Option<Note> = std::fs::read(with_suffix(&config.db_path, NOTE_SUFFIX))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    Some(match note {
        Some(Note { name, staged_at }) => PendingRestore { name, staged_at },
        None => PendingRestore {
            name: database.file_name()?.to_string_lossy().into_owned(),
            staged_at: written
                .map(chrono::DateTime::<chrono::Utc>::from)
                .map(crate::services::routing::format_timestamp)
                .unwrap_or_default(),
        },
    })
}

/// Withdraw the restore staged for the next start, behind any staging under way. `None` when
/// none was staged.
pub async fn withdraw_pending_restore(state: &AppState) -> AppResult<Option<PendingRestore>> {
    let Some(_lock) = state.jobs.try_lock("restore") else {
        return Err(AppError::InProgress {
            reason: "restore_staging",
            message: state.localizer().await.translate("ErrorRestoreStaging", &[]),
        });
    };
    let config = &state.config;
    let targets = [config.db_path.clone(), config.secret_key_path(), config.api_key_path()];
    let pending = pending_restore(config);
    if pending.is_none() && !targets.iter().any(|target| pending_path(target).exists()) {
        return Ok(None);
    }
    discard_pending(&targets);
    crate::crypto::sync_parent(&config.db_path);
    let withdrawn =
        pending.unwrap_or(PendingRestore { name: String::new(), staged_at: String::new() });
    info!("The restore of {} was withdrawn", withdrawn.name);
    Ok(Some(withdrawn))
}

/// Remove what an interrupted backup or staging leaves, the database and the master key in
/// the clear among it. Called from `main` before anything runs, the one moment none of these
/// files can belong to work in progress.
pub fn sweep_leftovers(config: &crate::config::Config) {
    for target in [&config.db_path, &config.secret_key_path(), &config.api_key_path()] {
        std::fs::remove_file(staging_path(target)).ok();
    }
    if !pending_path(&config.db_path).exists() {
        std::fs::remove_file(with_suffix(&config.db_path, NOTE_SUFFIX)).ok();
    }
    // Everything in the work directory is a run's, and none is running yet.
    if let Ok(entries) = std::fs::read_dir(config.data_dir.join(".backup-work")) {
        for entry in entries.filter_map(Result::ok) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
    sweep_in(&backups_in(&config.data_dir));
}

/// Remove the leftovers of an interrupted run from an archive folder.
fn sweep_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_str().is_some_and(is_leftover) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// A name only an interrupted run leaves in the backup folder: a partial archive, or a
/// `.<stamp>.db` snapshot an upgraded installation may still hold.
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

/// Carry today's revocations, password, signing secrets, webhook tokens and withdrawals into a
/// staged database, the archive's own counting only where today holds and withdrew none. No
/// session survives. `sealing`, the live box then the next start's, reseals what is carried.
async fn keep_todays_credentials(
    live: &sqlx::SqlitePool,
    staged: &Path,
    sealing: Option<(&crate::crypto::SecretBox, &crate::crypto::SecretBox)>,
) -> AppResult<()> {
    use sqlx::{ConnectOptions, Connection};

    let revoked: Vec<(String, String)> =
        sqlx::query_as("SELECT id, revoked_at FROM api_keys WHERE revoked_at IS NOT NULL")
            .fetch_all(live)
            .await?;
    let secrets: Vec<(String, String)> =
        sqlx::query_as("SELECT secret, created_at FROM webhook_secrets").fetch_all(live).await?;
    let tokens: Vec<(String, Option<String>, Option<String>)> =
        if has_column(live, "instances", "webhook_token_at").await? {
            sqlx::query_as("SELECT id, webhook_token, webhook_token_at FROM instances")
                .fetch_all(live)
                .await?
        } else {
            sqlx::query_as("SELECT id, webhook_token, NULL FROM instances").fetch_all(live).await?
        };
    let accounts: Vec<(String, String, String, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash, created_at, updated_at FROM users")
            .fetch_all(live)
            .await?;
    let passphrase: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(sealed::SETTING)
        .fetch_optional(live)
        .await?;
    let withdrawn: Vec<(String, String)> =
        sqlx::query_as("SELECT name, withdrawn_at FROM withdrawn_credentials")
            .fetch_all(live)
            .await?;
    let secrets_withdrawn =
        withdrawn.iter().any(|(name, _)| name == Credential::SigningSecret.name());
    let adopted: Vec<String> = if has_table(live, "backup_copy_folders").await? {
        sqlx::query_scalar("SELECT path FROM backup_copy_folders").fetch_all(live).await?
    } else {
        Vec::new()
    };
    let copied: Vec<(String, String)> = if has_table(live, "backup_copies").await? {
        sqlx::query_as("SELECT name, copied_at FROM backup_copies").fetch_all(live).await?
    } else {
        Vec::new()
    };

    // A rollback journal: the staged file is renamed alone, so a log beside it would be lost.
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(staged)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .connect()
        .await?;
    let result: AppResult<()> = async {
        let mut tx = connection.begin().await?;
        for (id, revoked_at) in &revoked {
            sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
                .bind(revoked_at)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        // Today's password, and no session: a password changed since may have leaked.
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
        // Today's backup passphrase, or later archives would be sealed with an old one, or none.
        if let Some(passphrase) = &passphrase {
            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(sealed::SETTING)
            .bind(resealed(passphrase, sealing))
            .execute(&mut *tx)
            .await?;
        }
        if !secrets.is_empty() {
            sqlx::query("DELETE FROM webhook_secrets").execute(&mut *tx).await?;
            for (secret, created_at) in &secrets {
                sqlx::query("INSERT INTO webhook_secrets (secret, created_at) VALUES (?, ?)")
                    .bind(resealed(secret, sealing))
                    .bind(created_at)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        // A token rotated since may have leaked, and the Arr holds today's.
        let dated: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('instances')
                            WHERE name = 'webhook_token_at')",
        )
        .fetch_one(&mut *tx)
        .await?;
        for (id, token, token_at) in &tokens {
            let statement = if dated {
                "UPDATE instances SET webhook_token = ?, webhook_token_at = ?
                  WHERE id = ? AND webhook_token IS NOT ?"
            } else {
                "UPDATE instances SET webhook_token = ? WHERE id = ? AND webhook_token IS NOT ?"
            };
            let mut update = sqlx::query(statement).bind(token);
            if dated {
                update = update.bind(token_at);
            }
            update.bind(id).bind(token).execute(&mut *tx).await?;
        }
        // Withdrawn secrets stay withdrawn, and the record travels for a later restore.
        if secrets_withdrawn {
            sqlx::query("DELETE FROM webhook_secrets").execute(&mut *tx).await?;
        }
        // The copy folder stays adopted, and what this database copied there stays its own.
        if !adopted.is_empty() {
            sqlx::raw_sql(COPY_FOLDERS_TABLE).execute(&mut *tx).await?;
            for path in &adopted {
                sqlx::query(
                    "INSERT INTO backup_copy_folders (path) VALUES (?) ON CONFLICT(path) DO NOTHING",
                )
                .bind(path)
                .execute(&mut *tx)
                .await?;
            }
        }
        if !copied.is_empty() {
            sqlx::raw_sql(COPIES_TABLE).execute(&mut *tx).await?;
            for (name, copied_at) in &copied {
                sqlx::query(
                    "INSERT INTO backup_copies (name, copied_at) VALUES (?, ?)
                     ON CONFLICT(name) DO NOTHING",
                )
                .bind(name)
                .bind(copied_at)
                .execute(&mut *tx)
                .await?;
            }
        }
        if !withdrawn.is_empty() {
            sqlx::raw_sql(WITHDRAWN_CREDENTIALS_TABLE).execute(&mut *tx).await?;
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

/// The table as migration `002_release_0_2_0` creates it: a restore writes into it before that
/// migration runs on an older restored database.
pub(crate) const WITHDRAWN_CREDENTIALS_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS withdrawn_credentials (
    name TEXT PRIMARY KEY CHECK(name IN ('master_api_key', 'signing_secret')),
    withdrawn_at TEXT NOT NULL DEFAULT (datetime('now'))
);";

/// The copy tables as migration `003_release_0_3_0` creates them, for the same reason.
pub(crate) const COPY_FOLDERS_TABLE: &str = "CREATE TABLE IF NOT EXISTS backup_copy_folders (
    path TEXT PRIMARY KEY
);";
pub(crate) const COPIES_TABLE: &str = "CREATE TABLE IF NOT EXISTS backup_copies (
    name TEXT PRIMARY KEY,
    copied_at TEXT NOT NULL DEFAULT (datetime('now'))
);";

/// Whether `live` holds `table`: `routarr restore` reads a database no start migrated.
async fn has_table(live: &sqlx::SqlitePool, table: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
    )
    .bind(table)
    .fetch_one(live)
    .await?)
}

/// Whether `live` holds `column` in `table`, for the same reason.
async fn has_column(live: &sqlx::SqlitePool, table: &str, column: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name = ?)")
        .bind(table)
        .bind(column)
        .fetch_one(live)
        .await?)
}

/// `value` as the next start can open it: sealed again when only the live box opens it, and as
/// it is otherwise, readable already or unreadable before the restore.
fn resealed(
    value: &str,
    sealing: Option<(&crate::crypto::SecretBox, &crate::crypto::SecretBox)>,
) -> String {
    let Some((live, next)) = sealing else { return value.to_string() };
    if next.open(value).is_ok() {
        return value.to_string();
    }
    live.open(value).and_then(|opened| next.seal(&opened)).unwrap_or_else(|_| value.to_string())
}

/// The box the master key in place opens a database's secrets with, read as a start reads it
/// and never generated. `None` without a key.
pub(super) async fn secrets_in_place(
    config: &crate::config::Config,
    pool: &sqlx::SqlitePool,
) -> AppResult<Option<crate::crypto::SecretBox>> {
    let Some(key) = key_in_place(config, &config.secret_key_path()) else { return Ok(None) };
    let salt: Option<String> = sqlx::query_scalar("SELECT salt FROM secret_salt LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    Ok(Some(crate::crypto::SecretBox::load(
        Some(&key),
        config.previous_secret_key.as_deref(),
        &config.secret_key_path(),
        salt.as_deref().map(str::as_bytes),
    )?))
}

/// Refuse a restore whose credentials the next start could not open, and answer the box that
/// start opens them with, `None` when no key is found. That start uses `ROUTARR_SECRET_KEY` when
/// set, over any `routarr.key`, or else the key file the archive brings back, or the one in place.
async fn opened_by_the_next_start(
    config: &crate::config::Config,
    staged: &Path,
    localizer: &Localizer,
) -> AppResult<Option<crate::crypto::SecretBox>> {
    use sqlx::{ConnectOptions, Connection};

    let brought_back = staging_path(&config.secret_key_path());
    let key_file = if brought_back.exists() { brought_back } else { config.secret_key_path() };
    let Some(key) = key_in_place(config, &key_file) else {
        return Ok(None);
    };
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(staged)
        .read_only(true)
        .connect()
        .await?;
    // A database with no salt holds only `enc:v1:` seals, which need none.
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
        "ErrorBackupKeyMismatchVariable"
    } else {
        "ErrorBackupKeyMismatchFile"
    };
    match sealed {
        Some(sealed) if next_start.open(&sealed).is_err() => {
            Err(AppError::BadRequest(localizer.translate(refusal, &[])))
        }
        _ => Ok(Some(next_start)),
    }
}

/// The master key a start would use: `ROUTARR_SECRET_KEY`, or else `key_file`. Read without
/// `SecretBox::load`, which writes a new key where it finds none.
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
    let english = Localizer::new(DEFAULT_LANGUAGE);
    sealed::opens(archive, &Passphrase::new(passphrase.to_string()), &english).unwrap_or(false)
}

/// Whether a file is a Routarr database this build can open. Read-only and immutable, so the
/// check writes nothing beside it. A zero-byte file opens as an empty database, and the build
/// that applies a restore may not be the one that staged it.
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
            Some(schema) if crate::db::is_before_0_1_5(&schema) => Err(format!(
                "it is at schema '{schema}', which a Routarr older than 0.1.5 left: 0.1.5 has \
                 to open it first"
            )),
            Some(_) => Ok(()),
        })
    }
    .await;
    connection.close().await.ok();

    verdict.map_err(|e| e.to_string())?
}

/// Keep the master key that opens the database a restore sets aside beside it, as long as that
/// database is kept, when the restore brings another. An earlier restore's copy goes, since the
/// database it opened goes. With no key in place, the copy is the one a stopped start kept.
fn set_the_key_aside(config: &crate::config::Config) -> AppResult<()> {
    let key = config.secret_key_path();
    let kept = with_suffix(&key, PRE_RESTORE);
    let Some(opening) = key_in_place(config, &key) else { return Ok(()) };
    let restored = key_in_place(config, &pending_path(&key));
    if restored.is_none_or(|restored| restored == opening) {
        match std::fs::remove_file(&kept) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(AppError::Config(format!("cannot take {} away: {e}", kept.display())));
            }
            _ => {}
        }
    } else {
        std::fs::rename(&key, &kept)
            .map_err(|e| AppError::Config(format!("cannot set {} aside: {e}", key.display())))?;
        info!("The master key that opens the database restored over is kept at {}", kept.display());
    }
    crate::crypto::sync_parent(&key);
    Ok(())
}

/// Set the database a restore replaces aside, with its write-ahead log, which SQLite would
/// otherwise replay over the restored file. The log is handled by its path: a start stopped
/// between the two renames leaves it beside a database already set aside.
fn set_the_database_aside(database: &Path) -> AppResult<()> {
    let kept = with_suffix(database, PRE_RESTORE);
    let (wal, kept_wal) = (with_suffix(database, "-wal"), with_suffix(&kept, "-wal"));
    if database.exists() {
        // An earlier restore's log, which is not the database set aside now.
        std::fs::remove_file(&kept_wal).ok();
        std::fs::rename(database, &kept).map_err(|e| {
            AppError::Config(format!("cannot set {} aside: {e}", database.display()))
        })?;
        info!("The database restored over is kept at {}", kept.display());
    }
    if wal.exists() && (kept_wal.exists() || std::fs::rename(&wal, &kept_wal).is_err()) {
        std::fs::remove_file(&wal)
            .map_err(|e| AppError::Config(format!("cannot take {} away: {e}", wal.display())))?;
    }
    crate::crypto::sync_parent(database);
    Ok(())
}

/// The database the last restore replaced, kept beside the live one until the owner deletes it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReplacedDatabase {
    /// With its write-ahead log.
    pub size_bytes: u64,
    /// When it was last written, by the server the restore stopped.
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub replaced_at: String,
}

/// The database the last restore set aside, `None` when there is none.
pub fn replaced_database(config: &crate::config::Config) -> Option<ReplacedDatabase> {
    let kept = with_suffix(&config.db_path, PRE_RESTORE);
    let database = std::fs::metadata(&kept).ok()?;
    let wal = std::fs::metadata(with_suffix(&kept, "-wal")).ok();
    let written = [Some(&database), wal.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|meta| meta.modified().ok())
        .max()
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(crate::services::routing::format_timestamp)
        .unwrap_or_default();
    Some(ReplacedDatabase {
        size_bytes: database.len() + wal.map_or(0, |wal| wal.len()),
        replaced_at: written,
    })
}

/// Delete the database the last restore set aside and the master key kept to open it. `false`
/// when there was none.
pub fn delete_replaced_database(config: &crate::config::Config) -> AppResult<bool> {
    let kept = with_suffix(&config.db_path, PRE_RESTORE);
    let found = kept.exists();
    let key = with_suffix(&config.secret_key_path(), PRE_RESTORE);
    // The key last: a deletion stopped halfway leaves no database it opens without it.
    for file in [with_suffix(&kept, "-wal"), with_suffix(&kept, "-shm"), kept, key] {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(AppError::Internal(format!("cannot delete {}: {e}", file.display())));
            }
            _ => {}
        }
    }
    if found {
        info!("The database the last restore replaced was deleted");
    }
    Ok(found)
}

/// Apply a pending restore, from `main` before the pool opens. Each file is renamed into place,
/// the database last, so keys pending alone are an interrupted staging. The database is checked
/// again first, and a rejected set is removed so that no later start refuses it again.
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

    // At WARN: the start that applies it may come long after the staging.
    match pending_restore(config) {
        Some(PendingRestore { name, staged_at }) => {
            warn!(
                "Applying the restore of {name}, staged at {staged_at} UTC, before opening the database"
            )
        }
        None => info!("A restore is pending, applying it before opening the database"),
    }

    // Before anything is restored, or a resumed start reads the archive's key as the one replaced.
    if config.db_path.exists() {
        set_the_key_aside(config)?;
    }
    set_the_database_aside(&config.db_path)?;
    for target in targets.into_iter().rev() {
        let staged = pending_path(&target);
        if !staged.exists() {
            continue;
        }

        if target == config.api_key_path() && target.exists() {
            // Not set aside, and an earlier copy goes: a key withdrawn on purpose must not survive.
            std::fs::remove_file(with_suffix(&target, PRE_RESTORE)).ok();
        }
        std::fs::remove_file(with_suffix(&target, "-shm")).ok();

        std::fs::rename(&staged, &target)
            .map_err(|e| AppError::Config(format!("cannot restore {}: {e}", target.display())))?;
        crate::crypto::sync_parent(&target);
        crate::crypto::restrict_permissions(&target);
        info!("Restored {}", target.display());
    }
    std::fs::remove_file(with_suffix(&config.db_path, NOTE_SUFFIX)).ok();

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restore_creates_the_withdrawals_table_as_the_migration_does() {
        let migration = include_str!("../../../migrations/002_release_0_2_0.sql");
        assert!(migration.contains(WITHDRAWN_CREDENTIALS_TABLE), "the two definitions drifted");
        let migration = include_str!("../../../migrations/003_release_0_3_0.sql");
        for table in [COPY_FOLDERS_TABLE, COPIES_TABLE] {
            assert!(migration.contains(table), "the two definitions drifted: {table}");
        }
    }

    #[test]
    fn only_a_name_routarr_generated_is_accepted() {
        assert!(is_valid_backup_name("routarr-backup-20260823-120000.zip"));
        assert!(is_valid_backup_name("routarr-backup-20260823-120000-app.zip.enc"));
        assert!(!is_valid_backup_name("routarr-backup-20260823-120000.enc"));

        // Taken from a URL, so every way out of the directory has to miss.
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
            schema: "002_release_0_2_0".into(),
            created_at: String::new(),
            includes_master_key: false,
        }
    }

    /// A backup the disk cannot hold is refused before the snapshot, which would fill the disk
    /// the live database writes to.
    #[tokio::test]
    async fn a_backup_the_disk_cannot_hold_is_refused_before_any_copy() {
        let dir = crate::tests::TempDir::new("no-room");
        let mut config = crate::config::Config::for_tests();
        config.set_db_path(dir.join("routarr.db"));
        let pool = crate::db::init_pool(&config).await.unwrap();
        let english = Localizer::new(DEFAULT_LANGUAGE);

        let refused = write_archive(&config, &pool, "", None, None, &english, |_| Ok(4096)).await;

        let Err(AppError::Conflict { reason, message }) = refused else {
            panic!("a backup was taken onto a full disk: {refused:?}")
        };
        assert_eq!(reason, "disk_full");
        assert!(message.contains("4.0 kB"), "{message}");
        let snapshots = std::fs::read_dir(dir.join(".backup-work")).map_or(0, |e| e.count());
        assert_eq!(snapshots, 0, "a snapshot was written for nothing");
        assert!(list_in(&backups_in(&config.data_dir)).is_empty(), "an archive was written");
        pool.close().await;
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

        // A missing snapshot fails the archive once it is opened.
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
        // Under a backup name it would be listed, kept by the retention, and restorable.
        assert_eq!(left, Vec::<String>::new(), "a failed archive was left behind");
    }
}

#[cfg(test)]
mod archive_shape {
    /// A lint over the source, as no test database is big enough to show it: the archive is
    /// deflated off the runtime and the database streamed. It reads code lines only, so a word in
    /// a comment neither satisfies a check nor trips one.
    #[test]
    fn the_database_is_never_read_whole_and_never_deflated_on_the_runtime() {
        const SOURCE: &str = include_str!("mod.rs");
        let code_of = |function: &str| -> Vec<&str> {
            SOURCE
                .split_once(function)
                .expect(function)
                .1
                .split_once("\n}")
                .expect("its closing brace")
                .0
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("//"))
                .collect()
        };

        let build = code_of("fn build_zip(");
        // Guards the parser: matching nothing, every assertion below would pass.
        assert!(build.len() > 30, "only {} lines of build_zip parsed", build.len());
        assert!(
            build.iter().any(|line| line.contains("std::io::copy(")),
            "the database entry must be copied through, not materialised"
        );
        // The one whole read allowed is the two keys', a few bytes each.
        let whole: Vec<&str> = build
            .iter()
            .copied()
            .filter(|line| {
                ["fs::read", "read_to_end(", "read_to_string("].iter().any(|r| line.contains(r))
            })
            .collect();
        assert!(
            whole.len() == 1 && whole[0].contains("source.as_ref().map(std::fs::read)"),
            "a whole read other than the two keys', the snapshot's perhaps: {whole:?}"
        );

        let write = code_of("async fn write_archive(");
        assert!(write.len() > 30, "only {} lines of write_archive parsed", write.len());
        assert!(
            write
                .iter()
                .skip_while(|line| !line.contains("spawn_blocking("))
                .any(|line| line.contains("build_zip(")),
            "deflating a library-sized database must not hold a runtime worker"
        );
    }
}
