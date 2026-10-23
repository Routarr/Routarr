//! Whether a move the Arr took was made: Radarr and Sonarr answer a move with
//! files once they have queued it, and carry the files in a command of their
//! own.

use std::time::Duration;

use crate::jobs::Attribution;
use crate::services::{executor, sync};

use super::fake_arr::FakeArr;
use super::{TestApp, one_move_ready};

/// Totoro's folder before the move, and where the move puts it.
const BEFORE: &str = "/movies/standard/My Neighbor Totoro (1988)";
const AFTER: &str = "/movies/anime/My Neighbor Totoro (1988)";

/// Longer than `CEILING_ROUNDS` rounds of a 1 ms `move_wait`, so the run stops waiting.
const STALLED: Duration = Duration::from_millis(800);

async fn apply_with_files(app: &TestApp, decision_id: &str) -> executor::ApplyReport {
    executor::apply_decisions(
        &app.state,
        &[decision_id.to_string()],
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap()
}

/// The decision's status and its error, as an English reader reads it.
async fn decision(app: &TestApp, decision_id: &str) -> (String, Option<String>) {
    let (status, stored): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_message FROM decisions WHERE id = ?")
            .bind(decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let english = crate::localization::Localizer::new("en");
    (status, stored.map(|stored| crate::localization::said(&stored, &english)))
}

async fn totoro_path(app: &TestApp) -> String {
    sqlx::query_scalar("SELECT current_path FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn sync(app: &TestApp) {
    sync::sync_instance(&app.state, "inst-1", &Attribution::manual(None)).await.unwrap();
}

/// Every row a move with files writes waits for the disk: under the pool's `synchronous =
/// NORMAL` a power cut can roll back the record of a move the Arr made.
#[tokio::test]
async fn every_record_of_a_move_with_files_is_written_durably() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    app.execute(&[
        // As `db::init_pool` sets it, where the test pool keeps SQLite's FULL.
        "PRAGMA synchronous = NORMAL",
        "CREATE TEMP TABLE written (tbl TEXT, level INTEGER)",
        "CREATE TEMP TRIGGER durable_decisions AFTER UPDATE ON decisions BEGIN
             INSERT INTO written SELECT 'decisions', synchronous FROM pragma_synchronous;
         END",
        "CREATE TEMP TRIGGER durable_media AFTER UPDATE ON media BEGIN
             INSERT INTO written SELECT 'media', synchronous FROM pragma_synchronous;
         END",
        "CREATE TEMP TRIGGER durable_logs AFTER INSERT ON execution_logs BEGIN
             INSERT INTO written SELECT 'execution_logs', synchronous FROM pragma_synchronous;
         END",
    ])
    .await;

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!(report.applied, 1, "{report:?}");
    let written: Vec<(String, i64)> =
        sqlx::query_as("SELECT tbl, level FROM written").fetch_all(&app.state.pool).await.unwrap();
    assert!(written.len() >= 3, "{written:?}");
    let normal: Vec<_> = written.iter().filter(|(_, level)| *level != 2).collect();
    assert!(normal.is_empty(), "written under NORMAL: {normal:?}");
}

/// The move fails on disk and the Arr puts the old path back.
#[tokio::test]
async fn a_move_the_arr_rolls_back_is_recorded_as_failed() {
    let arr = FakeArr::start().await;
    arr.rolling_back(10);
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.failed), (0, 1), "{report:?}");
    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed");
    let reason = reason.unwrap_or_default();
    assert!(reason.contains("Fake radarr") && reason.contains(BEFORE), "{reason}");
    assert_eq!(totoro_path(&app).await, BEFORE, "the row kept a move the Arr undid");
}

/// The Arr records the new path but leaves the files where they were.
#[tokio::test]
async fn a_move_whose_command_fails_is_recorded_as_failed() {
    let arr = FakeArr::start().await;
    arr.failing_the_move_of(10);
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.failed), (0, 1), "{report:?}");
    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed");
    assert!(reason.unwrap_or_default().contains("Access to the path is denied"));
}

/// Both Arrs skip a move whose source folder they cannot see, keep the new path and end the
/// command completed, and their next scan drops the title's file records.
#[tokio::test]
async fn a_move_whose_folder_the_arr_cannot_see_is_not_recorded_as_moved() {
    let arr = FakeArr::start().await;
    arr.missing_source(10);
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.failed), (0, 1), "{report:?}");
    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed");
    let reason = reason.unwrap_or_default();
    assert!(reason.contains("Fake radarr cannot see") && reason.contains(BEFORE), "{reason}");
    assert!(arr.recorded().writes.is_empty(), "{:?}", arr.recorded().query_strings);
    assert_eq!(totoro_path(&app).await, BEFORE);
}

/// A title with no file yet has no folder to see, and moves.
#[tokio::test]
async fn a_title_without_files_moves_whatever_the_arr_sees_of_its_folder() {
    let arr = FakeArr::with_unimported_movie().await;
    arr.missing_source(10);
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.failed), (1, 0), "{report:?}");
    assert_eq!(totoro_path(&app).await, AFTER);
}

#[tokio::test]
async fn a_move_still_running_when_the_run_stops_waiting_is_confirmed_by_the_next_sync() {
    let arr = FakeArr::start().await;
    arr.moving_for(STALLED);
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_move_wait(Duration::from_millis(1));

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.moving), (0, 1), "{report:?}");
    assert_eq!(report.stopped, Some(executor::StopReason::Stalled));
    assert_eq!(decision(&app, &decision_id).await.0, "requested");
    assert_eq!(totoro_path(&app).await, AFTER);

    sync(&app).await;
    assert_eq!(decision(&app, &decision_id).await.0, "requested", "settled while still moving");

    tokio::time::sleep(STALLED).await;
    sync(&app).await;
    assert_eq!(decision(&app, &decision_id).await.0, "applied");
}

#[tokio::test]
async fn a_requested_move_the_arr_undid_is_failed_by_the_next_sync() {
    let arr = FakeArr::start().await;
    arr.moving_for(STALLED);
    arr.rolling_back(10);
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_move_wait(Duration::from_millis(1));
    apply_with_files(&app, &decision_id).await;
    tokio::time::sleep(STALLED).await;

    sync(&app).await;

    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed", "{reason:?}");
    assert!(reason.unwrap_or_default().contains(BEFORE));
    assert_eq!(totoro_path(&app).await, BEFORE);
}

#[tokio::test]
async fn a_requested_move_whose_title_is_gone_is_failed_by_the_next_sync() {
    let arr = FakeArr::start().await;
    arr.moving_for(STALLED);
    let app = TestApp::films_to_move(&arr, 1).await.with_move_wait(Duration::from_millis(1));
    app.simulate().await;
    let decision_id: String =
        sqlx::query_scalar("SELECT id FROM decisions WHERE media_id = 'm-0' AND superseded = 0")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    apply_with_files(&app, &decision_id).await;
    tokio::time::sleep(STALLED).await;
    arr.forget_movie(100);

    sync(&app).await;

    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed", "{reason:?}");
    assert!(reason.unwrap_or_default().contains("no longer holds"));
}

/// Once the Arr holds the title back where it came from, the decision reads `skipped`.
#[tokio::test]
async fn a_requested_revert_is_settled_by_the_next_sync() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_move_wait(Duration::from_millis(1));
    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    arr.moving_for(STALLED);

    let reverted = executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(reverted.moving, 1, "{reverted:?}");
    assert_eq!(decision(&app, &decision_id).await.0, "requested");
    tokio::time::sleep(STALLED).await;
    sync(&app).await;
    assert_eq!(decision(&app, &decision_id).await.0, "skipped");
    assert_eq!(totoro_path(&app).await, BEFORE);
}

/// A crash between the Arr's answer and the record then leaves a move the next sync settles.
#[tokio::test]
async fn a_move_is_recorded_as_requested_before_it_is_sent() {
    // Within the outbound timeout of `Config::for_tests`.
    let arr = FakeArr::holding_edits(Duration::from_millis(100)).await;
    let (app, decision_id) = one_move_ready(&arr).await;
    let (confirmed, by) = (executor::Confirmed::all(), Attribution::manual(None));

    let applying = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &confirmed,
        &by,
    );
    let watching = async {
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            while arr.recorded().writes.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        reached.await.expect("the move never reached the Arr");
        decision(&app, &decision_id).await.0
    };
    let (applied, while_sending) = tokio::join!(applying, watching);

    assert_eq!(while_sending, "requested");
    assert_eq!(applied.unwrap().applied, 1);
    assert_eq!(decision(&app, &decision_id).await.0, "applied");
}

/// A move whose answer never came may have been made all the same.
#[tokio::test]
async fn a_move_answered_after_the_timeout_is_recorded_from_what_the_arr_holds() {
    let arr = FakeArr::start().await;
    arr.answering_updates_after(Duration::from_millis(400));
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_http_budget(Duration::from_millis(150));

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (1, 0), "{report:?}");
    assert_eq!(decision(&app, &decision_id).await.0, "applied");
    assert_eq!(totoro_path(&app).await, AFTER);
}

/// The upgraded table also accepts the `requested` status.
#[tokio::test]
async fn an_upgrade_keeps_every_decision_and_its_log() {
    let pool = super::database_of_0_1_5().await;
    for statement in [
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                target_category, action, status)
         VALUES ('d-1', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move', 'applied')",
        "INSERT INTO execution_logs (id, decision_id, action, success)
         VALUES ('l-1', 'd-1', 'move', 1)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    let kept: (String, String) = sqlx::query_as(
        "SELECT d.status, l.id FROM decisions d JOIN execution_logs l ON l.decision_id = d.id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept, ("applied".to_string(), "l-1".to_string()));
    sqlx::query("UPDATE decisions SET status = 'requested' WHERE id = 'd-1'")
        .execute(&pool)
        .await
        .expect("a decision cannot be requested");
}
