//! A run that moves titles stops before its next move when somebody cancels
//! it, when the global dry-run is turned on, or when the server stops. What
//! already reached the Arr stays done and is recorded.

use std::time::Duration;

use super::fake_arr::FakeArr;
use super::{TestApp, finished, preferring_async};

/// Three films, each held 100 ms at the Arr: long enough to act while the
/// first is in flight, within the tests' 300 ms budget for an answer.
async fn three_films() -> (FakeArr, TestApp, Vec<String>) {
    let arr = FakeArr::holding_edits(Duration::from_millis(100)).await;
    let app = TestApp::films_to_move(&arr, 3).await;
    app.simulate().await;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM decisions WHERE action = 'move' AND superseded = 0 ORDER BY media_title",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    (arr, app, ids)
}

/// Start an apply of `ids` without waiting for it, and answer its task's id
/// once the first move has reached the Arr.
async fn applying(app: &TestApp, arr: &FakeArr, ids: &[String]) -> String {
    let body = serde_json::json!({
        "decision_ids": ids, "move_files": false, "confirm": ["capacity", "threshold"]
    });
    let started = app.send(preferring_async("/api/v1/decisions/apply", body)).await;
    assert_eq!(started.status, 202, "{}", started.json);
    let reached = tokio::time::timeout(Duration::from_secs(5), async {
        while arr.recorded().writes.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    reached.await.expect("the first move never reached the Arr");
    started.json["job_id"].as_str().unwrap().to_string()
}

async fn statuses(app: &TestApp) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT status FROM decisions WHERE action = 'move' AND superseded = 0 ORDER BY media_title",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

/// A cancel stops the apply before its next move: the move in flight is
/// recorded, the rest stays pending, and the task reads cancelled.
#[tokio::test]
async fn a_cancelled_apply_stops_before_its_next_move() {
    let (arr, app, ids) = three_films().await;
    let job = applying(&app, &arr, &ids).await;

    let cancelled = app.post(&format!("/api/v1/jobs/{job}/cancel"), serde_json::json!({})).await;

    assert_eq!(cancelled.status, 202, "{}", cancelled.json);
    let task = finished(&app, &job).await;
    assert_eq!(task["status"], "cancelled", "{task}");
    assert_eq!(task["result"]["stopped"], "cancelled");
    assert_eq!(statuses(&app).await, ["applied", "pending", "pending"]);
    assert_eq!(arr.recorded().writes.len(), 1, "a move was sent after the cancel");
}

/// Turning the dry-run on is the switch that overrides everything, an apply
/// already running included.
#[tokio::test]
async fn turning_the_dry_run_on_stops_an_apply_already_running() {
    let (arr, app, ids) = three_films().await;
    let job = applying(&app, &arr, &ids).await;

    app.store_setting("global_dry_run", "true").await;

    let task = finished(&app, &job).await;
    assert_eq!(task["status"], "success", "what moved counts: {task}");
    assert_eq!(task["result"]["stopped"], "dry_run");
    assert_eq!(statuses(&app).await, ["applied", "pending", "pending"]);
}

/// A stop of the server ends the apply before its next move and waits for
/// the move in flight to be recorded before the database closes.
#[tokio::test]
async fn a_stop_of_the_server_waits_for_the_move_in_flight() {
    let (arr, app, ids) = three_films().await;
    let job = applying(&app, &arr, &ids).await;

    let drained = app.state.jobs.drain(Duration::from_secs(5)).await;

    assert!(drained, "the apply outlived the grace");
    assert_eq!(statuses(&app).await, ["applied", "pending", "pending"]);
    let task = finished(&app, &job).await;
    assert_eq!(task["result"]["stopped"], "shutdown", "{task}");
}

/// Only a running apply or revert reads a cancel: a finished task is
/// refused, an unknown one is not found.
#[tokio::test]
async fn only_a_running_move_can_be_cancelled() {
    let (arr, app, ids) = three_films().await;
    let job = applying(&app, &arr, &ids).await;
    finished(&app, &job).await;

    let ended = app.post(&format!("/api/v1/jobs/{job}/cancel"), serde_json::json!({})).await;
    let unknown = app.post("/api/v1/jobs/nothing/cancel", serde_json::json!({})).await;

    assert_eq!(ended.status, 409, "{}", ended.json);
    assert_eq!(unknown.status, 404, "{}", unknown.json);
}
