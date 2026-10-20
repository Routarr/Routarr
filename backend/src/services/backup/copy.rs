//! A copy of each archive in `ROUTARR_BACKUP_COPY_DIR`, a folder mounted from another disk, so
//! the loss of the disk holding the database leaves the archives. Apart from the backup on
//! purpose: a share that sleeps or hangs holds back the copy alone, never a backup or a start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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

/// How long a copy may run before it is reported as failed. The thread copying cannot be
/// stopped, so it goes on, holding the copy lock until the folder answers.
const TIMEOUT: Duration =
    if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(10 * 60) };

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
}

/// Why a copy wrote nothing more.
enum Refusal {
    /// No folder at the path.
    Missing,
    /// Adopted, and its marker gone.
    Unmounted,
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
            Refusal::TimedOut => "JobDetailBackupCopyTimedOut",
            Refusal::Failed(_) => "JobDetailBackupCopyFailed",
        };
        let detail = Detail::new(key).with("folder", folder);
        match self {
            Refusal::Failed(reason) => detail.with("reason", reason),
            Refusal::Missing | Refusal::Unmounted | Refusal::TimedOut => detail,
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
    tokio::spawn(async move { run(&state, folder, &by).await });
}

async fn run(state: &AppState, folder: PathBuf, by: &Attribution) {
    // Behind the copy before it, whose work this one only checks: a wait past the timeout is a
    // copy that hangs.
    let lock = state.jobs.lock_within(LOCK, TIMEOUT).await;
    let shown = folder.display().to_string();
    let detail = Detail::new("JobDetailCopyingBackups").with("folder", &shown);
    let mut job = match state.jobs.start(JobKind::BackupCopy, by, None, detail).await {
        Ok(job) => job,
        Err(e) => return warn!("The backups were not copied: {e}"),
    };
    let Some(lock) = lock else {
        let busy = Detail::new("JobDetailBackupCopyBusy").with("folder", &shown);
        job.fail_with(busy.clone()).await;
        return said_failed(state, &shown, busy);
    };
    let (adopted, keep) = match (adopted(state, &folder).await, super::kept(state).await) {
        (Ok(adopted), Ok(keep)) => (adopted, keep),
        (Err(e), _) | (_, Err(e)) => return job.fail(&e).await,
    };
    let source = super::backup_dir(state);
    let names: Vec<String> = super::list(state).into_iter().map(|file| file.name).collect();
    let target = folder.clone();
    let copying = tokio::task::spawn_blocking(move || {
        let _lock = lock;
        bring_level(&source, &target, &names, adopted, keep)
    });
    let outcome = match tokio::time::timeout(TIMEOUT, copying).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => Err(Refusal::Failed(e.to_string())),
        Err(_) => Err(Refusal::TimedOut),
    };
    match outcome {
        Ok(report) => {
            if !adopted && let Err(e) = record_adopted(state, &folder).await {
                warn!("{shown} was not recorded as the copy folder: {e}");
            }
            info!("Backups copied to {shown}: {}", report.copied.len());
            let done = Detail::new("JobDetailBackupsCopied")
                .with("folder", &shown)
                .with("count", report.copied.len());
            job.report(&report);
            job.succeed(done).await;
        }
        Err(refusal) => {
            let refused = refusal.detail(&shown);
            job.fail_with(refused.clone()).await;
            said_failed(state, &shown, refused);
        }
    }
}

/// Every failure is told, a copy running unattended whoever started it.
fn said_failed(state: &AppState, folder: &str, reason: Detail) {
    notify::send_later(state, Event::BackupCopyFailed { folder: folder.to_string(), reason });
}

/// What `run` does, on a blocking thread: any call here may wait on the share.
fn bring_level(
    source: &Path,
    folder: &Path,
    names: &[String],
    adopted: bool,
    keep: usize,
) -> Result<CopyReport, Refusal> {
    if !folder.is_dir() {
        return Err(Refusal::Missing);
    }
    if !folder.join(MARKER).exists() {
        if adopted {
            return Err(Refusal::Unmounted);
        }
        mark(folder).map_err(failed)?;
    }
    super::sweep_in(folder);

    let held: HashSet<String> = list_in(folder).into_iter().map(|file| file.name).collect();
    let mut report = CopyReport::default();
    for name in names.iter().filter(|name| !held.contains(*name)) {
        if copy_one(&source.join(name), &folder.join(name)).map_err(failed)? {
            report.copied.push(name.clone());
        }
    }
    // Sealed or opened since its copy, an archive goes by another name, and its copy with it.
    let plain = |name: &str| name.strip_suffix(sealed::SUFFIX).unwrap_or(name).to_string();
    let current: HashSet<&str> = names.iter().map(String::as_str).collect();
    let renamed: HashSet<String> = names.iter().map(|name| plain(name)).collect();
    for stale in held.iter().filter(|name| !current.contains(name.as_str())) {
        if renamed.contains(&plain(stale)) {
            std::fs::remove_file(folder.join(stale)).map_err(failed)?;
        }
    }
    super::prune_in(folder, keep);

    report.present = list_in(folder).into_iter().map(|file| file.name).collect();
    Ok(report)
}

/// Copy `from` to `to` as an archive is written: private, synced, and named only once whole.
/// `false` when `from` went meanwhile, pruned or deleted.
fn copy_one(from: &Path, to: &Path) -> std::io::Result<bool> {
    let mut input = match std::fs::File::open(from) {
        Ok(input) => input,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let partial = partial_path(to);
    std::fs::remove_file(&partial).ok();
    let scaffold = Scaffold(vec![partial.clone()]);
    let mut output = crate::crypto::create_private(&partial, true)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
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

fn mark(folder: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let marker = folder.join(MARKER);
    let mut file = crate::crypto::create_private(&marker, true)?;
    file.write_all(b"Routarr copies its backups into this folder. Keep this file here.\n")?;
    file.sync_all()?;
    crate::crypto::sync_parent(&marker);
    Ok(())
}

async fn adopted(state: &AppState, folder: &Path) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_copy_folders WHERE path = ?)")
        .bind(folder.to_string_lossy())
        .fetch_one(&state.pool)
        .await?)
}

async fn record_adopted(state: &AppState, folder: &Path) -> AppResult<()> {
    sqlx::query("INSERT INTO backup_copy_folders (path) VALUES (?) ON CONFLICT(path) DO NOTHING")
        .bind(folder.to_string_lossy())
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Take the copy folder as it is now, its marker written and the folder recorded, then copy:
/// for a folder changed on purpose, which a share left unmounted is not.
pub async fn adopt(state: &AppState, by: Attribution) -> AppResult<()> {
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
    record_adopted(state, &folder).await?;
    in_background(state, by);
    Ok(())
}

/// Remove `name`'s copy behind any copy running, its outcome logged alone: a share that does not
/// answer keeps the copy until the retention count prunes it.
pub fn forget_in_background(state: &AppState, name: &str) {
    let Some(folder) = state.config.backup_copy_dir.clone() else { return };
    let (jobs, copy) = (state.jobs.clone(), folder.join(name));
    tokio::spawn(async move {
        let Some(lock) = jobs.lock_within(LOCK, TIMEOUT).await else {
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
        match tokio::time::timeout(TIMEOUT, removing).await {
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
    /// `never` before the first copy, then the last one's: `running`, `copied`, `failed`, or
    /// `unmounted`, a folder without the marker its first copy left, which the owner may take as
    /// it is.
    pub state: &'static str,
    /// What the last copy said, in the interface language. Null before the first.
    pub detail: Option<String>,
    /// When a copy last ended well. Null before the first.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub copied_at: Option<String>,
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
    let present = result
        .and_then(|result| serde_json::from_str::<CopyReport>(&result).ok())
        .map(|report| report.present.into_iter().collect())
        .unwrap_or_default();
    let (state, detail) = match last {
        None => ("never", None),
        Some(LastCopy { status, detail: text, detail_key: key, detail_params: params }) => {
            let shown = match (status.as_str(), key.as_deref()) {
                ("running", _) => "running",
                ("success", _) => "copied",
                (_, Some("JobDetailBackupCopyUnmounted")) => "unmounted",
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
    let copy = BackupCopy { folder: folder.display().to_string(), state, detail, copied_at };
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
    let finished = chrono::NaiveDateTime::parse_from_str(&finished?, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(finished.and_utc())
}
