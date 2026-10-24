//! A copy of each archive in `ROUTARR_BACKUP_COPY_DIR`, a folder mounted from another disk, so
//! the loss of the disk holding the database leaves the archives. Apart from the backup on
//! purpose: a share that sleeps or hangs holds back the copy alone, never a backup or a start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{Scaffold, list_in, partial_path, sealed};
use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::localization::Localizer;
use crate::services::notify::{self, Event};
use crate::state::AppState;

/// Left in the copy folder by its first copy. Once a folder is adopted, one holding no marker is
/// the empty mount point of a share not mounted, and copying there would fill the local disk.
pub const MARKER: &str = ".routarr-backup-copy";

/// How long a copy may write nothing before it is reported as failed: a share slow to answer
/// keeps writing, one that hangs does not. The thread copying cannot be stopped, so it goes on,
/// holding the copy lock until the folder answers.
const STALL: Duration =
    if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(2 * 60) };

/// How long the Backups card waits for the folder when told to start the copy there.
const ANSWER: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(30) };

/// Held by one copy, or one removal from the copy folder, at a time.
const LOCK: &str = "backup_copy";

/// What a copy did, kept as its task's result for the Backups card.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyReport {
    /// The archives copied this time.
    pub copied: Vec<String>,
    /// The archives in the copy folder once done, newest first.
    pub present: Vec<String>,
    /// The copy folder sits on the data folder's disk, so its loss takes both.
    #[serde(default)]
    pub same_disk: bool,
}

/// Why a copy wrote nothing more.
enum Refusal {
    /// No folder at the path.
    Missing,
    /// Adopted, and its marker gone.
    Unmounted,
    /// Marked by a copy this database never recorded: another database's folder.
    Foreign,
    /// The folder did not answer in time.
    TimedOut,
    /// What the folder answered.
    Failed(String),
}

impl Refusal {
    fn detail(self, folder: &str) -> Detail {
        let key = match &self {
            Refusal::Missing => "JobDetailBackupCopyMissing",
            Refusal::Unmounted => "JobDetailBackupCopyUnmounted",
            Refusal::Foreign => "JobDetailBackupCopyForeign",
            Refusal::TimedOut => "JobDetailBackupCopyTimedOut",
            Refusal::Failed(_) => "JobDetailBackupCopyFailed",
        };
        let detail = Detail::new(key).with("folder", folder);
        match self {
            Refusal::Failed(reason) => detail.with("reason", reason),
            Refusal::Missing | Refusal::Unmounted | Refusal::Foreign | Refusal::TimedOut => detail,
        }
    }
}

fn failed(e: std::io::Error) -> Refusal {
    Refusal::Failed(e.to_string())
}

/// Bring the copy folder level with the backup folder on a task of its own: each archive the
/// copy lacks is copied, a copy of an archive sealed or opened since is replaced, and the
/// retention count prunes the copy as it prunes the backups. Nothing without a copy folder.
pub fn in_background(state: &AppState, by: Attribution) {
    let Some(folder) = state.config.backup_copy_dir.clone() else { return };
    let state = state.clone();
    // Its outcome is the task's, logged and notified there.
    tokio::spawn(async move { run(&state, folder, &by).await.ok() });
}

/// Copy into `ROUTARR_BACKUP_COPY_DIR` now, as a task the caller follows: its report, or why
/// nothing was copied.
pub async fn now(state: &AppState, by: Attribution) -> AppResult<CopyReport> {
    let Some(folder) = state.config.backup_copy_dir.clone() else {
        let localizer = state.localizer().await;
        return Err(AppError::BadRequest(localizer.translate("ErrorBackupCopyUnset", &[])));
    };
    run(state, folder, &by).await
}

/// One copy, its outcome settled in its task and, when it failed, notified.
async fn run(state: &AppState, folder: PathBuf, by: &Attribution) -> AppResult<CopyReport> {
    // Behind the copy before it, whose work this one only checks.
    let lock = lock_while_writing(state).await;
    let shown = folder.display().to_string();
    let detail = Detail::new("JobDetailCopyingBackups").with("folder", &shown);
    let mut job = match state.jobs.start(JobKind::BackupCopy, by, None, detail).await {
        Ok(job) => job,
        Err(e) => {
            warn!("The backups were not copied: {e}");
            return Err(e);
        }
    };
    let refused = async |refused: Detail| {
        let message = refused.said(&state.localizer().await);
        said_failed(state, &shown, refused);
        Err(AppError::Conflict { reason: "backup_copy_unreachable", message })
    };
    let Some(lock) = lock else {
        let busy = Detail::new("JobDetailBackupCopyBusy").with("folder", &shown);
        job.fail_with(busy.clone()).await;
        return refused(busy).await;
    };
    let read = (adopted(state, &folder).await, super::kept(state).await, ours(state).await);
    let (adopted, keep, ours) = match read {
        (Ok(adopted), Ok(keep), Ok(ours)) => (adopted, keep, ours),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            job.fail(&e).await;
            return Err(e);
        }
    };
    let pass = Pass {
        source: super::backup_dir(state),
        data: state.config.data_dir.clone(),
        folder: folder.clone(),
        names: super::list(state).into_iter().map(|file| file.name).collect(),
        adopted,
        keep,
        ours,
    };
    let copied = Arc::new(Mutex::new(Vec::new()));
    let (named, written) = (Arc::clone(&copied), Arc::clone(&state.backup_copy_written));
    let copying = tokio::task::spawn_blocking(move || {
        let _lock = lock;
        bring_level(pass, &named, &written)
    });
    let outcome = watched(copying, &state.backup_copy_written, STALL).await.and_then(|o| o);
    // Whatever the outcome: an archive copied and not recorded would never be pruned.
    let written = copied.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    if let Err(e) = record_copied(state, &written).await {
        warn!("The archives copied to {shown} were not recorded: {e}");
    }
    match outcome {
        Ok(report) => {
            if !adopted && let Err(e) = record_adopted(state, &folder).await {
                warn!("{shown} was not recorded as the copy folder: {e}");
            }
            if let Err(e) = forget_gone(state, &report.present).await {
                warn!("The copies gone from {shown} were not forgotten: {e}");
            }
            info!("Backups copied to {shown}: {}", report.copied.len());
            let done = Detail::new("JobDetailBackupsCopied")
                .with("folder", &shown)
                .with("count", report.copied.len());
            job.report(&report);
            job.succeed(done).await;
            Ok(report)
        }
        Err(refusal) => {
            let detail = refusal.detail(&shown);
            job.fail_with(detail.clone()).await;
            refused(detail).await
        }
    }
}

/// The outcome of `work`, or `TimedOut` once `written` stood still for `stall`.
async fn watched<T>(
    mut work: tokio::task::JoinHandle<T>,
    written: &AtomicU64,
    stall: Duration,
) -> Result<T, Refusal> {
    let mut seen = written.load(Ordering::SeqCst);
    loop {
        match tokio::time::timeout(stall, &mut work).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(e)) => return Err(Refusal::Failed(e.to_string())),
            Err(_) => {
                let now = written.load(Ordering::SeqCst);
                if now == seen {
                    return Err(Refusal::TimedOut);
                }
                seen = now;
            }
        }
    }
}

/// The copy lock, waited for while the copy holding it writes. `None` once that copy wrote
/// nothing for `STALL`, a share that hangs.
async fn lock_while_writing(state: &AppState) -> Option<crate::jobs::JobLock> {
    let mut seen = state.backup_copy_written.load(Ordering::SeqCst);
    loop {
        if let Some(lock) = state.jobs.lock_within(LOCK, STALL).await {
            return Some(lock);
        }
        let now = state.backup_copy_written.load(Ordering::SeqCst);
        if now == seen {
            return None;
        }
        seen = now;
    }
}

/// Every failure is told, a copy running unattended whoever started it.
fn said_failed(state: &AppState, folder: &str, reason: Detail) {
    notify::send_later(state, Event::BackupCopyFailed { folder: folder.to_string(), reason });
}

/// What a pass works from, read before it starts.
struct Pass {
    /// The backup folder.
    source: PathBuf,
    data: PathBuf,
    folder: PathBuf,
    /// The archives of the backup folder.
    names: Vec<String>,
    adopted: bool,
    keep: usize,
    /// The archives this database copied, the only ones it may remove from the folder.
    ours: HashSet<String>,
}

/// What `run` does, on a blocking thread: any call here may wait on the share. Each archive
/// copied lands in `copied` and in `ours` at once, and its bytes in `written` as they go.
fn bring_level(
    pass: Pass,
    copied: &Mutex<Vec<String>>,
    written: &AtomicU64,
) -> Result<CopyReport, Refusal> {
    let Pass { source, data, folder, names, adopted, keep, mut ours } = pass;
    let folder = folder.as_path();
    if !folder.is_dir() {
        return Err(Refusal::Missing);
    }
    if !folder.join(MARKER).exists() {
        if adopted {
            return Err(Refusal::Unmounted);
        }
        mark(folder).map_err(failed)?;
    } else if !adopted {
        return Err(Refusal::Foreign);
    }
    super::sweep_in(folder);

    let held: HashSet<String> = list_in(folder).into_iter().map(|file| file.name).collect();
    let mut report = CopyReport { same_disk: same_disk(folder, &data), ..CopyReport::default() };
    for name in names.iter().filter(|name| !held.contains(*name)) {
        if copy_one(&source.join(name), &folder.join(name), written).map_err(failed)? {
            copied.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(name.clone());
            ours.insert(name.clone());
            report.copied.push(name.clone());
        }
    }
    // Sealed or opened since its copy, an archive goes by another name, and its copy with it.
    let plain = |name: &str| name.strip_suffix(sealed::SUFFIX).unwrap_or(name).to_string();
    let current: HashSet<&str> = names.iter().map(String::as_str).collect();
    let renamed: HashSet<String> = names.iter().map(|name| plain(name)).collect();
    for stale in held.iter().filter(|name| !current.contains(name.as_str()) && ours.contains(*name))
    {
        if renamed.contains(&plain(stale)) {
            std::fs::remove_file(folder.join(stale)).map_err(failed)?;
        }
    }
    super::prune_in(folder, keep, Some(&ours));

    report.present = list_in(folder).into_iter().map(|file| file.name).collect();
    Ok(report)
}

/// Whether `a` and `b` sit on one file system, which a bind mount reports as its host's. False
/// where the system cannot tell.
fn same_disk(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(a), Ok(b)) if a.dev() == b.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// Copy `from` to `to` as an archive is written: private, synced, and named only once whole,
/// each byte counted in `written`. `false` when `from` went meanwhile, pruned or deleted.
fn copy_one(from: &Path, to: &Path, written: &AtomicU64) -> std::io::Result<bool> {
    let mut input = match std::fs::File::open(from) {
        Ok(input) => input,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let partial = partial_path(to);
    std::fs::remove_file(&partial).ok();
    let scaffold = Scaffold(vec![partial.clone()]);
    let mut output = Counted { file: crate::crypto::create_private(&partial, true)?, written };
    std::io::copy(&mut input, &mut output)?;
    output.file.sync_all()?;
    // Never over a copy that already exists, which `rename` would replace.
    if to.exists() {
        let exists = format!("{} exists already", to.display());
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, exists));
    }
    std::fs::rename(&partial, to)?;
    scaffold.keep();
    crate::crypto::sync_parent(to);
    Ok(true)
}

/// A file whose every write is counted as it lands.
struct Counted<'a> {
    file: std::fs::File,
    written: &'a AtomicU64,
}

impl std::io::Write for Counted<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let landed = self.file.write(bytes)?;
        self.written.fetch_add(landed as u64, Ordering::SeqCst);
        Ok(landed)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn mark(folder: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let marker = folder.join(MARKER);
    let mut file = crate::crypto::create_private(&marker, true)?;
    file.write_all(b"Routarr copies its backups into this folder. Keep this file here.\n")?;
    file.sync_all()?;
    crate::crypto::sync_parent(&marker);
    Ok(())
}

/// The folder as `backup_copy_folders` records it, whichever way the variable spells it. Never
/// `crate::paths::key`, which folds case and would make two local folders one.
fn folder_key(folder: &Path) -> String {
    crate::config::lexical(folder).to_string_lossy().into_owned()
}

/// Read through the same key, since a row may keep the spelling of the variable that wrote it.
async fn adopted(state: &AppState, folder: &Path) -> AppResult<bool> {
    let recorded: Vec<String> =
        sqlx::query_scalar("SELECT path FROM backup_copy_folders").fetch_all(&state.pool).await?;
    let key = folder_key(folder);
    Ok(recorded.iter().any(|path| folder_key(Path::new(path)) == key))
}

async fn record_adopted(state: &AppState, folder: &Path) -> AppResult<()> {
    sqlx::query("INSERT INTO backup_copy_folders (path) VALUES (?) ON CONFLICT(path) DO NOTHING")
        .bind(folder_key(folder))
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// The archives this database copied into the copy folder.
async fn ours(state: &AppState) -> AppResult<HashSet<String>> {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM backup_copies").fetch_all(&state.pool).await?;
    Ok(names.into_iter().collect())
}

async fn record_copied(state: &AppState, names: &[String]) -> AppResult<()> {
    for name in names {
        sqlx::query("INSERT INTO backup_copies (name) VALUES (?) ON CONFLICT(name) DO NOTHING")
            .bind(name)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}

/// Forget the copies no longer in the folder, `present` as a whole pass listed it.
async fn forget_gone(state: &AppState, present: &[String]) -> AppResult<()> {
    let present: HashSet<&str> = present.iter().map(String::as_str).collect();
    for name in ours(state).await? {
        if !present.contains(name.as_str()) {
            sqlx::query("DELETE FROM backup_copies WHERE name = ?")
                .bind(&name)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(())
}

/// Take the copy folder as it is now, its marker written and the folder recorded, for the copy
/// [`now`] makes: for a folder changed on purpose, which a share left unmounted is not.
pub async fn adopt(state: &AppState) -> AppResult<()> {
    let localizer = state.localizer().await;
    let Some(folder) = state.config.backup_copy_dir.clone() else {
        return Err(AppError::BadRequest(localizer.translate("ErrorBackupCopyUnset", &[])));
    };
    let marking = folder.clone();
    let marked = tokio::task::spawn_blocking(move || {
        if !marking.is_dir() {
            return Err(Refusal::Missing);
        }
        if marking.join(MARKER).exists() {
            return Ok(());
        }
        mark(&marking).map_err(failed)
    });
    let refusal = match tokio::time::timeout(ANSWER, marked).await {
        Ok(Ok(Ok(()))) => None,
        Ok(Ok(Err(refusal))) => Some(refusal),
        Ok(Err(e)) => Some(Refusal::Failed(e.to_string())),
        Err(_) => Some(Refusal::TimedOut),
    };
    if let Some(refusal) = refusal {
        let shown = folder.display().to_string();
        let message = match refusal {
            Refusal::TimedOut => {
                localizer.translate("ErrorBackupCopyNoAnswer", &[("folder", &shown)])
            }
            refusal => refusal.detail(&shown).said(&localizer),
        };
        return Err(AppError::Conflict { reason: "backup_copy_unreachable", message });
    }
    record_adopted(state, &folder).await
}

/// Remove `name`'s copy behind any copy running, its outcome logged alone: a share that does not
/// answer keeps the copy until the retention count prunes it.
pub fn forget_in_background(state: &AppState, name: &str) {
    let Some(folder) = state.config.backup_copy_dir.clone() else { return };
    let (state, copy) = (state.clone(), folder.join(name));
    tokio::spawn(async move {
        let Some(lock) = lock_while_writing(&state).await else {
            return warn!("{} stays, behind a copy that does not end", copy.display());
        };
        let shown = copy.display().to_string();
        let removing = tokio::task::spawn_blocking(move || {
            let _lock = lock;
            match std::fs::remove_file(&copy) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        });
        match tokio::time::timeout(STALL, removing).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => warn!("{shown} could not be deleted: {e}"),
            Ok(Err(e)) => warn!("{shown} could not be deleted: {e}"),
            Err(_) => warn!("{shown} stays: its folder did not answer"),
        }
    });
}

/// Where the copies go and how the last copy went, for the Backups card. Read from the tasks,
/// never from the folder, which may be a share asleep.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackupCopy {
    /// The folder `ROUTARR_BACKUP_COPY_DIR` names.
    pub folder: String,
    /// `never` before the first copy, then the last one's: `running`, `copied`, `failed`,
    /// `unmounted`, a folder without its first copy's marker, or `foreign`, one another Routarr
    /// database copied into. Neither gains or loses a file until the owner takes it as it is.
    pub state: &'static str,
    /// What the last copy said, in the interface language. Null before the first.
    pub detail: Option<String>,
    /// When a copy last ended well. Null before the first.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub copied_at: Option<String>,
    /// The last good copy found the folder on the disk of the data folder, whose loss it would
    /// not survive. False where the system cannot tell.
    pub same_disk: bool,
}

/// The newest copy task, as the Backups card reads it.
#[derive(sqlx::FromRow)]
struct LastCopy {
    status: String,
    detail: Option<String>,
    detail_key: Option<String>,
    detail_params: Option<String>,
}

/// The copy folder's state and the archives its last good copy left there, `None` without one.
pub async fn status(
    state: &AppState,
    localizer: &Localizer,
) -> AppResult<Option<(BackupCopy, HashSet<String>)>> {
    let Some(folder) = &state.config.backup_copy_dir else { return Ok(None) };
    let last: Option<LastCopy> = sqlx::query_as(
        "SELECT status, detail, detail_key, detail_params FROM jobs WHERE kind = 'backup_copy'
          ORDER BY started_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    let good: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT finished_at, result FROM jobs WHERE kind = 'backup_copy' AND status = 'success'
          ORDER BY started_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    let (copied_at, result) = good.unwrap_or_default();
    let report = result
        .and_then(|result| serde_json::from_str::<CopyReport>(&result).ok())
        .unwrap_or_default();
    let (present, same_disk) = (report.present.into_iter().collect(), report.same_disk);
    let (state, detail) = match last {
        None => ("never", None),
        Some(LastCopy { status, detail: text, detail_key: key, detail_params: params }) => {
            let shown = match (status.as_str(), key.as_deref()) {
                ("running", _) => "running",
                ("success", _) => "copied",
                (_, Some("JobDetailBackupCopyUnmounted")) => "unmounted",
                (_, Some("JobDetailBackupCopyForeign")) => "foreign",
                _ => "failed",
            };
            let said = crate::jobs::registry::render_detail(
                localizer,
                key.as_deref(),
                params.as_deref(),
                text,
            );
            (shown, said)
        }
    };
    let folder = folder.display().to_string();
    let copy = BackupCopy { folder, state, detail, copied_at, same_disk };
    Ok(Some((copy, present)))
}

/// When a copy last ended well, for the metrics.
pub async fn last_copied_at(state: &AppState) -> Option<chrono::DateTime<chrono::Utc>> {
    state.config.backup_copy_dir.as_ref()?;
    let finished: Option<String> = sqlx::query_scalar(
        "SELECT MAX(finished_at) FROM jobs WHERE kind = 'backup_copy' AND status = 'success'",
    )
    .fetch_one(&state.pool)
    .await
    .ok()?;
    crate::services::routing::parse_timestamp(&finished?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A share that answers slowly is no share that hangs: a pass longer than the stall that
    /// keeps writing ends as it ends.
    #[tokio::test]
    async fn a_slow_copy_that_progresses_is_not_reported_failed() {
        let written = Arc::new(AtomicU64::new(0));
        let writing = Arc::clone(&written);
        let work = tokio::task::spawn_blocking(move || {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_secs(1));
                writing.fetch_add(8192, Ordering::SeqCst);
            }
        });

        let outcome = watched(work, &written, Duration::from_secs(2)).await;

        assert!(outcome.is_ok(), "a copy still writing was reported failed");
    }

    #[tokio::test]
    async fn a_copy_that_stops_progressing_is_reported_timed_out() {
        let written = AtomicU64::new(0);
        let work = tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(1)));

        let outcome = watched(work, &written, Duration::from_millis(300)).await;

        assert!(matches!(outcome, Err(Refusal::TimedOut)), "a copy that hangs was not reported");
    }
}
