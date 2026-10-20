//! Moves that carry files, sent to each Arr one at a time: an Arr runs one disk command at once,
//! and a queue full of moves holds back the import of every finished download.

use std::time::Duration;

use super::TestApp;
use super::fake_arr::FakeArr;
use crate::jobs::Attribution;
use crate::services::executor::{self, BatchApplyReport, StopReason};

async fn apply_with_files(app: &TestApp) -> BatchApplyReport {
    let simulation = app.simulate().await;
    let (confirmed, by) = (executor::Confirmed::all(), Attribution::manual(None));
    let every = crate::models::DecisionFilter::default();
    executor::apply_simulation_in_batches(&app.state, &simulation, &every, true, &confirmed, &by)
        .await
        .expect("the apply ran")
}

async fn status_of(app: &TestApp, media_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM decisions WHERE media_id = ? AND superseded = 0")
        .bind(media_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

/// Each move with files is followed to its end before the next is sent, so the Arr never holds
/// two, and each title ends applied with the task counting it. Sent straight through, two moves
/// sit in one Arr at once, which the probe sees.
#[tokio::test]
async fn moves_with_files_reach_the_arr_one_at_a_time_and_end_applied() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(100));
    let app = TestApp::films_to_move(&arr, 4).await;

    let report = apply_with_files(&app).await;

    assert_eq!((report.applied, report.moving, report.stopped), (4, 0, None), "{report:?}");
    assert_eq!(arr.most_moves_held(), 1);
    let (done, total): (i64, i64) =
        sqlx::query_as("SELECT progress_current, progress_total FROM jobs WHERE kind = 'apply'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!((done, total), (4, 4));

    let straight = FakeArr::start().await;
    straight.moving_for(Duration::from_millis(100));
    let adapter = crate::integrations::adapter::ArrAdapter::new(
        app.state.http.clone(),
        "radarr",
        &straight.base_url,
        "key",
    )
    .unwrap();
    for _ in 0..2 {
        adapter.move_item(10, "/movies/anime", true).await.unwrap();
    }
    assert_eq!(straight.most_moves_held(), 2, "the probe cannot see a queue");
}

/// A stop while a move runs ends the run at once: the move goes on in the Arr and is left
/// `requested` for a sync to settle, and the titles not sent stay pending.
#[tokio::test]
async fn a_run_stopped_while_a_move_runs_leaves_it_requested_and_the_rest_pending() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(400));
    let app = TestApp::films_to_move(&arr, 3).await;

    let run = apply_with_files(&app);
    let stop = async {
        arr.written(1).await;
        app.store_setting("global_dry_run", "true").await;
    };
    let (report, ()) = tokio::join!(run, stop);

    assert_eq!(report.stopped, Some(StopReason::DryRun), "{report:?}");
    assert_eq!((report.applied, report.moving), (0, 1), "{report:?}");
    assert_eq!(arr.recorded().writes.len(), 1, "a title was sent after the stop");
    assert_eq!(status_of(&app, "m-0").await, "requested");
    assert_eq!(status_of(&app, "m-1").await, "pending");
    assert_eq!(status_of(&app, "m-2").await, "pending");
}

/// An Arr whose commands cannot be read for five times the wait stops the run, which reports
/// why: the move sent stays `requested`, the rest pending.
#[tokio::test]
async fn an_arr_whose_commands_cannot_be_read_stops_the_run_as_unreachable() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_secs(5));
    arr.break_command_listing();
    let app = TestApp::films_to_move(&arr, 2).await.with_move_wait(Duration::from_millis(10));

    let report = apply_with_files(&app).await;

    assert_eq!(report.stopped, Some(StopReason::Unreachable), "{report:?}");
    assert_eq!(report.moving, 1, "{report:?}");
    assert_eq!(status_of(&app, "m-0").await, "requested");
    assert_eq!(status_of(&app, "m-1").await, "pending");
    assert_eq!(app.last_job_status("apply").await, "success");
}

/// A title whose move is no longer one to make by the time its turn comes is skipped, not sent:
/// a run moving files can last hours.
#[tokio::test]
async fn a_title_no_longer_to_move_when_its_turn_comes_is_skipped() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(200));
    let app = TestApp::films_to_move(&arr, 2).await;

    let run = apply_with_files(&app);
    let withdrawn = async {
        arr.written(1).await;
        app.execute(&["UPDATE decisions SET status = 'skipped' WHERE media_id = 'm-1'"]).await;
    };
    let (report, ()) = tokio::join!(run, withdrawn);

    assert_eq!((report.applied, report.skipped), (1, 1), "{report:?}");
    assert_eq!(arr.recorded().writes.len(), 1, "the withdrawn title was sent");
}

/// While the second title's files move, the task counts the first as done and names the one
/// moving: a run of hours is followed title by title.
#[tokio::test]
async fn the_task_counts_each_title_and_names_the_one_moving() {
    let arr = FakeArr::start().await;
    arr.moving_for(Duration::from_millis(300));
    let app = TestApp::films_to_move(&arr, 3).await;

    let run = apply_with_files(&app);
    let watched = async {
        arr.written(2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let seen: (i64, i64, String) = sqlx::query_as(
            "SELECT progress_current, progress_total, detail FROM jobs WHERE kind = 'apply'",
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        seen
    };
    let (_, (done, total, detail)) = tokio::join!(run, watched);

    assert_eq!((done, total), (1, 3));
    assert!(detail.contains("Film 001"), "{detail}");
}
