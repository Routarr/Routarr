//! Applying and reverting decisions against a live fake Arr.

use crate::jobs::Attribution;
use crate::services::executor;
use crate::services::routing::{self, SimulationOptions};
use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// The client hanging up mid-apply — a browser navigating away, an Arr whose
/// webhook timed out — must not leave a move done at the Arr and unknown here.
/// Awaited in the request future, the work was dropped with the connection at
/// its next await: the Arr had moved the item, the decision stayed `pending`,
/// the row kept its old path, and the next pass proposed the move again.
#[tokio::test]
async fn hanging_up_mid_apply_still_records_the_move() {
    use std::time::Duration;

    // Long enough to hang up while the Arr is at work, and well under the
    // test client's own timeout, or the move fails on its own and proves
    // nothing about the hang-up.
    let arr = FakeArr::holding_edits(Duration::from_millis(100)).await;
    let (app, decision_id) = ready(&arr).await;

    // Driven until the Arr has the edit in hand, then dropped — which is what
    // a request future undergoes when its connection goes away.
    {
        let mut applying = Box::pin(app.post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": [decision_id.clone()], "move_files": false, "confirm": ["capacity", "threshold"] }),
        ));
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            while arr.recorded().writes.is_empty() {
                let _ = futures::poll!(applying.as_mut());
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(reached.is_ok(), "the edit never reached the Arr");
    }

    // Bounded, not timed: the work either finishes and records, or it was
    // dropped with the request and never will.
    let recorded = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM decisions WHERE id = ?")
                .bind(&decision_id)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
            if status == "applied" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(recorded.is_ok(), "the Arr moved the item and Routarr never recorded it");

    let path: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(path, "/movies/anime", "the row was not brought in line with the Arr");
    // Bounded, not immediate: the lock falls when the detached future ends,
    // which is after `job.succeed`, itself after the status this test has just
    // observed. Asserting on it straight away asserts an ordering the executor
    // never promised, and only holds on a machine fast enough to lose the race
    // by microseconds — which is why it passed everywhere but on a CI runner.
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        while app.state.jobs.try_lock("apply").is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(released.is_ok(), "the apply lock was left held");
    let job: String = sqlx::query_scalar(
        "SELECT status FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(job, "success", "the Tasks screen must see the job end");
}

/// A library wired to a fake Radarr, with dry-run off and one pending move.
/// A `?` between `start` and the outcome left the row `running` until the next
/// restart failed it as an orphan: the Tasks screen showed a job in progress,
/// with nothing to say why the apply had answered an error.
#[tokio::test]
async fn an_apply_that_cannot_load_its_moves_reports_a_failed_job() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    // The one column only the loader reads, so the guards before the job pass.
    sqlx::query("ALTER TABLE media DROP COLUMN current_path")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let outcome =
        executor::apply_unattended(&app.state, &[decision_id], &Attribution::manual(None)).await;
    assert!(outcome.is_err(), "the loader was meant to fail: {outcome:?}");

    let (status, error): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_message FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed", "the Tasks screen must see the job end");
    assert!(error.unwrap_or_default().contains("current_path"), "the failure names its cause");
    assert!(app.state.jobs.try_lock("apply").is_some(), "the apply lock was left held");
}

/// A decision names the folder the item was in when it was proposed. Moved
/// since — by hand in Radarr, or by an earlier apply the row already reflects —
/// the proposal describes an item that no longer exists, and sending it to the
/// Arr again is a second write nobody asked for.
#[tokio::test]
async fn a_decision_whose_item_has_moved_since_is_skipped_rather_than_reapplied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    sqlx::query(
        "UPDATE media SET current_root_folder = '/movies/anime',
                current_path = '/movies/anime/Totoro (1988)' WHERE id = 'm-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = executor::apply_unattended(&app.state, &[decision_id], &Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert!(arr.recorded().writes.is_empty(), "the Arr was written to for a stale decision");
}

async fn ready(arr: &FakeArr) -> (TestApp, String) {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_anime_rule().await;

    sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-2', 'inst-1', 2, '/movies/anime', 1, 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro', 8392,
                 '/movies/standard/Totoro (1988)', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, expires_at)
         VALUES ('tmdb', '8392', 'movie', '[\"Animation\"]', '[]', 'ja', '[]', '2099-01-01')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(result.moves_required, 1);
    let decision_id = result.decisions[0].id.clone();

    (app, decision_id)
}

#[tokio::test]
async fn applying_moves_the_media_and_records_it() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (1, 0));

    let write = arr.recorded().writes[0].clone();
    assert_eq!(write["rootFolderPath"], "/movies/anime");
    assert_eq!(write["moveFiles"], true);

    let (status, applied_at): (String, Option<String>) =
        sqlx::query_as("SELECT status, applied_at FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "applied");
    assert!(applied_at.is_some());
}

#[tokio::test]
async fn applying_rewrites_the_local_path_so_the_move_is_not_reproposed() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime");
    assert_eq!(path, "/movies/anime/Totoro (1988)", "the folder name must be preserved");

    // The whole point: a fresh simulation now agrees the media is in place.
    let again =
        routing::run_simulation(&app.state.pool, SimulationOptions::default()).await.unwrap();
    assert_eq!(again.moves_required, 0);
    assert_eq!(again.already_correct, 1);
}

#[tokio::test]
async fn applying_triggers_a_rescan() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let recorded = arr.recorded();
    assert!(
        recorded.writes.iter().any(|w| w["name"] == "RefreshMovie"),
        "the Arr must be told to rescan the new location"
    );
}

#[tokio::test]
async fn a_rescan_can_be_turned_off() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'refresh_after_move'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert!(!arr.recorded().writes.iter().any(|w| w["name"] == "RefreshMovie"));
}

#[tokio::test]
async fn an_upstream_failure_marks_the_decision_failed_and_logs_it() {
    let arr = FakeArr::failing(500).await;
    let (app, decision_id) = ready(&arr).await;

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (0, 1));
    assert_eq!(report.errors.len(), 1);

    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_message FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("500"));

    let (success, logged): (bool, i64) =
        sqlx::query_as("SELECT success, COUNT(*) FROM execution_logs")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(logged, 1);
    assert!(!success);

    // The library must be left describing reality, not the attempted move.
    let root: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(root, "/movies/standard");
}

#[tokio::test]
async fn a_superseded_decision_is_never_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    sqlx::query("UPDATE decisions SET superseded = 1").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped, 1);
    assert!(arr.recorded().writes.is_empty(), "nothing may be sent upstream");
}

#[tokio::test]
async fn reverting_puts_the_media_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    let report = executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (1, 0));

    let last_write =
        arr.recorded().writes.iter().rev().find(|w| w["rootFolderPath"].is_string()).cloned();
    assert_eq!(last_write.unwrap()["rootFolderPath"], "/movies/standard");

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/standard");
    assert_eq!(path, "/movies/standard/Totoro (1988)");

    let (status, reverted): (String, Option<String>) =
        sqlx::query_as("SELECT status, reverted_at FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "skipped");
    assert!(reverted.is_some());
}

#[tokio::test]
async fn a_decision_cannot_be_reverted_twice() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    let second =
        executor::revert_decisions(&app.state, &[decision_id], false, &Attribution::manual(None))
            .await
            .unwrap();

    assert_eq!(second.applied, 0);
    assert_eq!(second.skipped, 1);
}

#[tokio::test]
async fn a_pending_decision_cannot_be_reverted() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    let report =
        executor::revert_decisions(&app.state, &[decision_id], false, &Attribution::manual(None))
            .await
            .unwrap();

    assert_eq!(report.applied, 0);
    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn moves_to_the_same_folder_are_batched_into_one_call() {
    let arr = FakeArr::start().await;
    let (app, _) = ready(&arr).await;

    // A second movie on the same instance heading for the same folder.
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Akira', 8392,
                 '/movies/standard/Akira', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    let ids: Vec<String> = result.decisions.iter().map(|d| d.id.clone()).collect();
    assert_eq!(ids.len(), 2);

    let report = executor::apply_decisions(
        &app.state,
        &ids,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    assert_eq!(report.applied, 2);

    let recorded = arr.recorded();
    // The refresh command also carries `movieIds`, so match on the editor payload.
    let edits: Vec<_> =
        recorded.writes.iter().filter(|w| w["rootFolderPath"].is_string()).collect();
    assert_eq!(edits.len(), 1, "both movies share one editor call");
    assert_eq!(edits[0]["movieIds"].as_array().unwrap().len(), 2);
}

// ------------------------------------------------- the route, not the service
//
// Everything above calls `executor::` directly. Reverting is the one operation
// a user reaches for when something has already gone wrong, so it is worth
// knowing the route itself is wired — a handler that never receives the ids it
// is given fails in exactly the moment nobody wants a surprise.

#[tokio::test]
async fn the_revert_route_puts_the_media_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let body = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": [decision_id], "move_files": false }),
        )
        .await;
    let report = body.assert_ok();
    assert_eq!((report["applied"].as_i64(), report["failed"].as_i64()), (Some(1), Some(0)));

    let root: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(root, "/movies/standard");
}

#[tokio::test]
async fn the_revert_route_refuses_an_unknown_decision_without_a_panic() {
    let app = TestApp::new().await;
    let body = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": ["nope"], "move_files": false }),
        )
        .await;
    // A refusal that says so — not a 500, and not a success over nothing.
    assert_eq!(body.status, axum::http::StatusCode::BAD_REQUEST, "got {}", body.message());
    assert!(!body.message().is_empty(), "the refusal says nothing");
}

/// `apply` and `sync:{instance}` are different job locks, so an application and
/// a synchronisation of the same instance can run at once — and both write
/// `current_root_folder`. The losing interleaving is a sync that read the Arr
/// *before* the move landed and commits *after* it: it puts the old path back,
/// and the next simulation reproposes a move that already happened.
///
/// Made deterministic rather than raced: a sync whose read predates the move is
/// the same statement as a move stamped after the read. The fake still answers
/// `/movies/standard`, which is precisely the stale reply a real Arr gives while
/// the editor call is still in flight.
#[tokio::test]
async fn a_sync_that_read_before_the_move_does_not_put_the_old_path_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE media SET moved_at = datetime('now', '+1 minute') WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime", "the sync undid the move it did not know about");
    assert_eq!(path, "/movies/anime/Totoro (1988)");
}

/// The other half, and the one that stops the guard becoming a permanent veto:
/// once a read is newer than the move, the Arr is authoritative again. Someone
/// moving a film in Radarr's own interface must still be followed — Routarr
/// having moved it once is not a claim of ownership over it.
#[tokio::test]
async fn a_sync_that_read_after_the_move_still_follows_the_arr() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE media SET moved_at = datetime('now', '-1 minute') WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let root: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(root, "/movies/standard", "upstream stopped being authoritative");
}
