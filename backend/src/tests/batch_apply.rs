//! Applying a whole simulation in slices.
//!
//! The case: the first reclassification of an existing library, where the batch
//! ceiling turns a single decision into dozens of identical confirmations. Here
//! the ceiling becomes the slice size, and what replaces it as the guardrail is
//! a confirmation that is always required.

use crate::jobs::Attribution;
use crate::services::executor;

use super::TestApp;
use super::fake_arr::FakeArr;

async fn set_batch_limit(app: &TestApp, limit: usize) {
    app.store_setting("batch_limit", &limit.to_string()).await;
}

#[tokio::test]
async fn a_library_larger_than_the_batch_limit_is_applied_in_slices() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let report = executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.candidates, 12);
    assert_eq!(report.applied, 12);
    assert_eq!(report.failed, 0);
    // 12 items at 5 per slice: three slices, the last one short.
    assert_eq!(report.batches_planned, 3);
    assert_eq!(report.batches_run, 3);
    assert!(!report.stopped_early);

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending, 0, "nothing may be left behind");
}

#[tokio::test]
async fn global_dry_run_still_outranks_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 4).await;
    sqlx::query("UPDATE settings SET value = 'true' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let simulation = app.simulate().await;

    let refused = executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await;

    assert!(matches!(refused, Err(crate::error::AppError::BadRequest(_))));
    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn a_failing_slice_ends_the_run_instead_of_hammering_the_arr() {
    // The Arr rejects every write. The first slice fails, and the rest must not
    // be attempted, because an Arr that just refused five moves will refuse five
    // hundred.
    let arr = FakeArr::failing(500).await;
    let app = TestApp::films_to_move(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let report = executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert!(report.stopped_early, "the run must stop at the first failing slice");
    assert_eq!(report.batches_run, 1, "of {} planned", report.batches_planned);
    assert_eq!(report.applied, 0);
    assert!(report.failed > 0);
    assert!(!report.errors.is_empty(), "the failure must be reported, not just counted");
    assert_eq!(app.last_job_status("apply").await, "failed", "nothing done is a failed job");

    // What was never attempted is still pending, so the next simulation
    // reproposes it and nothing is silently lost.
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending, 7, "the untouched slices stay in the queue");
}

/// A later slice that cannot be read for revalidation ends the run as a
/// refused slice does: the titles earlier slices moved are reported, the job
/// closes with its report, and the rest stays pending. The count of routing
/// changes, the first thing a slice reads, leaves while the first slice's edit
/// is held, so the second slice cannot load.
#[tokio::test]
async fn a_slice_that_cannot_be_loaded_after_one_that_moved_ends_the_run_with_its_report() {
    use std::time::Duration;

    let arr = FakeArr::holding_edits(Duration::from_millis(300)).await;
    let app = TestApp::films_to_move(&arr, 10).await.with_http_budget(Duration::from_secs(5));
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let (confirmed, by) = (executor::Confirmed::all(), Attribution::manual(None));
    let run =
        executor::apply_simulation_in_batches(&app.state, &simulation, false, &confirmed, &by);
    let unreadable = async {
        while arr.recorded().writes.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        app.execute(&["ALTER TABLE overrides RENAME TO overrides_gone"]).await;
    };
    let (report, ()) = tokio::join!(run, unreadable);

    let report = report.expect("the moves already made were not reported");
    assert!(report.stopped_early, "{report:?}");
    assert_eq!((report.batches_run, report.applied), (1, 5), "{report:?}");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE status = 'pending'").await, 5);
    assert_eq!(app.last_job_status("apply").await, "success");
    let stored = app.count("SELECT COUNT(*) FROM jobs WHERE kind = 'apply' AND result IS NOT NULL");
    assert_eq!(stored.await, 1, "the job carries no report");
}

#[tokio::test]
async fn a_simulation_with_nothing_to_move_is_refused_rather_than_reported_empty() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 0).await;
    let simulation = app.simulate().await;

    let refused = executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await;

    assert!(matches!(refused, Err(crate::error::AppError::BadRequest(_))));
}

/// A scheduled pass after a sync writes every proposal again under a new
/// simulation. Applying the earlier one applies the identical proposals that
/// replaced its own, leaves the one now going elsewhere, and touches no title
/// it did not propose.
#[tokio::test]
async fn an_earlier_simulation_is_applied_through_the_identical_proposals_since() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 4).await;
    set_batch_limit(&app, 50).await;
    let earlier = app.simulate().await;
    arr.hold_film(104);
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('m-4', 'inst-1', 104, 'movie', 'Film 004', 8392, '/movies/standard/Film 104',
                 '/movies/standard', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-3', 'm-3', 'standard')",
    ])
    .await;
    app.simulate().await;

    let report = apply(&app, &earlier).await;

    assert_eq!((report.applied, report.superseded), (3, 1), "{report:?}");
    let pending: Vec<String> = sqlx::query_scalar(
        "SELECT media_id FROM decisions
          WHERE status = 'pending' AND superseded = 0 AND action = 'move'",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, ["m-4"], "a title the earlier simulation never proposed was moved");
}

/// Every proposal of a simulation replaced by another move: refused with the
/// reason, not "no decision selected".
#[tokio::test]
async fn a_simulation_whose_proposals_all_go_elsewhere_now_is_refused() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let earlier = app.simulate().await;
    app.execute(&[
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-0', 'm-0', 'standard')",
    ])
    .await;
    app.simulate().await;

    let refused = executor::apply_simulation_in_batches(
        &app.state,
        &earlier,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await;

    assert!(matches!(refused, Err(crate::error::AppError::Conflict(_))), "{refused:?}");
    assert!(arr.recorded().writes.is_empty());
}

/// A film Radarr no longer holds fails alone, and the others still move.
#[tokio::test]
async fn a_film_radarr_no_longer_holds_fails_alone() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 3).await;
    arr.forget_movie(101);
    let simulation = app.simulate().await;

    let report = apply(&app, &simulation).await;

    assert_eq!((report.applied, report.failed), (2, 1), "{report:?}");
    let failed: Vec<String> =
        sqlx::query_scalar("SELECT media_id FROM decisions WHERE status = 'failed'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(failed, ["m-1"]);
}

/// Apply a whole simulation, every guardrail confirmed.
async fn apply(app: &TestApp, simulation: &str) -> crate::services::executor::BatchApplyReport {
    executor::apply_simulation_in_batches(
        &app.state,
        simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap()
}

/// Progress is written as each slice ends, so the operations queue shows a
/// run moving rather than one that jumps to its end. Each edit is held, so the
/// count between two slices stands long enough to be read.
#[tokio::test]
async fn progress_is_recorded_as_each_slice_ends() {
    use std::time::Duration;

    let arr = FakeArr::holding_edits(Duration::from_millis(200)).await;
    let app = TestApp::films_to_move(&arr, 12).await.with_http_budget(Duration::from_secs(5));
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let (confirmed, by) = (executor::Confirmed::all(), Attribution::manual(None));
    let run =
        executor::apply_simulation_in_batches(&app.state, &simulation, false, &confirmed, &by);
    let midway = async {
        loop {
            let job: Option<(i64, i64, String)> = sqlx::query_as(
                "SELECT progress_current, progress_total, status FROM jobs WHERE kind = 'apply'",
            )
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
            match job {
                Some((current, total, status)) if status == "running" && current > 0 => {
                    return Some((current, total));
                }
                Some((_, _, status)) if status != "running" => return None,
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    };
    let (report, midway) = tokio::join!(run, midway);

    report.unwrap();
    let (current, total) = midway.expect("no slice was counted before the run ended");
    assert_eq!(total, 12);
    assert!(current < total, "{current} of {total} while running");
}

/// A run stops at the first slice the Arr refuses whole however much moved
/// before it, and what moved counts: the job succeeded, and its counts say
/// what failed.
#[tokio::test]
async fn a_slice_refused_after_one_that_moved_ends_the_run() {
    let arr = FakeArr::start().await;
    // Films 005 to 009, the second slice of five.
    for id in 105..110 {
        arr.refuse_movie(id);
    }
    let app = TestApp::films_to_move(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let report = executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert!(report.stopped_early, "{report:?}");
    assert_eq!((report.batches_run, report.applied, report.failed), (2, 5, 5), "{report:?}");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE status = 'pending'").await, 2);
    assert_eq!(app.last_job_status("apply").await, "success");
}

/// The dry-run turned on while a whole simulation is applied stops the
/// slice running before its next move, and the slices after it never run.
#[tokio::test]
async fn turning_the_dry_run_on_stops_the_slices_still_to_run() {
    use std::time::Duration;

    let arr = FakeArr::holding_edits(Duration::from_millis(100)).await;
    let app = TestApp::films_to_move(&arr, 10).await;
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;
    let (confirmed, by) = (executor::Confirmed::all(), Attribution::manual(None));

    let run =
        executor::apply_simulation_in_batches(&app.state, &simulation, false, &confirmed, &by);
    let switch = async {
        while arr.recorded().writes.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        app.store_setting("global_dry_run", "true").await;
    };
    let (report, ()) = tokio::join!(run, switch);

    let report = report.unwrap();
    assert_eq!(report.stopped, Some(executor::StopReason::DryRun), "{report:?}");
    assert_eq!((report.batches_run, report.applied), (1, 1), "{report:?}");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE status = 'pending'").await, 9);
}

/// One film the Arr refuses, deleted there since the last sync say, fails
/// alone, and the slices after it still run.
#[tokio::test]
async fn one_refused_film_does_not_end_the_run() {
    let arr = FakeArr::start().await;
    arr.refuse_movie(105);
    let app = TestApp::films_to_move(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = app.simulate().await;

    let report = apply(&app, &simulation).await;

    assert!(!report.stopped_early, "{report:?}");
    assert_eq!((report.batches_run, report.applied, report.failed), (3, 11, 1), "{report:?}");
}

#[tokio::test]
async fn the_endpoint_refuses_without_a_confirmation_however_small_the_library() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 3).await;
    set_batch_limit(&app, 50).await;
    let simulation = app.simulate().await;

    let response = app
        .post("/api/v1/decisions/apply-all", serde_json::json!({ "simulation_id": simulation }))
        .await;

    // A stable code, never the message text: the client must recognise this
    // without parsing prose that changes with the language.
    assert_eq!(response.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(response.json["error"], "confirmation_required");
    assert!(arr.recorded().writes.is_empty(), "nothing may be written before confirming");
}

/// The question is asked whole: how many move, and whether their files move
/// with them. The interface shows it as it comes, so a clause missing here is
/// missing on screen, and a threshold of zero is no threshold to name.
#[tokio::test]
async fn the_batch_question_says_how_many_move_and_whether_their_files_do() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 3).await;
    let simulation = app.simulate().await;
    let localizer = app.state.localizer().await;
    let ask = |key: &str| localizer.translate(key, &[("count", "3")]);

    for (move_files, expected) in
        [(false, ask("ConfirmApplyAllItems")), (true, ask("ConfirmApplyAllItemsWithFiles"))]
    {
        let response = app
            .post(
                "/api/v1/decisions/apply-all",
                serde_json::json!({ "simulation_id": simulation, "move_files": move_files }),
            )
            .await;
        let message = response.assert_status(axum::http::StatusCode::CONFLICT)["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(message.starts_with(&expected), "move_files {move_files}: {message}");
    }
}

#[tokio::test]
async fn the_endpoint_applies_everything_once_confirmed() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 7).await;
    set_batch_limit(&app, 3).await;
    let simulation = app.simulate().await;

    let response = app
        .post(
            "/api/v1/decisions/apply-all",
            serde_json::json!({ "simulation_id": simulation, "confirm": ["batch"] }),
        )
        .await;

    let body = response.assert_ok();
    assert_eq!(body["applied"], 7);
    assert_eq!(body["batches_planned"], 3);
    assert_eq!(body["stopped_early"], false);
}
