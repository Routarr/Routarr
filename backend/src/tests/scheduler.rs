//! What the scheduler decides to run, and what it declines to.
//!
//! This is the code that acts with nobody watching: every fifteen minutes it
//! syncs, enriches, simulates, **applies** (which writes to Radarr), backs up
//! and purges. A defect here is the one a user cannot see happening.
//!
//! Each test asserts on the `jobs` table, because every stage records itself
//! there with its trigger. That is the same evidence the Tasks screen
//! shows, so a test passing here means the screen would have shown it too.

use std::collections::HashMap;

use crate::jobs::scheduler;
use crate::services::backup;

use super::fake_arr::FakeArr;
use super::{TestApp, warning_messages};

/// Run one tick with fresh cadence state, as the loop does on its first pass,
/// and wait for the work it handed off, as the loop does before stopping.
async fn tick(app: &TestApp) {
    if let Some(chain) = tick_only(app).await {
        chain.await.expect("the post-sync chain must not panic");
    }
}

async fn tick_only(app: &TestApp) -> Option<tokio::task::JoinHandle<()>> {
    scheduler::tick(&app.state, &mut HashMap::new(), &mut None, &mut None)
        .await
        .expect("the tick itself must not fail");
    app.state.post_sync.lock().await.take()
}

/// The jobs recorded by the scheduler, by kind.
async fn scheduled_jobs(app: &TestApp) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT kind FROM jobs WHERE trigger = 'schedule' ORDER BY started_at, kind",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

/// A library that will actually sync, with backups off: taking one needs a
/// database on disk, and what belongs here is the *decision* to take it, which
/// its own test covers.
async fn ready(arr: &FakeArr) -> TestApp {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.store_setting("backup_enabled", "false").await;
    app
}

// ------------------------------------------------------------ the sweep

#[tokio::test]
async fn a_tick_syncs_the_library_and_runs_the_housekeeping() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;

    tick(&app).await;

    let kinds = scheduled_jobs(&app).await;
    assert!(kinds.contains(&"sync".to_string()), "nothing synced: {kinds:?}");
    // Maintenance is unconditional, the rest follows from having synced.
    assert!(kinds.contains(&"maintenance".to_string()), "housekeeping was skipped: {kinds:?}");

    let media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(media, 1, "the library was not written");
}

/// Turning background syncs off must not silently turn the purges off with
/// them: retention has nothing to do with syncing.
#[tokio::test]
async fn disabling_auto_sync_still_leaves_housekeeping_running() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    app.store_setting("auto_sync_enabled", "false").await;

    tick(&app).await;

    let kinds = scheduled_jobs(&app).await;
    assert!(!kinds.contains(&"sync".to_string()), "auto-sync was off and it synced: {kinds:?}");
    assert!(
        kinds.contains(&"maintenance".to_string()),
        "disabling sync also disabled housekeeping: {kinds:?}"
    );
}

/// Enrichment and simulation are work proportional to the library, and
/// running them on an unchanged one every quarter of an hour is pure waste:
/// with the syncs switched off, or with every one failing, nothing follows.
/// `a_tick_hands_its_post_sync_work_back_rather_than_awaiting_it` is the
/// control, a tick that synced and handed the work on.
#[tokio::test]
async fn nothing_downstream_runs_when_nothing_synced() {
    let arr = FakeArr::start().await;
    let switched_off = ready(&arr).await;
    switched_off.store_setting("auto_sync_enabled", "false").await;
    let failing = TestApp::new().await;
    failing.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    failing.store_setting("backup_enabled", "false").await;

    for app in [&switched_off, &failing] {
        assert!(tick_only(app).await.is_none(), "work followed a tick that synced nothing");
    }
    assert_eq!(failing.last_job_status("sync").await, "failed", "the sync was never tried");
}

#[tokio::test]
async fn disabling_auto_simulate_stops_at_the_sync() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    app.store_setting("auto_simulate_enabled", "false").await;

    tick(&app).await;

    assert!(scheduled_jobs(&app).await.contains(&"sync".to_string()));
    let decisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(decisions, 0, "auto-simulate was off and it simulated anyway");
}

// ------------------------------------------------- failure does not cascade

/// An instance that is down is tried again at its own interval, not on every
/// tick: each attempt costs a full connect timeout, in a loop that syncs the
/// other instances after it.
#[tokio::test]
async fn a_failing_instance_waits_its_interval_before_the_next_attempt() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.store_setting("backup_enabled", "false").await;
    let mut last_sync = HashMap::new();

    for _ in 0..2 {
        scheduler::tick(&app.state, &mut last_sync, &mut None, &mut None)
            .await
            .expect("the tick itself must not fail");
    }

    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'sync' AND trigger = 'schedule'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1, "the instance that is down was tried on every tick");
}

/// An unreachable Arr is the ordinary case: a NAS asleep, a container
/// restarting. The tick has to survive it and still do the rest, or one dead
/// instance quietly stops the backups and the purges for everybody.
#[tokio::test]
async fn a_failing_sync_does_not_cancel_the_rest_of_the_tick() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.store_setting("backup_enabled", "false").await;

    tick(&app).await;

    let kinds = scheduled_jobs(&app).await;
    assert!(
        kinds.contains(&"maintenance".to_string()),
        "a dead instance stopped the housekeeping: {kinds:?}"
    );

    let failed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'sync' AND status = 'failed'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(failed, 1, "the failure was not recorded for the user to see");
}

#[tokio::test]
async fn a_disabled_instance_is_left_alone() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    sqlx::query("UPDATE instances SET enabled = 0").execute(&app.state.pool).await.unwrap();

    tick(&app).await;

    assert!(arr.recorded().api_keys.is_empty(), "a disabled instance was contacted");
    let media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(media, 0);
}

// ------------------------------------------------------------- guardrails

/// A library where a move is genuinely possible: an unimported film, a rule that
/// matches it, and a mapped destination that is not where it already sits.
///
/// Without this, "nothing was written" is true of any tick, since there would
/// be nothing to write, and the guardrail tests below would pass whatever the
/// guardrails did.
async fn ready_to_apply(arr: &FakeArr) -> TestApp {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.store_setting("backup_enabled", "false").await;

    // Sync first so the root folders exist to be mapped. The mapping is the
    // user's and the sync preserves it, so the tick's own sync will not undo it.
    crate::services::sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::manual(None),
    )
    .await
    .unwrap();

    sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-anime', 'anime')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE root_folders SET category = 'anime' WHERE path = '/movies/kids'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('r-1', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]', 'anime', 'all')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    app
}

/// The control that gives the two tests below their meaning: with both switches
/// thrown, the scheduler really does write to the Arr on its own. The write is
/// recorded as the schedule's and nobody's: "who moved this" never names a
/// person who was not there.
#[tokio::test]
async fn the_scheduler_applies_when_both_switches_allow_it() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = ready_to_apply(&arr).await;
    app.store_setting("global_dry_run", "false").await;
    app.store_setting("auto_apply_enabled", "true").await;

    tick(&app).await;

    assert!(
        !arr.recorded().writes.is_empty(),
        "the fixture cannot produce a write, so the guardrail tests would prove nothing"
    );
    let job: (String, Option<String>) =
        sqlx::query_as("SELECT trigger, subject FROM jobs WHERE kind = 'apply'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(job, ("auto".into(), None));
    let logged: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT actor, subject FROM execution_logs")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(logged, [(Some("auto".into()), None)]);
}

/// The scheduler is the path that can write without anyone asking, so the
/// guardrail that stops it has to hold from here too, not only from the button.
#[tokio::test]
async fn the_scheduler_writes_nothing_while_dry_run_holds() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = ready_to_apply(&arr).await;
    app.store_setting("global_dry_run", "true").await;
    app.store_setting("auto_apply_enabled", "true").await;

    tick(&app).await;

    // A move *was* proposed (the same fixture writes one when allowed to), and
    // dry-run is the only thing that stopped it.
    let proposed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE action = 'move'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(proposed > 0, "nothing was proposed, so nothing was withheld");
    assert!(
        arr.recorded().writes.is_empty(),
        "dry-run was on and the scheduler wrote to the Arr anyway"
    );
}

#[tokio::test]
async fn automatic_application_stays_opt_in() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = ready_to_apply(&arr).await;
    app.store_setting("global_dry_run", "false").await;
    // `auto_apply_enabled` left at its default, which is off.

    tick(&app).await;

    let proposed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE action = 'move'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(proposed > 0, "nothing was proposed, so nothing was withheld");
    assert!(
        arr.recorded().writes.is_empty(),
        "live mode alone was enough to make the scheduler write"
    );
}

// ---------------------------------------------------------------- backups

#[tokio::test]
async fn a_backup_is_decided_on_its_own_cadence() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    // Nothing to sync from here would still leave the backup due: it protects
    // against losing the database, which has nothing to do with syncing.
    app.store_setting("auto_sync_enabled", "false").await;
    app.store_setting("backup_enabled", "true").await;

    tick(&app).await;

    assert!(
        scheduled_jobs(&app).await.contains(&"backup".to_string()),
        "a backup was due and the scheduler did not attempt one"
    );
}

/// The interval counts from the newest archive on disk, not from the start of
/// the process: a restart taking one at once would prune a good archive, and a
/// few restarts after a bad change would leave only copies of it.
#[tokio::test]
async fn a_restart_takes_no_backup_before_the_interval_since_the_last_one() {
    let dir = super::TempDir::new("backup-cadence");
    let mut config = crate::config::Config::for_tests();
    config.data_dir = dir.to_path_buf();
    let app = TestApp::around(crate::state::AppState::for_tests().await.with_config(config));
    app.store_setting("auto_sync_enabled", "false").await;
    app.store_setting("backup_enabled", "true").await;
    let backups = backup::backup_dir(&app.state);
    std::fs::create_dir_all(&backups).unwrap();
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    std::fs::write(backups.join(format!("routarr-backup-{stamp}.zip")), b"taken before").unwrap();

    tick(&app).await;

    assert!(
        !scheduled_jobs(&app).await.contains(&"backup".to_string()),
        "a restart took a backup the interval did not call for"
    );
}

#[tokio::test]
async fn backups_disabled_means_none_is_attempted() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;

    tick(&app).await;

    assert!(!scheduled_jobs(&app).await.contains(&"backup".to_string()));
    assert!(backup::list(&app.state).is_empty());
}

// ------------------------------------------------------------- the loop itself
//
// The tests above exercise one `tick`. These two are about the loop around it,
// which is what actually runs unattended, and whose two failure modes are both
// silent: a panic that ends every future sweep, and a task still writing when
// the pool closes.

/// A panic inside a sweep must not end the scheduler.
///
/// Without the guard the task unwinds, nothing holds its `JoinHandle`, and
/// syncs, backups and retention purges stop for good while the HTTP server
/// keeps answering. Nothing in the interface would say so. Driven through
/// `start` itself, with the waits shortened: a test of `catch_unwind` alone
/// would prove a standard library function and nothing about this loop.
#[tokio::test]
async fn a_panicking_sweep_does_not_end_the_scheduler() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    // The floor is the wait: a setting of zero minutes leaves only it.
    app.store_setting("scheduler_interval_minutes", "0").await;
    scheduler::PANIC_NEXT_TICK.with(|flag| flag.set(true));

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let timings = scheduler::Timings {
        settle: std::time::Duration::from_millis(10),
        floor: std::time::Duration::from_millis(50),
        ..Default::default()
    };
    let handle = scheduler::start_with(app.state.clone(), stopped, timings);

    // The incident is recorded, and the pass after it runs: a sync appears.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let rows: Vec<(String, String, Option<String>)> =
                sqlx::query_as("SELECT kind, status, detail FROM jobs WHERE trigger = 'schedule'")
                    .fetch_all(&app.state.pool)
                    .await
                    .unwrap();
            let panicked = rows.iter().any(|(kind, status, error)| {
                kind == "scheduler"
                    && status == "failed"
                    && error.as_deref().unwrap_or_default().contains("a sweep blew up")
            });
            let synced = rows.iter().any(|(kind, _, _)| kind == "sync");
            if panicked && synced {
                return rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(outcome.is_ok(), "the loop did not record the panic and carry on");

    stop.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("the scheduler did not stop")
        .expect("the scheduler task panicked");
}

/// The enrichment drains a whole backlog at a paced source, which on a large
/// anime library takes hours. Awaited by the tick, it would keep every other
/// instance from syncing, hold back the backup and the purge, and make
/// `sync_interval_minutes` mean nothing. The chain runs on a task of its own,
/// which the tick hands back and the loop keeps, so shutdown still waits for
/// it.
#[tokio::test]
async fn a_tick_hands_its_post_sync_work_back_rather_than_awaiting_it() {
    let arr = FakeArr::start().await;
    // TMDb answers the library's one film slowly, so the enrichment is still
    // running when a tick that hands it off returns.
    let tmdb = super::fake_tmdb::FakeTmdb::with(vec![], vec![8392]).await;
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    let app = ready(&arr).await;
    let app = TestApp::around(app.state.clone().with_config(config));

    let chain = tick_only(&app).await;
    let kinds = scheduled_jobs(&app).await;
    assert!(kinds.contains(&"sync".to_string()), "nothing synced: {kinds:?}");
    let chain = chain.expect("a tick that synced hands back the work that follows");
    assert!(!chain.is_finished(), "the tick waited for the enrichment before returning");
    chain.await.unwrap();
    let decisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(decisions > 0, "the chain did not simulate");
}

/// The chain reads the library when it starts, so one still running when the
/// next sync ends picks up what that sync wrote, and none starts beside it:
/// two would drain the same backlog at the same paced sources.
#[tokio::test]
async fn a_sync_ending_while_its_chain_runs_starts_no_second_one() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    let running = tokio::spawn(std::future::pending::<()>());
    let id = running.id();
    *app.state.post_sync.lock().await = Some(running);

    // Bounded: waiting on the running chain never ends.
    let followed = std::time::Duration::from_secs(5);
    let by = crate::jobs::Attribution::unattended("schedule");
    let started = tokio::time::timeout(followed, scheduler::follow_sync(&app.state, &by));
    assert!(started.await.is_ok(), "the sync waited for the running chain");

    let kept = app.state.post_sync.lock().await.take().expect("the running chain was dropped");
    assert_eq!(kept.id(), id, "another chain took the place of the running one");
    kept.abort();
}

/// A chain that panicked ends in its `JoinHandle` and nowhere else, so the
/// next sync it would have followed records the panic where the Tasks screen
/// reads, then starts its own.
#[tokio::test]
async fn a_chain_that_panicked_is_recorded_when_the_next_one_starts() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    let blew_up = tokio::spawn(async { panic!("the chain blew up") });
    while !blew_up.is_finished() {
        tokio::task::yield_now().await;
    }
    let id = blew_up.id();
    *app.state.post_sync.lock().await = Some(blew_up);

    scheduler::follow_sync(&app.state, &crate::jobs::Attribution::unattended("schedule")).await;

    let detail: Option<String> = sqlx::query_scalar(
        "SELECT detail FROM jobs WHERE kind = 'scheduler' AND status = 'failed'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(detail.unwrap_or_default().contains("the chain blew up"), "the panic went unrecorded");
    let next = app.state.post_sync.lock().await.take().expect("no chain started");
    assert_ne!(next.id(), id);
    next.await.expect("the new chain must not panic");
}

/// Asking the scheduler to stop must actually stop it, and quickly.
///
/// `main` closes the pool right after this returns, and
/// `wal_checkpoint(TRUNCATE)` cannot truncate a write-ahead log another
/// connection is still writing to.
#[tokio::test]
async fn the_scheduler_stops_when_asked() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let handle = crate::jobs::scheduler::start(app.state.clone(), stopped);

    // Inside the five-second settling wait, which is where a shutdown most
    // often lands: a container is stopped far more often than it is left to
    // reach its first sweep.
    stop.send(true).unwrap();

    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(
        ended.is_ok(),
        "the scheduler was still running five seconds after being asked to stop"
    );
}

/// Between two passes the loop waits for as long as the setting says, an hour
/// here, and a stop arriving then ends it at once rather than after the wait.
#[tokio::test]
async fn the_scheduler_stops_between_two_passes_when_asked() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;
    app.store_setting("scheduler_interval_minutes", "60").await;

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let timings =
        scheduler::Timings { settle: std::time::Duration::from_millis(10), ..Default::default() };
    let handle = scheduler::start_with(app.state.clone(), stopped, timings);
    let first_pass = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !scheduled_jobs(&app).await.contains(&"maintenance".to_string()) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(first_pass.is_ok(), "the first pass never ended");

    stop.send(true).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("the scheduler waited out the hour")
        .expect("the scheduler task panicked");
}

/// The warning counts the passes that failed in the last day: one older says
/// nothing about whether it is still happening.
#[tokio::test]
async fn a_pass_that_failed_more_than_a_day_ago_is_not_counted() {
    let app = TestApp::new().await;
    app.execute(&["INSERT INTO jobs (id, kind, status, trigger, started_at)
           VALUES ('j-old', 'scheduler', 'failed', 'schedule', datetime('now', '-25 hours')),
                  ('j-new', 'scheduler', 'failed', 'schedule', datetime('now', '-23 hours'))"])
        .await;

    let status = app.get("/api/v1/status").await;
    let warnings = warning_messages(status.assert_ok());

    let one = app.state.localizer().await.translate("WarnSchedulerPanicked", &[("count", "1")]);
    assert!(warnings.contains(&one), "{warnings:?}");
}

/// A panicked pass has to leave evidence where somebody will find it.
///
/// The loop carries on, which is right, since stopping would end every sync,
/// backup and purge. But carrying on quietly is its own failure. The incident
/// lands in the `jobs` table, where the Tasks screen reads, and the diagnostics
/// badge counts it.
#[tokio::test]
async fn a_panicked_pass_is_visible_without_reading_the_log() {
    let arr = FakeArr::start().await;
    let app = ready(&arr).await;

    // Nothing to see before it happens.
    let quiet = app.get("/api/v1/status").await;
    let before = quiet.assert_ok()["warnings"].as_array().unwrap().len();

    crate::jobs::scheduler::record_panic(&app.state, "a sweep blew up").await;

    let after = app.get("/api/v1/status").await;
    let after = after.assert_ok();
    let warnings = warning_messages(after);
    assert_eq!(warnings.len(), before + 1, "the badge did not count it: {warnings:?}");
    assert!(
        warnings.iter().any(|w| w.contains("background pass")),
        "the warning does not say what happened: {warnings:?}"
    );

    // And the diagnostics page shows the same thing, since both read one list.
    let health = app.get("/api/v1/health?probe=false").await;
    let health = health.assert_ok();
    assert!(
        warning_messages(health).iter().any(|w| w.contains("background pass")),
        "the two lists disagree"
    );
}
