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

async fn decision(app: &TestApp, decision_id: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error_message FROM decisions WHERE id = ?")
        .bind(decision_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
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

/// The Arr's move fails on disk and the Arr puts the old path back: the
/// apply reads the title back and records a failure naming the instance,
/// with the title where the Arr holds it.
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

/// A move command the Arr ends as failed leaves the new path recorded and the
/// files where they were: the decision fails with the Arr's own words.
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

/// A move still running when the apply stops waiting is recorded as
/// requested, the title where the Arr already holds it. The next sync, once
/// the Arr has finished, confirms it.
#[tokio::test]
async fn a_move_still_running_at_the_deadline_is_confirmed_by_the_next_sync() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(600));
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_move_wait(Duration::from_millis(150));

    let report = apply_with_files(&app, &decision_id).await;

    assert_eq!((report.applied, report.moving), (0, 1), "{report:?}");
    assert_eq!(decision(&app, &decision_id).await.0, "requested");
    assert_eq!(totoro_path(&app).await, AFTER);

    sync(&app).await;
    assert_eq!(decision(&app, &decision_id).await.0, "requested", "settled while still moving");

    tokio::time::sleep(Duration::from_millis(600)).await;
    sync(&app).await;
    assert_eq!(decision(&app, &decision_id).await.0, "applied");
}

/// The same, but the Arr undid the move after the apply stopped waiting: the
/// next sync reads the title outside its target and fails the decision.
#[tokio::test]
async fn a_requested_move_the_arr_undid_is_failed_by_the_next_sync() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(300));
    arr.rolling_back(10);
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_move_wait(Duration::from_millis(100));
    apply_with_files(&app, &decision_id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    sync(&app).await;

    let (status, reason) = decision(&app, &decision_id).await;
    assert_eq!(status, "failed", "{reason:?}");
    assert!(reason.unwrap_or_default().contains(BEFORE));
    assert_eq!(totoro_path(&app).await, BEFORE);
}

/// The decision says `requested` before the write leaves: a stop or a crash
/// between the Arr's answer and the record then leaves a move the next sync
/// settles from what the Arr holds, rather than one nobody recorded.
#[tokio::test]
async fn a_move_is_recorded_as_requested_before_it_is_sent() {
    // Within the tests' 300 ms budget for an answer.
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

/// A move whose answer never came may have been made all the same: the title
/// is read back, and recorded from where the Arr holds it.
#[tokio::test]
async fn a_move_answered_after_the_timeout_is_recorded_from_what_the_arr_holds() {
    let arr = FakeArr::start().await;
    arr.answering_updates_after(Duration::from_millis(400));
    let (app, decision_id) = one_move_ready(&arr).await;
    let app = app.with_http_budget(Duration::from_millis(150));

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id.clone()],
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

/// An upgrade keeps every decision and the log lines pointing at it, and
/// lets a decision be `requested`.
#[tokio::test]
async fn an_upgrade_keeps_every_decision_and_its_log() {
    let pool = super::database_through("015_certification_scale").await;
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
