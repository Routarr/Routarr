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

use sqlx::AssertSqlSafe;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

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

/// Where archives live: a `backups` directory beside the database.
pub fn backup_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("backups")
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
    // holds a worker for as long as it takes, and `accounts.rs` already states
    // the rule for a hash that costs 355 ms. Everything the closure needs is
    // taken by value first, so nothing borrows `state` across the boundary.
    let (archive, snapshot_path) = (path.clone(), snapshot.clone());
    let keys = [state.config.secret_key_path(), state.config.api_key_path()];
    let manifest_for_zip = manifest.clone();
    let result = tokio::task::spawn_blocking(move || {
        let outcome = build_zip(&archive, &snapshot_path, &manifest_for_zip, &keys);
        // The snapshot is scaffolding; leaving it behind would double the space
        // a backup costs and look like a stray database. Removed inside the
        // task so a cancelled caller does not leave it: a blocking task runs to
        // the end whatever happens to the future awaiting it.
        std::fs::remove_file(&snapshot_path).ok();
        outcome
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
    // The zip carries the master key and the API key in clear: private from
    // its first byte, and never written over an archive that already exists.
    let file = crate::crypto::create_private(path, true)
        .map_err(|e| AppError::Internal(format!("cannot create {}: {e}", path.display())))?;
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

    // Copied through, never materialised: `std::fs::read` here held the entire
    // database in a `Vec<u8>` and then handed it to the deflater, which is two
    // copies of a file that grows with somebody's library.
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

    zip.finish().map_err(|e| AppError::Internal(format!("cannot finish the archive: {e}")))?;
    Ok(())
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
/// the files are written beside their targets and a marker tells `main` to move
/// them into place before it opens anything.
pub async fn stage_restore(state: &AppState, name: &str) -> AppResult<BackupManifest> {
    if !is_valid_backup_name(name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }

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
    let named = name.to_string();
    tokio::task::spawn_blocking(move || extract_staged(&archive, &targets, &named))
        .await
        .map_err(|e| AppError::Internal(format!("the restore task failed: {e}")))??;

    info!("Backup {name} staged; it is applied on the next start");
    Ok(manifest)
}

/// Copy the three known entries beside their targets.
///
/// Synchronous on purpose — the caller runs it on a blocking thread. The entry
/// names are literals and the destinations are computed here, so no name out of
/// the archive ever reaches a path.
fn extract_staged(archive: &Path, targets: &[(&str, PathBuf); 3], name: &str) -> AppResult<()> {
    let file =
        std::fs::File::open(archive).map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::BadRequest(format!("not a readable archive: {e}")))?;

    for (entry, target) in targets {
        let Ok(mut source) = zip.by_name(entry) else {
            // Said out loud. An archive without the master key restores a
            // database whose every sealed credential this installation cannot
            // open, and the only other trace is `reseal_secrets` reporting them
            // one by one at the next start. The manifest carries the same
            // answer to the caller, which is what the interface warns on.
            warn!(
                "Backup {name} carries no '{entry}'; the restore leaves the current one in place"
            );
            continue;
        };
        let staged = staged_path(target);
        let mut out = crate::crypto::create_private(&staged, false)
            .map_err(|e| AppError::Internal(format!("cannot stage {}: {e}", staged.display())))?;
        std::io::copy(&mut source, &mut out)
            .map_err(|e| AppError::Internal(format!("cannot stage {entry}: {e}")))?;
    }

    Ok(())
}

fn staged_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(PENDING_SUFFIX);
    PathBuf::from(name)
}

/// Apply a staged restore, if one is pending.
///
/// Called from `main` **before** the pool is opened. Each file is moved into
/// place with a rename, which is atomic within a filesystem, so an interruption
/// leaves either the old file or the new one — never half of either.
pub fn apply_pending_restore(config: &crate::config::Config) -> AppResult<bool> {
    let targets = [config.db_path.clone(), config.secret_key_path(), config.api_key_path()];

    if !targets.iter().any(|target| staged_path(target).exists()) {
        return Ok(false);
    }

    info!("A restore is pending; applying it before opening the database");

    for target in targets {
        let staged = staged_path(&target);
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
