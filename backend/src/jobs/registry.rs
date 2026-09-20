//! Background job tracking — the queue behind the Tasks screen.
//!
//! Jobs are recorded in SQLite so the UI can show history across restarts, and
//! one in-memory permit per key prevents the same long task (a full sync, an
//! enrichment pass) from running twice concurrently when the scheduler and a
//! user both trigger it. A caller either takes the permit if it is free or
//! waits its turn for it, first come first served, within a budget.

use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppResult;

/// The kinds of work Routarr runs in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Sync,
    Enrich,
    Simulate,
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
            JobKind::Enrich => "enrich",
            JobKind::Simulate => "simulate",
            JobKind::Apply => "apply",
            JobKind::Revert => "revert",
            JobKind::Maintenance => "maintenance",
            JobKind::Backup => "backup",
            JobKind::Scheduler => "scheduler",
        }
    }
}

#[derive(Clone)]
pub struct JobRegistry {
    pool: SqlitePool,
    /// One permit per key, created on first use and never removed: the keys
    /// are a handful of task names and instance ids. Held by whoever runs the
    /// task; the semaphore's own queue is what makes a wait first come first
    /// served.
    locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    /// How many callers are waiting for each key, so a wait can be bounded.
    waiting: Arc<Mutex<HashMap<String, usize>>>,
}

impl JobRegistry {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            locks: Arc::new(Mutex::new(HashMap::new())),
            waiting: Arc::new(Mutex::new(HashMap::new())),
        }
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
    /// while waiting leaves the queue — the future is dropped, and its place
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
    /// behind it. The place is given back when the returned guard drops —
    /// which a cancelled future does too, so a caller that hangs up while
    /// waiting does not keep its place.
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
    /// Without this, a crash mid-sync leaves a job spinning forever in the UI
    /// and its lock key is never released.
    pub async fn recover_orphans(&self) -> AppResult<u64> {
        let result = sqlx::query(
            "UPDATE jobs SET status = 'failed', finished_at = datetime('now'),
             error_message = 'Interrupted by a Routarr restart'
             WHERE status IN ('running', 'queued')",
        )
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
        trigger: &str,
        instance_id: Option<&str>,
        detail: &str,
    ) -> AppResult<JobHandle> {
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO jobs (id, kind, status, trigger, instance_id, detail)
             VALUES (?, ?, 'running', ?, ?, ?)",
        )
        .bind(&id)
        .bind(kind.as_str())
        .bind(trigger)
        .bind(instance_id)
        .bind(detail)
        .execute(&self.pool)
        .await?;

        info!(job_id = %id, kind = kind.as_str(), trigger, "Job started: {detail}");

        Ok(JobHandle { id, pool: self.pool.clone(), kind, settled: false })
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
}

impl JobHandle {
    /// Update the progress counters shown on the Tasks screen.
    pub async fn progress(&self, current: usize, total: usize) {
        let _ =
            sqlx::query("UPDATE jobs SET progress_current = ?, progress_total = ? WHERE id = ?")
                .bind(current as i64)
                .bind(total as i64)
                .bind(&self.id)
                .execute(&self.pool)
                .await;
    }

    /// Mark the job finished successfully.
    pub async fn succeed(mut self, detail: &str) {
        self.settled = true;
        info!(job_id = %self.id, kind = self.kind.as_str(), "Job finished: {detail}");
        let _ = sqlx::query(
            "UPDATE jobs SET status = 'success', detail = ?, finished_at = datetime('now') WHERE id = ?",
        )
        .bind(detail)
        .bind(&self.id)
        .execute(&self.pool)
        .await;
    }

    /// Mark the job failed, keeping the message for the UI and the log page.
    pub async fn fail(mut self, error: &str) {
        self.settled = true;
        warn!(job_id = %self.id, kind = self.kind.as_str(), "Job failed: {error}");
        let _ = sqlx::query(
            "UPDATE jobs SET status = 'failed', error_message = ?, finished_at = datetime('now') WHERE id = ?",
        )
        .bind(error)
        .bind(&self.id)
        .execute(&self.pool)
        .await;
    }
}

/// A handle dropped before an outcome was recorded is a job that ended
/// without saying how — a `?` between `start` and the outcome, or a panic on
/// the task — and the row would otherwise sit `running` until the next restart
/// failed it as an orphan, with the Tasks screen showing work in progress.
impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        // No runtime means the process is going down, and `recover_orphans`
        // will say so at the next start.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        warn!(job_id = %self.id, kind = self.kind.as_str(), "Job ended without reporting an outcome");
        let pool = self.pool.clone();
        let id = std::mem::take(&mut self.id);
        runtime.spawn(async move {
            let _ = sqlx::query(
                "UPDATE jobs SET status = 'failed', finished_at = datetime('now'),
                        error_message = 'The job ended without reporting an outcome; see the log'
                  WHERE id = ? AND status = 'running'",
            )
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
    /// without saying how — a `?` on the way, or a panic on the task — and the
    /// row sat `running` until the next restart failed it as an orphan.
    #[tokio::test]
    async fn a_handle_dropped_without_an_outcome_fails_its_job() {
        let registry = JobRegistry::new(crate::db::test_pool().await);
        let pool = registry.pool.clone();
        let handle = registry.start(JobKind::Sync, "manual", None, "syncing").await.unwrap();
        let id = handle.id.clone();
        drop(handle);

        let settled = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let row: (String, Option<String>) =
                    sqlx::query_as("SELECT status, error_message FROM jobs WHERE id = ?")
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
        assert!(
            settled.1.as_deref().unwrap_or_default().contains("without reporting"),
            "{settled:?}"
        );

        // The ordinary ending is untouched.
        let handle = registry.start(JobKind::Sync, "manual", None, "syncing").await.unwrap();
        let id = handle.id.clone();
        handle.succeed("done").await;
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
    /// comes back when its holder drops — cancelled or not.
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

    #[tokio::test]
    async fn job_lifecycle_is_persisted() {
        let pool = crate::db::test_pool().await;
        let registry = JobRegistry::new(pool.clone());

        let handle = registry.start(JobKind::Sync, "manual", None, "syncing").await.unwrap();
        let id = handle.id.clone();
        handle.progress(3, 10).await;
        handle.succeed("done").await;

        let (status, current, total, detail): (String, i64, i64, String) = sqlx::query_as(
            "SELECT status, progress_current, progress_total, detail FROM jobs WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(status, "success");
        assert_eq!((current, total), (3, 10));
        assert_eq!(detail, "done");
    }

    #[tokio::test]
    async fn orphaned_jobs_are_failed_on_startup() {
        let pool = crate::db::test_pool().await;
        let registry = JobRegistry::new(pool.clone());
        let handle = registry.start(JobKind::Enrich, "schedule", None, "enriching").await.unwrap();
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

        assert!(registry.locks.is_poisoned(), "précondition du test");
        assert!(
            registry.try_lock("sync:all").is_some(),
            "the registry must keep working after a task panicked"
        );
    }
}
