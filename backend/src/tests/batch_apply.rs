//! Applying a whole simulation in slices.
//!
//! The case: the first reclassification of an existing library, where the batch
//! ceiling turns a single decision into dozens of identical confirmations. Here
//! the ceiling becomes the slice size, and what replaces it as the guardrail is
//! a confirmation that is always required.

use crate::jobs::Attribution;
use crate::services::executor;
use crate::services::routing::{self, SimulationOptions};

use super::TestApp;
use super::fake_arr::FakeArr;

/// A library of `count` films the anime rule wants to move, ready to apply.
async fn library(arr: &FakeArr, count: usize) -> TestApp {
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
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, expires_at)
         VALUES ('tmdb', '8392', 'movie', '[\"Animation\"]', '[]', 'ja', '[]', '2099-01-01')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    for index in 0..count {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
             current_root_folder, monitored, has_files)
             VALUES (?, 'inst-1', ?, 'movie', ?, 8392, ?, '/movies/standard', 1, 1)",
        )
        .bind(format!("m-{index}"))
        .bind(index as i64 + 100)
        .bind(format!("Film {index:03}"))
        .bind(format!("/movies/standard/Film {index:03}"))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    app
}

async fn set_batch_limit(app: &TestApp, limit: usize) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('batch_limit', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(limit.to_string())
    .execute(&app.state.pool)
    .await
    .unwrap();
}

async fn simulate(app: &TestApp) -> String {
    routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap()
    .simulation_id
}

#[tokio::test]
async fn a_library_larger_than_the_batch_limit_is_applied_in_slices() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = simulate(&app).await;

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
    let app = library(&arr, 4).await;
    sqlx::query("UPDATE settings SET value = 'true' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let simulation = simulate(&app).await;

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
    // The Arr rejects every write. The first slice fails; the rest must not be
    // attempted, because an Arr that just refused five moves will refuse five
    // hundred.
    let arr = FakeArr::failing(500).await;
    let app = library(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = simulate(&app).await;

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

    // What was never attempted is still pending, so the next simulation
    // reproposes it and nothing is silently lost.
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending, 7, "the untouched slices stay in the queue");
}

#[tokio::test]
async fn a_simulation_with_nothing_to_move_is_refused_rather_than_reported_empty() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 0).await;
    let simulation = simulate(&app).await;

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

#[tokio::test]
async fn it_only_touches_what_its_own_simulation_proposed() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 4).await;
    set_batch_limit(&app, 50).await;

    let earlier = simulate(&app).await;
    let later = simulate(&app).await;
    assert_ne!(earlier, later);

    // The earlier run's proposals were superseded, so asking for them applies
    // nothing rather than replaying a stale view of the library.
    let refused = executor::apply_simulation_in_batches(
        &app.state,
        &earlier,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await;
    assert!(matches!(refused, Err(crate::error::AppError::BadRequest(_))));

    let report = executor::apply_simulation_in_batches(
        &app.state,
        &later,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    assert_eq!(report.applied, 4);
}

#[tokio::test]
async fn progress_is_recorded_so_the_operations_queue_can_show_it() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 12).await;
    set_batch_limit(&app, 5).await;
    let simulation = simulate(&app).await;

    executor::apply_simulation_in_batches(
        &app.state,
        &simulation,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let (current, total): (i64, i64) = sqlx::query_as(
        "SELECT progress_current, progress_total FROM jobs
          WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(total, 12);
    assert!(current > 0, "progress must be reported as slices complete");
}

#[tokio::test]
async fn the_endpoint_refuses_without_a_confirmation_however_small_the_library() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 3).await;
    set_batch_limit(&app, 50).await;
    let simulation = simulate(&app).await;

    let response = app
        .post("/api/v1/decisions/apply-all", serde_json::json!({ "simulation_id": simulation }))
        .await;

    // A stable code, never the message text: the client must recognise this
    // without parsing prose that changes with the language.
    assert_eq!(response.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(response.json["error"], "confirmation_required");
    assert!(arr.recorded().writes.is_empty(), "nothing may be written before confirming");
}

#[tokio::test]
async fn the_endpoint_applies_everything_once_confirmed() {
    let arr = FakeArr::start().await;
    let app = library(&arr, 7).await;
    set_batch_limit(&app, 3).await;
    let simulation = simulate(&app).await;

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
