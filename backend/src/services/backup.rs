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
//! * **A restore never half-applies.** The bundle is validated and *staged*;
//!   the swap happens at the next startup, before the pool opens. Replacing a
//!   database under an open pool is how a restore destroys what it recovers.
//!   Nothing is marked pending until every file is written and the database
//!   has been opened and checked, and the start checks it again.

use sqlx::AssertSqlSafe;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::error::{AppError, AppResult};
use crate::jobs::JobKind;
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
#[derive(Debug, Clone, Serialize)]
pub struct BackupFile {
    pub name: String,
    pub size_bytes: u64,
    pub created_at: String,
}

/// Entry names inside the archive. Fixed, so a restore knows what to look for.
const DB_ENTRY: &str = "routarr.db";
const MASTER_KEY_ENTRY: &str = "routarr.key";
const API_KEY_ENTRY: &str = "routarr.api_key";
const MANIFEST_ENTRY: &str = "manifest.json";

/// Marker telling the next startup that a restore is pending.
const PENDING_SUFFIX: &str = ".restore-pending";

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
        && name.starts_with("routarr-backup-")
        && name.ends_with(".zip")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..")
}

/// Take a backup now.
pub async fn create(state: &AppState, trigger: &str) -> AppResult<BackupFile> {
    let Some(_lock) = state.jobs.try_lock("backup") else {
        return Err(AppError::Conflict("A backup is already running".into()));
    };

    let job = state.jobs.start(JobKind::Backup, trigger, None, "Taking a backup").await?;
    let outcome = write_archive(state).await;

    match &outcome {
        Ok(file) => job.succeed(&format!("{} ({} bytes)", file.name, file.size_bytes)).await,
        Err(e) => job.fail(&e.to_string()).await,
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

async fn write_archive(state: &AppState) -> AppResult<BackupFile> {
    let dir = backup_dir(state);
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", dir.display())))?;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let name = format!("routarr-backup-{stamp}.zip");
    let path = dir.join(&name);

    // A consistent snapshot of the live database, WAL included. The temporary
    // file sits beside the archive so the copy never lands somewhere the
    // container cannot write.
    let snapshot = dir.join(format!(".{stamp}.db"));
    // A copy of the whole database, sealed credentials included, and twice
    // the space a backup costs: gone on every way out of this function. It
    // moves into the archive task below, which runs to its end whatever
    // happens to the future awaiting it, so a cancelled caller does not
    // remove it under the task that reads it.
    let snapshot_scaffold = Scaffold(vec![snapshot.clone()]);
    vacuum_into(state, &snapshot).await?;

    let schema: String =
        sqlx::query_scalar("SELECT name FROM _migrations ORDER BY id DESC LIMIT 1")
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_else(|| "unknown".to_string());

    let master_key = state.config.secret_key_path();
    let manifest = BackupManifest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema,
        created_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
        includes_master_key: master_key.exists(),
    };

    // Off the runtime, and the whole of it: deflating a library-sized database
    // holds a worker for as long as it takes. Everything the closure needs is
    // taken by value first, so nothing borrows `state` across the boundary.
    let (archive, snapshot_path) = (path.clone(), snapshot.clone());
    let keys = [state.config.secret_key_path(), state.config.api_key_path()];
    let manifest_for_zip = manifest.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _snapshot = snapshot_scaffold;
        build_zip(&archive, &snapshot_path, &manifest_for_zip, &keys)
    })
    .await
    .map_err(|e| AppError::Internal(format!("the archive task failed: {e}")))?;
    result?;

    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    crate::crypto::restrict_permissions(&path);

    info!("Backup written to {} ({size_bytes} bytes)", path.display());
    Ok(BackupFile { name, size_bytes, created_at: manifest.created_at })
}

/// `VACUUM INTO` against the live pool.
async fn vacuum_into(state: &AppState, target: &Path) -> AppResult<()> {
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
    sqlx::query(AssertSqlSafe(format!("VACUUM INTO '{quoted}'"))).execute(&state.pool).await?;

    // `VACUUM INTO` reports success against an in-memory database and writes
    // nothing. One stat call turns that into a message that says so.
    if !target.exists() {
        return Err(AppError::Internal(format!(
            "the database snapshot was not written to {}",
            target.display()
        )));
    }
    // `VACUUM INTO` creates the file under the umask, and it is the whole
    // database — sealed credentials included — until the zip is written and
    // it is removed.
    crate::crypto::restrict_permissions(target);

    Ok(())
}

/// Write the archive, streaming the database rather than holding it.
///
/// Synchronous on purpose — the caller runs it on a blocking thread. Nothing
/// here may read a file whose size follows the library into memory:
/// `api/backup.rs` streams the download for exactly that reason, and an
/// archive is the whole database.
fn build_zip(
    path: &Path,
    snapshot: &Path,
    manifest: &BackupManifest,
    keys: &[PathBuf; 2],
) -> AppResult<()> {
    // Written under a name `list` does not show, and given its own only once
    // whole: a zip is readable as soon as it is finished, and `ZipWriter`
    // finishes it on drop, early return included.
    let partial = partial_path(path);
    std::fs::remove_file(&partial).ok();
    let scaffold = Scaffold(vec![partial.clone()]);

    // The zip carries the master key and the API key in clear: private from
    // its first byte.
    let file = crate::crypto::create_private(&partial, true)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", partial.display())))?;
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
    // credential in it is undecryptable — a restore that silently loses them
    // is worse than one that refuses.
    // These two are a handful of bytes each, so reading them whole is not the
    // same question as the database above.
    for (entry, source) in [MASTER_KEY_ENTRY, API_KEY_ENTRY].iter().zip(keys) {
        if let Ok(bytes) = std::fs::read(source) {
            add(&mut zip, options, entry, &bytes)?;
        }
    }

    let file =
        zip.finish().map_err(|e| AppError::Internal(format!("cannot finish the archive: {e}")))?;
    // On disk before it takes a backup name: renamed first, a power cut can
    // leave an empty file under that name, listed and counted by the retention.
    file.sync_all()
        .map_err(|e| AppError::Internal(format!("cannot write {}: {e}", partial.display())))?;

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

/// Every archive on disk, newest first.
pub fn list(state: &AppState) -> Vec<BackupFile> {
    let dir = backup_dir(state);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut files: Vec<BackupFile> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(is_valid_backup_name))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let created = metadata
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from)
                .map(crate::services::routing::format_timestamp)
                .unwrap_or_default();
            Some(BackupFile {
                name: entry.file_name().to_string_lossy().into_owned(),
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

/// Delete everything past the retention count, oldest first.
pub async fn prune(state: &AppState) -> AppResult<usize> {
    // Read as stored, a value above the ceiling included: lowering a retention
    // count removes archives, so only the operator's own save does — see
    // `settings::Kind::Retention`.
    let keep: usize = state.setting("backup_retention_count", 7usize).await.max(1);
    let files = list(state);
    if files.len() <= keep {
        return Ok(0);
    }

    let dir = backup_dir(state);
    let mut removed = 0;
    for file in files.into_iter().skip(keep) {
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
pub async fn stage_restore(state: &AppState, name: &str) -> AppResult<BackupManifest> {
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
        stage(&state, &name).await
    })
    .await
    .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))?
}

async fn stage(state: &AppState, name: &str) -> AppResult<BackupManifest> {
    let archive = backup_dir(state).join(name);
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
        (DB_ENTRY, state.config.db_path.clone()),
        (MASTER_KEY_ENTRY, state.config.secret_key_path()),
        (API_KEY_ENTRY, state.config.api_key_path()),
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
    if let Err(reason) = check_database(&staging_path(&state.config.db_path)).await {
        return Err(AppError::BadRequest(format!(
            "the archive's database cannot be restored: {reason}"
        )));
    }

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
        if let Err(e) = std::fs::rename(&staging, pending_path(target)) {
            discard_pending(&paths);
            return Err(AppError::Internal(format!("cannot stage {}: {e}", target.display())));
        }
        sync_parent(target);
    }
    staged.keep();

    info!("Backup {name} staged; it is applied on the next start");
    Ok(manifest)
}

/// Copy the three known entries beside their targets, under the staging suffix.
///
/// Synchronous on purpose — the caller runs it on a blocking thread. The entry
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
                    "Backup {name} carries no '{entry}'; the restore leaves the current one in place"
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

/// Make a rename durable before the next one.
///
/// The database is renamed last so that a set interrupted before it is
/// rejected whole, and that order survives a power cut only once each rename
/// has reached the disk. Best effort: a filesystem that cannot open a
/// directory to sync it still renames.
fn sync_parent(path: &Path) {
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir).and_then(|dir| dir.sync_all()).ok();
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
    let Ok(entries) = std::fs::read_dir(backups_in(&config.data_dir)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_str().is_some_and(is_leftover) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// A name only an interrupted run leaves: a partial archive or a snapshot.
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
/// leaves either the old file or the new one — never half of either. The
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

    info!("A restore is pending; applying it before opening the database");

    for target in targets.into_iter().rev() {
        let staged = pending_path(&target);
        if !staged.exists() {
            continue;
        }

        // The WAL and shared-memory files belong to the *old* database. Left in
        // place next to a restored file, SQLite would try to replay them over
        // it and refuse to open, or worse, succeed.
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = target.as_os_str().to_os_string();
            sidecar.push(suffix);
            std::fs::remove_file(PathBuf::from(sidecar)).ok();
        }

        std::fs::rename(&staged, &target)
            .map_err(|e| AppError::Config(format!("cannot restore {}: {e}", target.display())))?;
        sync_parent(&target);
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
        let dir = std::env::temp_dir().join(format!("routarr-zip-exists-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("routarr-backup-20260101-000000.zip");
        std::fs::write(&archive, b"an earlier archive").unwrap();
        let snapshot = dir.join(".20260101-000000.db");
        std::fs::write(&snapshot, b"a database").unwrap();

        let outcome = build_zip(
            &archive,
            &snapshot,
            &manifest(),
            &[dir.join("routarr.key"), dir.join("routarr.api_key")],
        );

        assert!(outcome.is_err(), "an archive was written over another");
        assert_eq!(std::fs::read(&archive).unwrap(), b"an earlier archive");
        assert!(!partial_path(&archive).exists(), "the refused archive was left behind");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_archive_that_fails_midway_is_not_left_under_a_backup_name() {
        let dir = std::env::temp_dir().join(format!("routarr-zip-fails-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        // The snapshot is gone by the time the database entry is copied, so
        // the archive fails after it was opened.
        let outcome = build_zip(
            &dir.join("routarr-backup-20260101-000000.zip"),
            &dir.join(".missing.db"),
            &manifest(),
            &[dir.join("routarr.key"), dir.join("routarr.api_key")],
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

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod archive_shape {
    /// The archive is written off the runtime, and the database is streamed.
    ///
    /// Neither property is visible to a behavioural test: a backup of the
    /// two-page database a test seeds blocks for microseconds and fits in a
    /// cache line, so the suite is green whichever way this is written. What
    /// makes it matter is the machine this runs on — the commentary through
    /// this crate designs for a two-core NAS — and the cadence, since
    /// `backup_enabled` defaults to true and the scheduler takes one a day.
    ///
    /// `api/backup.rs` already streams the *download* and says why: "reading it
    /// into memory first doubles the process's footprint". The write path is
    /// the same file, and it is the one nobody asks for.
    #[test]
    fn the_database_is_never_read_whole_and_never_deflated_on_the_runtime() {
        const SOURCE: &str = include_str!("backup.rs");

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
