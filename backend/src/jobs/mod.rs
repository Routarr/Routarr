pub mod registry;
pub mod scheduler;

pub use registry::{Detail, JobHandle, JobKind, JobLock, JobRegistry, Progress, Stop, announcing};

/// Run work that writes on a task of its own, and wait for it: a request future is dropped at
/// its next await once the client hangs up. Spawned, the work keeps its lock and job handle,
/// and the registry tracks it, so a server stop waits for it too.
pub async fn detached<T, F>(jobs: &JobRegistry, work: F) -> crate::error::AppResult<T>
where
    F: std::future::Future<Output = crate::error::AppResult<T>> + Send + 'static,
    T: Send + 'static,
{
    jobs.spawn_tracked(work).await.map_err(|e| {
        crate::error::AppError::Internal(format!("the task ended before it reported: {e}"))
    })?
}

/// What set a job off. Stored on the job row and rendered on the Tasks
/// queue, where the frontend builds its translation key as `Trigger{Capitalised}`,
/// so a new value here needs its `Trigger…` key in `locales/en.json`.
pub const TRIGGER_MANUAL: &str = "manual";
pub const TRIGGER_SCHEDULE: &str = "schedule";
pub const TRIGGER_WEBHOOK: &str = "webhook";
/// An application key asked, and its name is the subject.
pub const TRIGGER_API: &str = "api";
/// What the automation does once a sync has run: the enrichment, the
/// simulation and the moves applied without asking. The subject is who set
/// the sync off, when somebody did.
pub const TRIGGER_AUTO: &str = "auto";

/// The longest interval an instance may be synced on, in minutes: a day. Every writer of
/// `sync_interval_minutes` clamps to it and `scheduler::is_due` reads it, so the stored interval
/// is the one compared. The sync runs on the first scheduler tick past it.
pub const MAX_SYNC_INTERVAL_MINUTES: i64 = 24 * 60;

/// The lock key a library-wide simulation holds: two at once evaluate the library twice and keep
/// the later read. A one-item pass takes none, since `routing::store_run` writes only what it
/// evaluated, so a season import never queues behind a sweep. How many passes load at once is
/// `routing::LibraryPasses`'s question.
pub const FULL_SIMULATION: &str = "simulate";

/// What caused a write, and who asked for it, carried together since every writer needs both
/// and two parallel parameters is how one gets forgotten at a call site.
#[derive(Debug, Clone)]
pub struct Attribution {
    /// One of the `TRIGGER_*` constants.
    pub trigger: String,
    /// `None` when nobody asked ([`Attribution::unattended`]) or when the mode
    /// names nobody (`Identity::actor`).
    pub subject: Option<String>,
    /// The application key that asked, when one did: its name can be given
    /// again once it is revoked, and what it wrote stays its own.
    pub key: Option<String>,
    /// The task the work runs in, which each move it logs names.
    pub job: Option<String>,
}

impl Attribution {
    /// Somebody asked, through the interface or with the master key.
    pub fn manual(subject: Option<&str>) -> Self {
        Self {
            trigger: TRIGGER_MANUAL.to_string(),
            subject: subject.map(str::to_string),
            key: None,
            job: None,
        }
    }

    /// Nobody asked: the scheduler, or an Arr's webhook.
    pub fn unattended(trigger: &str) -> Self {
        Self { trigger: trigger.to_string(), subject: None, key: None, job: None }
    }

    /// An application asked, with the key `key` that names it.
    pub fn application(name: &str, key: &str) -> Self {
        Self {
            trigger: TRIGGER_API.to_string(),
            subject: Some(name.to_string()),
            key: Some(key.to_string()),
            job: None,
        }
    }

    /// The automation, after a sync `by` set off.
    pub fn automatic(by: &Attribution) -> Self {
        Self {
            trigger: TRIGGER_AUTO.to_string(),
            subject: by.subject.clone(),
            key: by.key.clone(),
            job: None,
        }
    }

    pub fn in_job(&self, job: &str) -> Self {
        Self { job: Some(job.to_string()), ..self.clone() }
    }
}
