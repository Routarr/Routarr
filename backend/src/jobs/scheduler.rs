//! Background scheduler. Each instance is polled on its own
//! `sync_interval_minutes` rather than on one shared sweep.

use futures::FutureExt;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info};

use crate::jobs::{FULL_SIMULATION, JobKind, TRIGGER_SCHEDULE};
use crate::services::{auto_apply, backup, enrichment, maintenance, routing, sync};
use crate::state::AppState;

/// How long the loop waits: before its first pass, and at least between two.
///
/// Stated apart from the loop so a test can drive `start` itself in
/// milliseconds; the application never changes them.
pub struct Timings {
    /// Before the first pass, so the server finishes binding first.
    pub settle: Duration,
    /// The least a pass waits for the next, whatever the setting says.
    pub floor: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self { settle: Duration::from_secs(5), floor: Duration::from_secs(60) }
    }
}

// Makes the next pass panic, once. The loop's whole promise is to outlive a
// panic and say so, and nothing else in the code base can be made to panic on
// demand. Thread-local, because the tests share one process and a
// `#[tokio::test]` runs its tasks on its own thread: global, the flag one test
// raised was consumed by another test's pass.
#[cfg(test)]
thread_local! {
    pub(crate) static PANIC_NEXT_TICK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Start the polling loop. Returns a handle the caller waits on when stopping.
///
/// **A panic must not end the loop.** A panic unwinds the task and nothing
/// holds its `JoinHandle`, so syncs, backups and purges would stop for good
/// while the server kept answering and the interface looked healthy.
///
/// **Stopping has to be waited for.** `main` closes the pool afterwards, and
/// `wal_checkpoint(TRUNCATE)` cannot truncate while another connection is
/// writing — which is what a mid-flight pass is doing.
pub fn start(state: AppState, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    start_with(state, shutdown, Timings::default())
}

pub fn start_with(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
    timings: Timings,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("Background scheduler started");
        if wait_or_stop(&mut shutdown, timings.settle).await {
            return;
        }

        // instance_id -> when it was last synced by the scheduler.
        let mut last_sync: HashMap<String, tokio::time::Instant> = HashMap::new();
        let mut last_maintenance: Option<tokio::time::Instant> = None;
        let mut last_backup: Option<tokio::time::Instant> = None;
        // The work a pass hands off — enrichment, simulation, auto-apply —
        // running while the loop goes on syncing on time. Kept here so a
        // shutdown waits for it rather than closing the pool under it.
        let mut chain: Option<JoinHandle<()>> = None;

        loop {
            let pass = AssertUnwindSafe(tick(
                &state,
                &mut last_sync,
                &mut last_maintenance,
                &mut last_backup,
                &mut chain,
            ))
            .catch_unwind()
            .await;

            match pass {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("Scheduler tick failed: {e}"),
                // Carrying on quietly would be its own failure: nothing gives
                // an operator a reason to open the log of an application that
                // looks well. The incident goes to the `jobs` table, which the
                // Tasks screen and `offline_warnings` both read.
                Err(panic) => record_panic(&state, &describe_panic(panic)).await,
            }

            let minutes: u64 = state.setting("scheduler_interval_minutes", 15u64).await;
            let wait = Duration::from_secs(minutes.min(24 * 60) * 60).max(timings.floor);
            if wait_or_stop(&mut shutdown, wait).await {
                if let Some(running) = chain.take() {
                    reap(&state, running).await;
                }
                info!("Background scheduler stopped");
                return;
            }
        }
    })
}

fn describe_panic(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown cause".to_string())
}

/// Best effort: a panic caused by an unreachable database takes this write
/// with it.
async fn record_panic(state: &AppState, cause: &str) {
    error!("Scheduler pass panicked and was restarted: {cause}");
    if let Ok(job) =
        state.jobs.start(JobKind::Scheduler, TRIGGER_SCHEDULE, None, "Unattended pass").await
    {
        job.fail(&format!("the pass panicked: {cause}")).await;
    }
}

/// Wait for a finished or finishing chain, and say so if it panicked: a
/// spawned task's panic ends in its `JoinHandle` and nowhere else.
async fn reap(state: &AppState, chain: JoinHandle<()>) {
    if let Err(e) = chain.await
        && e.is_panic()
    {
        record_panic(state, &format!("post-sync work: {}", describe_panic(e.into_panic()))).await;
    }
}

/// What follows a sync: enrich what is new, simulate, and apply what the
/// unattended guardrails allow. On a task of its own — see `tick`.
fn spawn_post_sync(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = enrichment::enrich_all_media(&state, "schedule").await {
            error!("Scheduled enrichment failed: {e}");
        }

        // Keep the pending decision list current so the dashboard is meaningful
        // without the user having to press "Simulate" first.
        // Serialised against the one a user can start from the interface. Two
        // full passes racing both supersede the other's pending decisions and
        // the later commit wins, so the survivor may have been computed from
        // rules that changed in between. The webhook's single-item run takes no
        // such lock and must not: `store_decisions` supersedes only the media it
        // evaluated, so a season import is never blocked by a sweep.
        if state.bool_setting("auto_simulate_enabled", true).await
            && let Some(_pass) = state.jobs.try_lock(FULL_SIMULATION)
        {
            let options = routing::SimulationOptions {
                trigger: crate::jobs::TRIGGER_SCHEDULE.to_string(),
                persist: true,
                language: state.language().await,
                ..Default::default()
            };
            match routing::run_simulation(&state.pool, options).await {
                Ok(result) => {
                    // Catches what the webhook missed — an instance without a
                    // webhook configured, or an item added while Routarr was
                    // down. Same guardrails: only file-free media, capped, and
                    // nothing at all if the sweep is too large.
                    if let Err(e) = auto_apply::apply_simulation(
                        &state,
                        &result.simulation_id,
                        crate::jobs::TRIGGER_SCHEDULE,
                    )
                    .await
                    {
                        error!("Scheduled auto-apply failed: {e}");
                    }
                }
                Err(e) => error!("Scheduled simulation failed: {e}"),
            }
        }
    })
}

/// Sleep, unless asked to stop first. `true` means stop.
///
/// Only the wait is interruptible, so a shutdown arriving mid-pass lets that
/// pass finish rather than cutting a sync in half.
async fn wait_or_stop(shutdown: &mut watch::Receiver<bool>, wait: Duration) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        _ = sleep(wait) => false,
        _ = shutdown.changed() => true,
    }
}

/// Exposed to `src/tests/scheduler.rs`: the orchestration is what runs
/// unattended, and needs a real database and a real Arr to be exercised.
pub(crate) async fn tick(
    state: &AppState,
    last_sync: &mut HashMap<String, tokio::time::Instant>,
    last_maintenance: &mut Option<tokio::time::Instant>,
    last_backup: &mut Option<tokio::time::Instant>,
    chain: &mut Option<JoinHandle<()>>,
) -> crate::error::AppResult<()> {
    #[cfg(test)]
    if PANIC_NEXT_TICK.with(|flag| flag.replace(false)) {
        panic!("a sweep blew up");
    }

    // Auto-sync only gates the sync sweep. Retention maintenance keeps running:
    // disabling background syncs must not silently disable the purges too.
    let auto_sync = state.bool_setting("auto_sync_enabled", true).await;
    if !auto_sync {
        debug!("Auto-sync is disabled in settings; skipping sync sweep");
    }

    let instances = if auto_sync { state.instances(true).await? } else { Vec::new() };
    let now = tokio::time::Instant::now();
    let mut synced_any = false;

    for instance in &instances {
        if !is_due(instance.sync_interval_minutes, last_sync.get(&instance.id).copied(), now) {
            continue;
        }

        match sync::sync_instance(state, &instance.id, "schedule").await {
            Ok(_) => {
                last_sync.insert(instance.id.clone(), now);
                synced_any = true;
            }
            // A conflict means a manual sync is already running — not an error.
            Err(crate::error::AppError::Conflict(_)) => {}
            Err(e) => error!("Scheduled sync of '{}' failed: {e}", instance.name),
        }
    }

    if synced_any {
        // Handed off, not awaited: the enrichment drains a whole backlog at a
        // paced source — hours, on a large anime library — and while the tick
        // waited for it no other instance synced on time, no backup ran and no
        // purge did. A chain still running from the last pass is left to
        // finish; it reads the library when it starts, so what this pass
        // synced is picked up by the next one.
        match chain {
            Some(running) if !running.is_finished() => {
                debug!("The previous pass's enrichment is still running; not starting another");
            }
            _ => {
                if let Some(finished) = chain.take() {
                    reap(state, finished).await;
                }
                *chain = Some(spawn_post_sync(state.clone()));
            }
        }
    }

    // A backup is worth taking on its own cadence: it protects against losing
    // the database, which has nothing to do with whether anything synced.
    if state.bool_setting("backup_enabled", true).await {
        let hours: u64 = state.setting("backup_interval_hours", 24u64).await.clamp(1, 24 * 7);
        let due = last_backup.is_none_or(|last| {
            tokio::time::Instant::now().duration_since(last) >= Duration::from_secs(hours * 3600)
        });
        if due {
            match backup::create(state, "schedule").await {
                // Stamped on an attempt that happened, not on one that worked.
                // Recorded on success alone, a backup failing on a full disk is
                // retried every tick — ninety-six `VACUUM INTO` a day against
                // SQLite's single writer, and ninety-six failed jobs on the
                // screen meant to show what needs attention. `sync` already
                // draws this line, with `last_sync_attempt_at` beside
                // `last_sync_at`.
                Ok(_) => *last_backup = Some(tokio::time::Instant::now()),
                // A conflict means one is already running: nothing was
                // attempted, so nothing is recorded and the next tick tries.
                Err(crate::error::AppError::Conflict(_)) => {}
                Err(e) => {
                    *last_backup = Some(tokio::time::Instant::now());
                    error!("Scheduled backup failed: {e}");
                }
            }
        }
    }

    // Housekeeping is cheap but pointless every tick. Measured in wall time,
    // not ticks: with a long scheduler interval, "every 4th tick" silently
    // became "every few days".
    let now = tokio::time::Instant::now();
    let maintenance_due =
        last_maintenance.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(3600));
    if maintenance_due {
        match maintenance::run(state, "schedule").await {
            Ok(_) => *last_maintenance = Some(now),
            // Same reason as the backup above: an hourly pass that fails must
            // not become a pass on every tick.
            Err(e) => {
                *last_maintenance = Some(now);
                error!("Scheduled maintenance failed: {e}");
            }
        }
    }

    Ok(())
}

/// Whether an instance is due for a scheduled sync.
///
/// Pure so the per-instance interval contract is testable without spinning up
/// the polling loop: never synced → due; otherwise due once its own
/// `sync_interval_minutes` (clamped to [1, `MAX_SYNC_INTERVAL_MINUTES`]) has
/// elapsed.
fn is_due(
    interval_minutes: i64,
    last: Option<tokio::time::Instant>,
    now: tokio::time::Instant,
) -> bool {
    let minutes = interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES) as u64;
    let interval = Duration::from_secs(minutes * 60);
    last.is_none_or(|last| now.duration_since(last) >= interval)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn an_instance_never_synced_is_due_immediately() {
        assert!(is_due(15, None, Instant::now()));
    }

    #[tokio::test]
    async fn each_instance_follows_its_own_interval() {
        let now = Instant::now();
        let two_minutes_ago = now - Duration::from_secs(120);

        // Synced two minutes ago: the 1-minute instance is due again, the
        // 60-minute one is not — the column must actually drive the cadence.
        assert!(is_due(1, Some(two_minutes_ago), now));
        assert!(!is_due(60, Some(two_minutes_ago), now));
    }

    #[tokio::test]
    async fn a_nonsensical_interval_is_clamped_rather_than_honoured() {
        let now = Instant::now();
        let ninety_seconds_ago = now - Duration::from_secs(90);

        // 0 and negative intervals collapse to the 1-minute floor instead of
        // hammering the Arr on every tick.
        assert!(is_due(0, Some(ninety_seconds_ago), now));
        assert!(is_due(-5, Some(ninety_seconds_ago), now));
        // An absurdly large interval is capped at 24 h, so the instance still
        // syncs at least daily.
        let a_day_and_a_bit_ago = now - Duration::from_secs(25 * 3600);
        assert!(is_due(999_999, Some(a_day_and_a_bit_ago), now));
    }
}
