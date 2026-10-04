pub mod registry;
pub mod scheduler;

pub use registry::{Detail, JobHandle, JobKind, JobRegistry, Progress, announcing};

/// Run work that writes on a task of its own, and wait for it.
///
/// A request future is dropped the moment the client hangs up (a browser
/// navigating away, a proxy timing out, an Arr whose webhook timed out), at
/// its next await. Work that writes must reach its end whatever the caller
/// does: an apply the Arr has performed is recorded, a sync that read the
/// library stores it. Spawned, the work keeps its lock and its job handle, so
/// the next caller still waits its turn and the Tasks screen sees it finish.
pub async fn detached<T, F>(work: F) -> crate::error::AppResult<T>
where
    F: std::future::Future<Output = crate::error::AppResult<T>> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(work).await.map_err(|e| {
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

/// The longest interval an instance may be synced on, in minutes: a day.
///
/// Every writer of `sync_interval_minutes` clamps to it and `scheduler::is_due`
/// reads it, so the stored interval is the one compared. The sync itself runs
/// on the first scheduler tick past it, so the tick is its grain. Written once
/// because another site with its own `1440` is how the two drift apart.
pub const MAX_SYNC_INTERVAL_MINUTES: i64 = 24 * 60;

/// The lock key a library-wide simulation holds.
///
/// Two full passes racing both supersede the other's pending decisions, and the
/// later commit wins, so the surviving proposals may have been computed from a
/// rule set that changed in between. Only *full* passes take it: the webhook
/// evaluates one item and `store_decisions` supersedes only what it evaluated,
/// so a season import must never queue behind a sweep. How many passes *load*
/// at once is another question, answered by `routing::library_pass`.
pub const FULL_SIMULATION: &str = "simulate";

/// What caused a write, and who asked for it.
///
/// The trigger answers "did the nightly sweep do this, or did somebody". The
/// subject answers which somebody, which only has an answer once a mode
/// vouches for a name. Carried together because every writer needs both and
/// two parallel parameters is how one of them gets forgotten at a call site.
#[derive(Debug, Clone)]
pub struct Attribution {
    /// One of the `TRIGGER_*` constants.
    pub trigger: String,
    /// `None` when nobody asked ([`Attribution::unattended`]) or when the mode
    /// names nobody (`Identity::actor`).
    pub subject: Option<String>,
}

impl Attribution {
    /// Somebody asked, through the interface or with the master key.
    pub fn manual(subject: Option<&str>) -> Self {
        Self { trigger: TRIGGER_MANUAL.to_string(), subject: subject.map(str::to_string) }
    }

    /// Nobody asked: the scheduler, or an Arr's webhook.
    pub fn unattended(trigger: &str) -> Self {
        Self { trigger: trigger.to_string(), subject: None }
    }

    /// An application asked, with a key that names it.
    pub fn application(name: &str) -> Self {
        Self { trigger: TRIGGER_API.to_string(), subject: Some(name.to_string()) }
    }
}
