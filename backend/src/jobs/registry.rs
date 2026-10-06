//! Background job tracking, the queue behind the Tasks screen.
//!
//! Jobs are recorded in SQLite so the UI can show history across restarts, and
//! one in-memory permit per key prevents the same long task (a full sync, an
//! enrichment pass) from running twice concurrently when the scheduler and a
//! user both trigger it. A caller either takes the permit if it is free or
//! waits its turn for it, first come first served, within a budget.

use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::AppResult;
use crate::localization::{DEFAULT_LANGUAGE, Localizer};

/// The kinds of work Routarr runs in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Sync,
    /// Every enabled instance read again on one call: the task a caller
    /// follows, holding one report per instance. Each instance keeps its own
    /// `Sync` task beside it.
    SyncAll,
    Enrich,
    /// A simulation that stores its proposals, the one `/decisions` lists and
    /// an apply reads.
    Simulate,
    /// A simulation that stores nothing and answers its proposals alone.
    Preview,
    Apply,
    Revert,
    Maintenance,
    Backup,
    /// The unattended loop itself, recorded only when a pass *panicked*.
    ///
    /// Not a job anyone starts: the individual tasks keep their own kinds.
    /// This exists so a panic leaves evidence where the Tasks screen and
    /// `offline_warnings` already look, instead of only in a log nobody reads.
    Scheduler,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Sync => "sync",
            JobKind::SyncAll => "sync_all",
            JobKind::Enrich => "enrich",
            JobKind::Simulate => "simulate",
            JobKind::Preview => "preview",
            JobKind::Apply => "apply",
            JobKind::Revert => "revert",
            JobKind::Maintenance => "maintenance",
            JobKind::Backup => "backup",
            JobKind::Scheduler => "scheduler",
        }
    }
}

/// What a job is doing or did: a dictionary key and the values it names.
///
/// Stored as the key, not as a sentence, because the Tasks screen reads it
/// later in the language the interface speaks then (`api::jobs`). The log and
/// the `detail` column take the English text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    key: &'static str,
    params: BTreeMap<String, String>,
}

impl Detail {
    pub fn new(key: &'static str) -> Self {
        Self { key, params: BTreeMap::new() }
    }

    pub fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }

    fn english(&self) -> String {
        Localizer::new(DEFAULT_LANGUAGE).translate_map(self.key, &self.params)
    }

    fn stored_params(&self) -> String {
        serde_json::Value::from_iter(
            self.params
                .iter()
                .map(|(name, value)| (name.clone(), serde_json::Value::from(value.as_str()))),
        )
        .to_string()
    }
}

/// A stored detail in `localizer`'s language: from its key when the row has
/// one, as the text it was written with otherwise.
pub fn render_detail(
    localizer: &Localizer,
    key: Option<&str>,
    params: Option<&str>,
    text: Option<String>,
) -> Option<String> {
    let Some(key) = key else { return text };
    let params: BTreeMap<String, String> =
        params.and_then(|stored| serde_json::from_str(stored).ok()).unwrap_or_default();
    Some(localizer.translate_map(key, &params))
}

#[derive(Clone)]
pub struct JobRegistry {
    pool: SqlitePool,
    /// One permit per key, created on first use and never removed: the keys
    /// are a handful of task names and instance ids. Held by whoever runs the
    /// task. The semaphore's own queue is what makes a wait first come first
    /// served.
    locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    /// How many callers are waiting for each key, so a wait can be bounded.
    waiting: Arc<Mutex<HashMap<String, usize>>>,
    /// The work that writes, spawned through `jobs::detached`, which a stop
    /// of the server waits for.
    tracker: TaskTracker,
    /// Set as the server stops: a run that moves titles ends before its next
    /// move.
    closing: Arc<AtomicBool>,
    /// The cancel flag of each running job that reads one, by job id.
    cancels: Cancels,
}

type Cancels = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

impl JobRegistry {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            locks: Arc::new(Mutex::new(HashMap::new())),
            waiting: Arc::new(Mutex::new(HashMap::new())),
            tracker: TaskTracker::new(),
            closing: Arc::new(AtomicBool::new(false)),
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Run `work` on a task of its own that a stop of the server waits for.
    pub fn spawn_tracked<F>(&self, work: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tracker.spawn(work)
    }

    /// End every run that moves titles before its next move, then wait up to
    /// `grace` for the work in flight to record what it did. `false` when
    /// some was still running at the bound.
    pub async fn drain(&self, grace: Duration) -> bool {
        self.closing.store(true, Ordering::SeqCst);
        self.tracker.close();
        tokio::time::timeout(grace, self.tracker.wait()).await.is_ok()
    }

    /// Ask the running job `id` to stop before its next move. `false` when no
    /// running job of that id reads a cancel.
    pub fn cancel(&self, id: &str) -> bool {
        let cancels = self.cancels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cancels.get(id).map(|flag| flag.store(true, Ordering::SeqCst)).is_some()
    }

    /// The permit for `key`, made on first sight.
    ///
    /// A poisoned mutex is recovered rather than propagated: the guarded value
    /// is a map of semaphores with no invariant a panic could corrupt, and
    /// refusing every later task because one of them panicked would be worse
    /// than the panic itself.
    fn permit_for(&self, key: &str) -> Arc<Semaphore> {
        let mut locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(locks.entry(key.to_string()).or_insert_with(|| Arc::new(Semaphore::new(1))))
    }

    /// Wait for the lock on `key` for at most `budget`, in arrival order.
    ///
    /// `None` past the budget, with nothing taken. A caller that hangs up
    /// while waiting leaves the queue: the future is dropped, and its place
    /// in the semaphore's queue with it.
    pub async fn lock_within(&self, key: &str, budget: Duration) -> Option<JobLock> {
        let permit = self.permit_for(key);
        let acquired = tokio::time::timeout(budget, permit.acquire_owned()).await.ok()?;
        acquired.ok().map(|permit| JobLock { _permit: permit })
    }

    /// Take a place in the queue for `key`, or none when `max_waiting` are
    /// already there.
    ///
    /// A lock a caller *waits* for costs a connection and a task for as long
    /// as the wait lasts, and `try_lock` alone bounds the work, not the queue
    /// behind it. The place is given back when the returned guard drops, as it
    /// does with a cancelled future, so a caller that hangs up while waiting
    /// does not keep its place.
    pub fn try_wait(&self, key: &str, max_waiting: usize) -> Option<WaitingPlace> {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = waiting.entry(key.to_string()).or_insert(0);
        if *count >= max_waiting {
            return None;
        }
        *count += 1;
        Some(WaitingPlace { key: key.to_string(), waiting: Arc::clone(&self.waiting) })
    }

    /// How many callers are waiting for `key` right now. Only a test asks.
    #[cfg(test)]
    pub fn waiting_for(&self, key: &str) -> usize {
        let waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        waiting.get(key).copied().unwrap_or(0)
    }

    /// Mark jobs left `running` by a previous process as failed.
    ///
    /// Without this, a crash mid-sync leaves a job spinning forever in the UI.
    /// Its lock needs no release: the permits live in memory and die with the
    /// process.
    pub async fn recover_orphans(&self) -> AppResult<u64> {
        let interrupted = Detail::new("JobDetailInterrupted");
        let result = sqlx::query(
            "UPDATE jobs SET status = 'failed', finished_at = datetime('now'),
             detail = ?, detail_key = ?, detail_params = NULL
             WHERE status = 'running'",
        )
        .bind(interrupted.english())
        .bind(interrupted.key)
        .execute(&self.pool)
        .await?;

        let n = result.rows_affected();
        if n > 0 {
            warn!("Marked {n} interrupted job(s) as failed after restart");
        }
        Ok(n)
    }

    /// Take the lock on `key` if it is free, or return `None` at once.
    pub fn try_lock(&self, key: &str) -> Option<JobLock> {
        self.permit_for(key).try_acquire_owned().ok().map(|permit| JobLock { _permit: permit })
    }

    /// Record the start of a job and return a handle for progress and outcome.
    pub async fn start(
        &self,
        kind: JobKind,
        by: &super::Attribution,
        instance_id: Option<&str>,
        detail: Detail,
    ) -> AppResult<JobHandle> {
        let id = Uuid::new_v4().to_string();
        let english = detail.english();
        let trigger = by.trigger.as_str();

        sqlx::query(
            "INSERT INTO jobs (id, kind, status, trigger, subject, instance_id, detail, detail_key,
                               detail_params)
             VALUES (?, ?, 'running', ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(kind.as_str())
        .bind(trigger)
        .bind(&by.subject)
        .bind(instance_id)
        .bind(&english)
        .bind(detail.key)
        .bind(detail.stored_params())
        .execute(&self.pool)
        .await?;

        info!(job_id = %id, kind = kind.as_str(), trigger, "Job started: {english}");

        announce(&id);
        Ok(JobHandle {
            id,
            pool: self.pool.clone(),
            kind,
            settled: false,
            result: None,
            cancels: Arc::clone(&self.cancels),
            closing: Arc::clone(&self.closing),
        })
    }
}

/// What ends a run that moves titles before its last move, read before each.
#[derive(Clone, Default)]
pub struct Stop {
    cancelled: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
}

impl Stop {
    /// A person cancelled the job.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// The server is stopping.
    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }
}

/// Released when dropped, including on panic or early return: the permit goes
/// back to its semaphore, and the next in its queue wakes.
pub struct JobLock {
    _permit: OwnedSemaphorePermit,
}

/// A place in the queue behind a key, returned by [`JobRegistry::try_wait`].
pub struct WaitingPlace {
    key: String,
    waiting: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for WaitingPlace {
    fn drop(&mut self) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = waiting.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                waiting.remove(&self.key);
            }
        }
    }
}

/// Handle to a running job.
pub struct JobHandle {
    pub id: String,
    pool: SqlitePool,
    kind: JobKind,
    /// Whether an outcome was recorded, so `Drop` knows when it has to.
    settled: bool,
    /// The report the job answers, written with its outcome.
    result: Option<String>,
    cancels: Cancels,
    closing: Arc<AtomicBool>,
}

/// The first job a piece of work started, and where to say its id.
struct Started {
    tell: Option<tokio::sync::oneshot::Sender<String>>,
    id: Option<String>,
}

tokio::task_local! {
    /// Where the first job a piece of work starts says its id, for a caller
    /// that answers as soon as the work has begun (`api::jobs::answer`).
    static STARTED: std::cell::RefCell<Started>;
}

/// Run `work`, saying through `started` the id of the first job it starts.
/// The sender goes with the work, so a work that ends before starting any job
/// closes the channel instead.
pub async fn announcing<T>(
    started: tokio::sync::oneshot::Sender<String>,
    work: impl std::future::Future<Output = T>,
) -> T {
    STARTED.scope(std::cell::RefCell::new(Started { tell: Some(started), id: None }), work).await
}

/// The id of the first job the running work started, when it runs under
/// [`announcing`]: the task a caller polls for the work's outcome.
pub fn announced() -> Option<String> {
    STARTED.try_with(|started| started.borrow().id.clone()).ok().flatten()
}

fn announce(id: &str) {
    let _ = STARTED.try_with(|started| {
        let mut started = started.borrow_mut();
        if let Some(tell) = started.tell.take() {
            started.id = Some(id.to_string());
            let _ = tell.send(id.to_string());
        }
    });
}

/// Where a long pass says how far it has gone, apart from the `JobHandle`
/// that settles the job, so a service reports without owning the job.
#[derive(Debug, Clone)]
pub struct Progress {
    id: String,
    pool: SqlitePool,
}

impl Progress {
    /// Update the progress counters the Tasks screen and a caller following
    /// the job read.
    pub async fn report(&self, current: usize, total: usize) {
        let _ =
            sqlx::query("UPDATE jobs SET progress_current = ?, progress_total = ? WHERE id = ?")
                .bind(current as i64)
                .bind(total as i64)
                .bind(&self.id)
                .execute(&self.pool)
                .await;
    }
}

impl JobHandle {
    /// Update the progress counters shown on the Tasks screen.
    pub async fn progress(&self, current: usize, total: usize) {
        self.progress_reporter().report(current, total).await;
    }

    pub fn progress_reporter(&self) -> Progress {
        Progress { id: self.id.clone(), pool: self.pool.clone() }
    }

    /// Keep the report this job answers, to be written with its outcome, so a
    /// finished job is never read without it.
    pub fn report(&mut self, result: &impl serde::Serialize) {
        self.result = serde_json::to_string(result).ok();
    }

    /// What stops this job's run before its end: a cancel, which this lets
    /// [`JobRegistry::cancel`] send, or the server stopping.
    pub fn stop(&self) -> Stop {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cancels = self.cancels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cancels.insert(self.id.clone(), Arc::clone(&cancelled));
        Stop { cancelled, closing: Arc::clone(&self.closing) }
    }

    /// Mark the job finished successfully, saying what it did.
    pub async fn succeed(self, detail: Detail) {
        self.settle("success", detail).await;
    }

    /// Mark the job cancelled, saying what it did before it stopped.
    pub async fn cancelled(self, detail: Detail) {
        self.settle("cancelled", detail).await;
    }

    /// Mark the job failed on an outcome it reports itself, such as every move
    /// of an apply refused, as opposed to an error it ran into.
    pub async fn fail_with(self, detail: Detail) {
        self.settle("failed", detail).await;
    }

    async fn settle(mut self, status: &'static str, detail: Detail) {
        self.settled = true;
        self.forget_cancel();
        let english = detail.english();
        info!(job_id = %self.id, kind = self.kind.as_str(), status, "Job finished: {english}");
        let (id, result) = (self.id.clone(), self.result.clone());
        let params = detail.stored_params();
        record_outcome(self.pool.clone(), self.id.clone(), move |pool| {
            let (id, english, params, result) =
                (id.clone(), english.clone(), params.clone(), result.clone());
            async move {
                sqlx::query(
                    "UPDATE jobs SET status = ?, detail = ?, detail_key = ?, detail_params = ?,
                            result = ?, finished_at = datetime('now')
                      WHERE id = ?",
                )
                .bind(status)
                .bind(english)
                .bind(detail.key)
                .bind(params)
                .bind(result)
                .bind(id)
                .execute(&pool)
                .await
                .map(|_| ())
            }
        })
        .await;
    }

    /// Mark the job failed on an error, whose text the Tasks screen shows as it
    /// is: it comes from the database, the network or an Arr, not from here.
    /// The log keeps the whole error, and the row what a caller may read of
    /// it: any key reads the task list.
    pub async fn fail(mut self, error: &crate::error::AppError) {
        self.settled = true;
        self.forget_cancel();
        warn!(job_id = %self.id, kind = self.kind.as_str(), "Job failed: {error}");
        let (id, error) = (self.id.clone(), error.public_message());
        record_outcome(self.pool.clone(), self.id.clone(), move |pool| {
            let (id, error) = (id.clone(), error.clone());
            async move {
                sqlx::query(
                    "UPDATE jobs SET status = 'failed', error_message = ?,
                            finished_at = datetime('now')
                      WHERE id = ?",
                )
                .bind(error)
                .bind(id)
                .execute(&pool)
                .await
                .map(|_| ())
            }
        })
        .await;
    }
}

/// How long a job's outcome waits before it is written again.
const OUTCOME_RETRY: Duration =
    if cfg!(test) { Duration::from_millis(20) } else { Duration::from_secs(1) };

/// Write a job's outcome, and again twice on a task of its own when the
/// database refuses it: a busy database past its timeout would otherwise leave
/// the task `running` until the next start, and a caller following it waiting
/// for ever. The last refusal is logged.
async fn record_outcome<W, F>(pool: SqlitePool, id: String, write: W)
where
    W: Fn(SqlitePool) -> F + Send + 'static,
    F: std::future::Future<Output = Result<(), sqlx::Error>> + Send,
{
    let Err(first) = write(pool.clone()).await else { return };
    warn!(job_id = %id, "The outcome of a job could not be written, trying again: {first}");
    tokio::spawn(async move {
        let mut last = first;
        for _ in 0..2 {
            tokio::time::sleep(OUTCOME_RETRY).await;
            match write(pool.clone()).await {
                Ok(()) => return,
                Err(e) => last = e,
            }
        }
        error!(job_id = %id, "The outcome of a job was never written: {last}");
    });
}

impl JobHandle {
    fn forget_cancel(&self) {
        let mut cancels = self.cancels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cancels.remove(&self.id);
    }
}

/// A handle dropped before an outcome was recorded is a job that ended
/// without saying how (a `?` between `start` and the outcome, or a panic on
/// the task), and the row would otherwise sit `running` until the next restart
/// failed it as an orphan, with the Tasks screen showing work in progress.
impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.forget_cancel();
        // No runtime means the process is going down, and `recover_orphans`
        // will say so at the next start.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        warn!(job_id = %self.id, kind = self.kind.as_str(), "Job ended without reporting an outcome");
        let pool = self.pool.clone();
        let id = std::mem::take(&mut self.id);
        let no_outcome = Detail::new("JobDetailNoOutcome");
        runtime.spawn(async move {
            let _ = sqlx::query(
                "UPDATE jobs SET status = 'failed', finished_at = datetime('now'),
                        detail = ?, detail_key = ?, detail_params = NULL
                  WHERE id = ? AND status = 'running'",
            )
            .bind(no_outcome.english())
            .bind(no_outcome.key)
            .bind(id)
            .execute(&pool)
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn locks_are_exclusive_and_released_on_drop() {
        let registry = JobRegistry::new(crate::db::test_pool().await);

        let first = registry.try_lock("sync:all");
        assert!(first.is_some());
        assert!(registry.try_lock("sync:all").is_none(), "second lock must be refused");
        assert!(registry.try_lock("sync:other").is_some(), "unrelated key must be free");

        drop(first);
        assert!(registry.try_lock("sync:all").is_some(), "lock must be released on drop");
    }

    /// A handle dropped before an outcome was recorded is a job that ended
    /// without saying how (a `?` on the way, or a panic on the task). Its row
    /// fails at once rather than sitting `running` until the next restart
    /// fails it as an orphan.
    #[tokio::test]
    async fn a_handle_dropped_without_an_outcome_fails_its_job() {
        let registry = JobRegistry::new(crate::db::test_pool().await);
        let pool = registry.pool.clone();
        let handle = registry
            .start(
                JobKind::Sync,
                &super::super::Attribution::manual(None),
                None,
                Detail::new("JobDetailSyncing").with("instance", "Radarr"),
            )
            .await
            .unwrap();
        let id = handle.id.clone();
        drop(handle);

        let settled = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let row: (String, Option<String>) =
                    sqlx::query_as("SELECT status, detail_key FROM jobs WHERE id = ?")
                        .bind(&id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if row.0 != "running" {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job was left running");
        assert_eq!(settled.0, "failed");
        assert_eq!(settled.1.as_deref(), Some("JobDetailNoOutcome"), "{settled:?}");

        // The ordinary ending is untouched.
        let handle = registry
            .start(
                JobKind::Sync,
                &super::super::Attribution::manual(None),
                None,
                Detail::new("JobDetailSyncing").with("instance", "Radarr"),
            )
            .await
            .unwrap();
        let id = handle.id.clone();
        handle.succeed(Detail::new("JobDetailSynced").with("media", 3).with("folders", 1)).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "success");
    }

    /// A wait is served in the order it was asked, and a newcomer that finds
    /// the key busy joins the back of the queue rather than racing the front.
    #[tokio::test]
    async fn waits_are_served_in_the_order_they_were_asked() {
        let registry = JobRegistry::new(crate::db::test_pool().await);
        let served: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        let held = registry.try_lock("sync:all").expect("free");
        let mut waiters = Vec::new();
        for n in 1..=3u8 {
            let (registry, served) = (registry.clone(), Arc::clone(&served));
            waiters.push(tokio::spawn(async move {
                let lock = registry.lock_within("sync:all", Duration::from_secs(5)).await;
                served.lock().unwrap().push(n);
                lock
            }));
            // Queued before the next one asks, so the order asked is known.
            tokio::task::yield_now().await;
        }

        drop(held);
        for waiter in waiters {
            assert!(waiter.await.unwrap().is_some(), "a waiter within the budget was refused");
        }
        assert_eq!(*served.lock().unwrap(), [1, 2, 3], "served out of the order asked");
    }

    /// Past the budget a wait answers `None` and holds nothing.
    #[tokio::test]
    async fn a_wait_past_its_budget_takes_nothing() {
        let registry = JobRegistry::new(crate::db::test_pool().await);
        let held = registry.try_lock("sync:all").expect("free");

        assert!(registry.lock_within("sync:all", Duration::from_millis(50)).await.is_none());

        drop(held);
        assert!(registry.lock_within("sync:all", Duration::from_millis(50)).await.is_some());
    }

    /// The queue behind a key is bounded like the key itself, and a place
    /// comes back when its holder drops, cancelled or not.
    #[tokio::test]
    async fn a_place_in_the_queue_is_refused_past_the_bound_and_returned_on_drop() {
        let registry = JobRegistry::new(crate::db::test_pool().await);

        let first = registry.try_wait("webhook:inst-1", 2).expect("the queue is empty");
        let second = registry.try_wait("webhook:inst-1", 2).expect("one place left");
        assert!(registry.try_wait("webhook:inst-1", 2).is_none(), "a third must be refused");
        assert_eq!(registry.waiting_for("webhook:inst-1"), 2);
        assert!(registry.try_wait("webhook:inst-2", 2).is_some(), "another key has its own queue");

        drop(first);
        assert_eq!(registry.waiting_for("webhook:inst-1"), 1);
        assert!(registry.try_wait("webhook:inst-1", 2).is_some(), "the place came back");

        drop(second);
    }

    /// A busy database refusing a job's outcome leaves the task running for
    /// ever unless the write is tried again: written once the database takes
    /// it, and logged when it never does.
    #[tokio::test]
    async fn a_job_outcome_the_database_refuses_is_written_again_or_logged() {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = crate::tests::LogCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
        let _logging = tracing::subscriber::set_default(subscriber);
        let pool = crate::db::test_pool().await;
        let registry = JobRegistry::new(pool.clone());
        let by = super::super::Attribution::manual(None);
        let detail = || Detail::new("JobDetailSyncing").with("instance", "Radarr");
        let hide = "ALTER TABLE jobs RENAME TO jobs_away";
        let restore = "ALTER TABLE jobs_away RENAME TO jobs";

        let refused_once = registry.start(JobKind::Sync, &by, None, detail()).await.unwrap();
        let id = refused_once.id.clone();
        sqlx::query(hide).execute(&pool).await.unwrap();
        refused_once.succeed(detail()).await;
        sqlx::query(restore).execute(&pool).await.unwrap();
        tokio::time::sleep(OUTCOME_RETRY * 3).await;
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "success", "the outcome was not written again");

        let refused = registry.start(JobKind::Sync, &by, None, detail()).await.unwrap();
        let id = refused.id.clone();
        sqlx::query(hide).execute(&pool).await.unwrap();
        refused.succeed(detail()).await;
        tokio::time::sleep(OUTCOME_RETRY * 4).await;
        let logged = capture.contents();
        assert!(logged.contains("was never written") && logged.contains(&id), "{logged}");
    }

    #[tokio::test]
    async fn job_lifecycle_is_persisted() {
        let pool = crate::db::test_pool().await;
        let registry = JobRegistry::new(pool.clone());

        let handle = registry
            .start(
                JobKind::Sync,
                &super::super::Attribution::manual(None),
                None,
                Detail::new("JobDetailSyncing").with("instance", "Radarr"),
            )
            .await
            .unwrap();
        let id = handle.id.clone();
        handle.progress(3, 10).await;
        handle.succeed(Detail::new("JobDetailSynced").with("media", 3).with("folders", 1)).await;

        let (status, current, total, detail, key, params): (
            String,
            i64,
            i64,
            String,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT status, progress_current, progress_total, detail, detail_key, detail_params
                   FROM jobs WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(status, "success");
        assert_eq!((current, total), (3, 10));
        assert_eq!(detail, "Titles: 3, root folders: 1");
        assert_eq!(key, "JobDetailSynced");
        assert_eq!(
            render_detail(&Localizer::new("fr"), Some(&key), Some(&params), Some(detail))
                .as_deref(),
            Some("Titres\u{a0}: 3, dossiers racines\u{a0}: 1"),
        );
    }

    /// A row without a detail key, which an upgraded database holds, reads as
    /// it was written.
    #[test]
    fn a_detail_without_a_key_keeps_its_text() {
        let french = Localizer::new("fr");
        let text = Some("media evaluated: 3, moves required: 1".to_string());
        assert_eq!(render_detail(&french, None, None, text.clone()), text);
        assert_eq!(render_detail(&french, None, None, None), None);
    }

    #[tokio::test]
    async fn orphaned_jobs_are_failed_on_startup() {
        let pool = crate::db::test_pool().await;
        let registry = JobRegistry::new(pool.clone());
        let handle = registry
            .start(
                JobKind::Enrich,
                &super::super::Attribution::unattended("schedule"),
                None,
                Detail::new("JobDetailEnriching"),
            )
            .await
            .unwrap();
        let id = handle.id.clone();
        std::mem::forget(handle); // simulate a crash: never finished

        assert_eq!(registry.recover_orphans().await.unwrap(), 1);

        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
    }
}

#[cfg(test)]
mod poison_tests {
    use super::*;

    #[tokio::test]
    async fn a_panicking_task_does_not_wedge_the_registry() {
        let registry = JobRegistry::new(crate::db::test_pool().await);

        // Poison the mutex the way a panic inside a locked section would.
        let poisoner = registry.clone();
        std::thread::spawn(move || {
            let _guard = poisoner.locks.lock().unwrap();
            panic!("boom");
        })
        .join()
        .expect_err("the thread is expected to panic");

        assert!(registry.locks.is_poisoned(), "the precondition of this test");
        assert!(
            registry.try_lock("sync:all").is_some(),
            "the registry must keep working after a task panicked"
        );
    }
}
