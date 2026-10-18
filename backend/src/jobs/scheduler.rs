//! Background scheduler. Each instance is polled on its own `sync_interval_minutes`.

use futures::FutureExt;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use crate::jobs::{Attribution, Detail, FULL_SIMULATION, JobKind, TRIGGER_SCHEDULE};
use crate::services::{auto_apply, backup, enrichment, maintenance, routing, sync};
use crate::state::AppState;

/// How long the loop waits before its first pass and at least between two. Kept apart so a
/// test can run the loop in milliseconds.
pub struct Timings {
    /// Before the first pass, so the server finishes binding first.
    pub settle: Duration,
    /// The least a pass waits for the next, whatever the setting says.
    pub floor: Duration,
    /// How long a shutdown waits for the post-sync work. Shorter than the
    /// wait `main` gives the scheduler, so the pool is never closed under it.
    pub grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            settle: Duration::from_secs(5),
            floor: Duration::from_secs(60),
            grace: Duration::from_secs(5),
        }
    }
}

// Panics the next pass once. Thread-local: a global flag would fire in another test's pass.
#[cfg(test)]
thread_local! {
    pub(crate) static PANIC_NEXT_TICK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Start the polling loop and return the handle a shutdown waits on. A panic must not end the
/// loop, or syncs, backups and purges stop for good while the server looks healthy. `main`
/// closes the pool after the wait, and a checkpoint cannot truncate under a running pass.
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

        loop {
            let pass = AssertUnwindSafe(tick(
                &state,
                &mut last_sync,
                &mut last_maintenance,
                &mut last_backup,
            ))
            .catch_unwind()
            .await;

            match pass {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("Scheduler tick failed: {e}"),
                // Recorded as a job so the Tasks screen and the warnings show it.
                Err(panic) => record_panic(&state, &describe_panic(panic)).await,
            }

            let minutes: u64 = state.setting::<u64>("scheduler_interval_minutes").await;
            let wait = Duration::from_secs(minutes.min(24 * 60) * 60).max(timings.floor);
            if wait_or_stop(&mut shutdown, wait).await {
                // Waited for whoever started it, since a sync route can start it too.
                let running = state.post_sync.lock().await.take();
                if let Some(running) = running {
                    reap_within(&state, running, timings.grace).await;
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
pub(crate) async fn record_panic(state: &AppState, cause: &str) {
    error!("Scheduler pass panicked and was restarted: {cause}");
    if let Ok(job) = state
        .jobs
        .start(
            JobKind::Scheduler,
            &Attribution::unattended(TRIGGER_SCHEDULE),
            None,
            Detail::new("JobDetailScheduledPass"),
        )
        .await
    {
        job.fail_with(Detail::new("JobDetailPanicked").with("cause", cause)).await;
    }
}

/// Wait for the post-sync chain for `grace` at most, recording a panic as `reap` does. Past
/// the grace the chain is aborted, or it writes into the pool `main` closes next. Its job is
/// marked interrupted at the next start (`JobRegistry::recover_orphans`).
async fn reap_within(state: &AppState, mut chain: JoinHandle<()>, grace: Duration) {
    match tokio::time::timeout(grace, &mut chain).await {
        Ok(Err(e)) if e.is_panic() => {
            record_panic(state, &format!("post-sync work: {}", describe_panic(e.into_panic())))
                .await;
        }
        Ok(_) => {}
        Err(_) => {
            chain.abort();
            warn!("The post-sync work was still running at shutdown, and was stopped");
        }
    }
}

/// Wait for a finished chain, and say so if it panicked: a spawned task's
/// panic ends in its `JoinHandle` and nowhere else.
async fn reap(state: &AppState, chain: JoinHandle<()>) {
    if let Err(e) = chain.await
        && e.is_panic()
    {
        record_panic(state, &format!("post-sync work: {}", describe_panic(e.into_panic()))).await;
    }
}

/// Start what follows a full sync unless the last one is still running: the next one picks
/// up what this sync wrote. Two never run at once, since each drains the same backlog at the
/// same paced sources.
pub async fn follow_sync(state: &AppState, by: &Attribution) {
    let mut slot = state.post_sync.lock().await;
    match slot.as_ref() {
        Some(running) if !running.is_finished() => {
            debug!("The work after the last sync is still running, not starting another");
        }
        _ => {
            if let Some(finished) = slot.take() {
                reap(state, finished).await;
            }
            *slot = Some(spawn_post_sync(state.clone(), Attribution::automatic(by)));
        }
    }
}

/// What follows a sync: enrich what is new, simulate, and apply what the
/// unattended guardrails allow. On a task of its own, for the reason `tick`
/// gives, attributed to the automation and to whoever set the sync off.
fn spawn_post_sync(state: AppState, by: Attribution) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Before the enrichment, which can take hours at a paced source.
        if let Err(e) = crate::services::rule_tests::check(&state).await {
            error!("The rule tests after a sync failed to run: {e}");
        }
        if let Err(e) = enrichment::enrich_all_media(&state, &by).await {
            error!("Enrichment after a sync failed: {e}");
        }

        // Locked against a full pass from the interface. The webhook's one-item run must not lock.
        if state.bool_setting("auto_simulate_enabled").await
            && let Some(_pass) = state.jobs.try_lock(FULL_SIMULATION)
        {
            let Some(result) = simulate_after_sync(&state, &by).await else { return };
            // Catches what the webhook missed, under the same guardrails.
            if let Err(e) = auto_apply::apply_simulation(&state, &result.simulation_id, &by).await {
                error!("Auto-apply after a sync failed: {e}");
            }
        }
    })
}

/// The simulation that follows a sync, as a task the Tasks screen lists with
/// its outcome, its proposals left to `/decisions`.
async fn simulate_after_sync(
    state: &AppState,
    by: &Attribution,
) -> Option<crate::models::SimulationResult> {
    let started = state.jobs.start(JobKind::Simulate, by, None, Detail::new("JobDetailSimulating"));
    let mut job = match started.await {
        Ok(job) => job,
        Err(e) => {
            error!("The simulation after a sync could not start: {e}");
            return None;
        }
    };
    let options = routing::SimulationOptions {
        trigger: by.trigger.clone(),
        subject: by.subject.clone(),
        persist: true,
        // Only the counts are read here, and the proposals are worded once, to be stored.
        max_returned: Some(0),
        language: state.language().await,
        progress: Some(job.progress_reporter()),
        ..Default::default()
    };
    match routing::run_simulation(&state.pool, &state.library_passes, options).await {
        Ok(result) => {
            crate::services::notify::send_later(
                state,
                crate::services::notify::Event::SimulationCompleted {
                    simulation_id: result.simulation_id.clone(),
                    total: result.total_media,
                    moves: result.moves_required,
                },
            );
            if let Ok(summary) = serde_json::to_value(&result) {
                job.report(&summary);
            }
            let detail = Detail::new("JobDetailSimulated")
                .with("total", result.total_media)
                .with("moves", result.moves_required);
            job.succeed(detail).await;
            Some(result)
        }
        Err(e) => {
            error!("Simulation after a sync failed: {e}");
            job.fail(&e).await;
            None
        }
    }
}

/// Sleep unless asked to stop first, and return `true` to stop. Only the wait is
/// interruptible, so a shutdown mid-pass lets that pass finish.
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
) -> crate::error::AppResult<()> {
    #[cfg(test)]
    if PANIC_NEXT_TICK.with(|flag| flag.replace(false)) {
        panic!("a sweep blew up");
    }

    // Only the sync sweep is gated: turning syncs off must not turn the purges off too.
    let auto_sync = state.bool_setting("auto_sync_enabled").await;
    if !auto_sync {
        debug!("Auto-sync is disabled in settings, skipping sync sweep");
    }

    let instances = if auto_sync { state.instances(true).await? } else { Vec::new() };
    let now = tokio::time::Instant::now();
    let mut synced_any = false;

    for instance in &instances {
        if !is_due(instance.sync_interval_minutes, last_sync.get(&instance.id).copied(), now) {
            continue;
        }

        match sync::sync_instance(state, &instance.id, &Attribution::unattended(TRIGGER_SCHEDULE))
            .await
        {
            Ok(_) => {
                last_sync.insert(instance.id.clone(), now);
                synced_any = true;
            }
            // A manual sync is already running, which is not an error.
            Err(crate::error::AppError::InProgress { .. }) => {}
            // Stamped on the attempt, or a down instance costs a connect timeout every tick.
            Err(e) => {
                last_sync.insert(instance.id.clone(), now);
                error!("Scheduled sync of '{}' failed: {e}", instance.name);
            }
        }
    }

    if synced_any {
        // Not awaited: the enrichment can take hours, holding back syncs, backups and purges.
        follow_sync(state, &Attribution::unattended(TRIGGER_SCHEDULE)).await;
    }

    // On its own cadence: a backup guards the database whether or not anything synced.
    if state.bool_setting("backup_enabled").await {
        let hours: u64 = state.setting::<u64>("backup_interval_hours").await.clamp(1, 24 * 7);
        // Counted from the newest archive, or restarts after a bad change prune the good ones.
        if last_backup.is_none() {
            *last_backup = backup::newest_taken(state);
        }
        let due = last_backup.is_none_or(|last| {
            tokio::time::Instant::now().duration_since(last) >= Duration::from_secs(hours * 3600)
        });
        if due {
            match backup::create(state, &Attribution::unattended(TRIGGER_SCHEDULE)).await {
                // Stamped on every attempt, or a failing backup retries on every tick.
                Ok(_) => *last_backup = Some(tokio::time::Instant::now()),
                // One is already running: nothing was attempted, so the next tick tries.
                Err(crate::error::AppError::InProgress { .. }) => {}
                Err(e) => {
                    *last_backup = Some(tokio::time::Instant::now());
                    error!("Scheduled backup failed: {e}");
                    let failed =
                        crate::services::notify::Event::BackupFailed { error: e.to_string() };
                    crate::services::notify::send_later(state, failed);
                }
            }
        }
    }

    // Hourly in wall time: counted in ticks, a long interval would make it every few days.
    let now = tokio::time::Instant::now();
    let maintenance_due =
        last_maintenance.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(3600));
    if maintenance_due {
        match maintenance::run(state, &Attribution::unattended(TRIGGER_SCHEDULE)).await {
            Ok(_) => *last_maintenance = Some(now),
            // A requested purge is running and does the same, so the next tick tries.
            Err(crate::error::AppError::InProgress { .. }) => {}
            // Stamped too, so a failing hourly pass does not run on every tick.
            Err(e) => {
                *last_maintenance = Some(now);
                error!("Scheduled maintenance failed: {e}");
            }
        }
    }

    Ok(())
}

/// Whether an instance is due: never synced, or its own interval, clamped to
/// [1, `MAX_SYNC_INTERVAL_MINUTES`], has elapsed.
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

    /// A chain still running at shutdown is dropped once the grace is out:
    /// `main` closes the pool right after, and a chain left running would go
    /// on writing into it.
    #[tokio::test]
    async fn a_shutdown_waits_for_the_post_sync_work_no_longer_than_its_grace() {
        let state = AppState::for_tests().await;
        let chain = tokio::spawn(tokio::time::sleep(Duration::from_secs(10)));
        let stopped = chain.abort_handle();
        let started = std::time::Instant::now();

        reap_within(&state, chain, Duration::from_millis(50)).await;

        assert!(started.elapsed() < Duration::from_secs(5), "the shutdown waited for the chain");
        tokio::task::yield_now().await;
        assert!(stopped.is_finished(), "the chain was left running");
    }

    #[tokio::test]
    async fn an_instance_never_synced_is_due_immediately() {
        assert!(is_due(15, None, Instant::now()));
    }

    #[tokio::test]
    async fn each_instance_follows_its_own_interval() {
        let now = Instant::now();
        let two_minutes_ago = now - Duration::from_secs(120);

        // Synced two minutes ago: due on a 1-minute interval, not on a 60-minute one.
        assert!(is_due(1, Some(two_minutes_ago), now));
        assert!(!is_due(60, Some(two_minutes_ago), now));
    }

    #[tokio::test]
    async fn a_nonsensical_interval_is_clamped_rather_than_honoured() {
        let now = Instant::now();
        let ninety_seconds_ago = now - Duration::from_secs(90);

        // 0 and negative intervals collapse to the 1-minute floor.
        let thirty_seconds_ago = now - Duration::from_secs(30);
        assert!(!is_due(0, Some(thirty_seconds_ago), now));
        assert!(!is_due(-5, Some(thirty_seconds_ago), now));
        assert!(is_due(0, Some(ninety_seconds_ago), now));
        assert!(is_due(-5, Some(ninety_seconds_ago), now));
        // An absurdly large interval is capped at 24 h, so the instance still syncs daily.
        let a_day_and_a_bit_ago = now - Duration::from_secs(25 * 3600);
        assert!(is_due(999_999, Some(a_day_and_a_bit_ago), now));
    }
}
