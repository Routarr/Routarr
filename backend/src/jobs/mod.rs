pub mod registry;
pub mod scheduler;

pub use registry::{JobHandle, JobKind, JobRegistry};

/// What set a job off. Stored on the job row and rendered on the Tasks
/// queue, where the frontend builds its translation key as `Trigger{Capitalised}`
/// — so adding a value here means adding `Trigger…` to every file in `locales/`.
pub const TRIGGER_MANUAL: &str = "manual";
pub const TRIGGER_SCHEDULE: &str = "schedule";
pub const TRIGGER_WEBHOOK: &str = "webhook";

/// The longest interval an instance may be synced on, in minutes: a day.
///
/// Every writer of `sync_interval_minutes` clamps to it and `scheduler::is_due`
/// reads it, so the number on screen is the number that runs. Written once
/// because a fourth site with its own `1440` is how the two drift apart.
pub const MAX_SYNC_INTERVAL_MINUTES: i64 = 24 * 60;

/// The lock key a library-wide simulation holds.
///
/// Two full passes racing both supersede the other's pending decisions, and the
/// later commit wins — so the surviving proposals may have been computed from a
/// rule set that changed in between. Only *full* passes take it: the webhook
/// evaluates one item and `store_decisions` supersedes only what it evaluated,
/// so a season import must never queue behind a sweep. How many passes *load*
/// at once is another question, answered by `routing::library_pass`.
pub const FULL_SIMULATION: &str = "simulate";

/// What caused a write, and who asked for it.
///
/// The trigger answers "did the nightly sweep do this, or did somebody"; the
/// subject answers which somebody, which only has an answer once a mode
/// vouches for a name. Carried together because every writer needs both and
/// two parallel parameters is how one of them gets forgotten at a call site.
#[derive(Debug, Clone)]
pub struct Attribution {
    /// One of the `TRIGGER_*` constants.
    pub trigger: String,
    /// See `Identity::actor` for the three cases where there is no name.
    pub subject: Option<String>,
}

impl Attribution {
    /// Somebody asked, through the interface or the API.
    pub fn manual(subject: Option<&str>) -> Self {
        Self { trigger: TRIGGER_MANUAL.to_string(), subject: subject.map(str::to_string) }
    }

    /// Nobody asked: the scheduler, or an Arr's webhook.
    pub fn unattended(trigger: &str) -> Self {
        Self { trigger: trigger.to_string(), subject: None }
    }
}
