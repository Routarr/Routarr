//! The getting-started guide reads where an installation stands from its data.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;

/// The steps as `GET /onboarding` returns them: `(id, done, optional)`.
async fn steps(app: &TestApp) -> Vec<(String, bool, bool)> {
    let body = app.get("/api/v1/onboarding").await.assert_ok().clone();
    body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_string(),
                s["done"].as_bool().unwrap(),
                s["optional"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn done(steps: &[(String, bool, bool)], id: &str) -> bool {
    steps.iter().find(|(step, ..)| step == id).map(|(_, done, _)| *done).unwrap()
}

async fn mark_synced(app: &TestApp) {
    sqlx::query("UPDATE instances SET last_sync_at = datetime('now') WHERE id = 'inst-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_fresh_installation_is_walked_through_the_guide() {
    let app = TestApp::new().await;

    let body = app.get("/api/v1/onboarding").await.assert_ok().clone();

    assert_eq!(body["state"], "pending");
    assert_eq!(body["complete"], false);
    let steps = steps(&app).await;
    let ids: Vec<&str> = steps.iter().map(|(id, ..)| id.as_str()).collect();
    assert_eq!(ids, ["instance", "categories", "metadata", "rule", "simulation", "live"]);
    assert!(steps.iter().all(|(_, done, _)| !done), "nothing is set up yet: {steps:?}");
    let optional: Vec<&str> =
        steps.iter().filter(|(.., optional)| *optional).map(|(id, ..)| id.as_str()).collect();
    assert_eq!(optional, ["metadata", "live"]);
}

#[tokio::test]
async fn each_step_ticks_itself_from_the_data() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // A saved instance that never synced has no root folders to map yet.
    assert!(!done(&steps(&app).await, "instance"));
    mark_synced(&app).await;
    assert!(done(&steps(&app).await, "instance"));
    assert!(done(&steps(&app).await, "categories"), "both seeded categories are mapped");

    assert!(!done(&steps(&app).await, "rule"));
    app.seed_anime_rule().await;
    assert!(done(&steps(&app).await, "rule"));

    assert!(!done(&steps(&app).await, "simulation"));
    app.post("/api/v1/simulate", json!({ "persist": true })).await.assert_ok();
    assert!(done(&steps(&app).await, "simulation"));

    // The two optional steps are still open, and they do not hold it back.
    let body = app.get("/api/v1/onboarding").await.assert_ok().clone();
    assert_eq!(body["complete"], true);
    assert!(!done(&steps(&app).await, "metadata"));
    assert!(!done(&steps(&app).await, "live"));
    // Finishing is the operator's click, never inferred.
    assert_eq!(body["state"], "pending");
}

/// The scheduler runs a simulation of its own after every sync. The step asks
/// the operator to read the proposals, which a pass nobody watched does not do.
#[tokio::test]
async fn a_simulation_the_scheduler_ran_leaves_the_step_open() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    crate::services::routing::run_simulation(
        &app.state.pool,
        crate::services::routing::SimulationOptions {
            persist: true,
            trigger: crate::jobs::TRIGGER_SCHEDULE.to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let proposals: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(proposals > 0, "the scheduled pass left nothing, so this proves nothing");

    assert!(!done(&steps(&app).await, "simulation"));
}

#[tokio::test]
async fn a_category_that_reaches_no_folder_holds_the_categories_step() {
    let app = TestApp::new().await;
    app.seed_library().await;
    mark_synced(&app).await;
    assert!(done(&steps(&app).await, "categories"));

    app.post("/api/v1/categories", json!({ "name": "kids" })).await.assert_ok();

    assert!(!done(&steps(&app).await, "categories"));
}

#[tokio::test]
async fn deleting_the_only_instance_puts_the_first_step_back() {
    let app = TestApp::new().await;
    app.seed_library().await;
    mark_synced(&app).await;
    assert!(done(&steps(&app).await, "instance"));

    app.delete("/api/v1/instances/inst-1").await;

    assert!(!done(&steps(&app).await, "instance"));
}

#[tokio::test]
async fn leaving_test_mode_ticks_the_last_step() {
    let app = TestApp::new().await;
    assert!(!done(&steps(&app).await, "live"));

    app.put("/api/v1/settings", json!({ "settings": { "global_dry_run": "false" } }))
        .await
        .assert_ok();

    assert!(done(&steps(&app).await, "live"));
}

#[tokio::test]
async fn a_skipped_guide_can_be_resumed_and_finished() {
    let app = TestApp::new().await;

    let body = app.put("/api/v1/onboarding", json!({ "state": "dismissed" })).await;
    assert_eq!(body.assert_ok()["state"], "dismissed");
    let body = app.put("/api/v1/onboarding", json!({ "state": "pending" })).await;
    assert_eq!(body.assert_ok()["state"], "pending");
    let body = app.put("/api/v1/onboarding", json!({ "state": "done" })).await;
    assert_eq!(body.assert_ok()["state"], "done");
    assert_eq!(app.get("/api/v1/onboarding").await.assert_ok()["state"], "done");
}

#[tokio::test]
async fn an_unknown_guide_state_is_refused_on_both_routes() {
    let app = TestApp::new().await;

    app.put("/api/v1/onboarding", json!({ "state": "later" }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    app.put("/api/v1/settings", json!({ "settings": { "onboarding": "later" } }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    assert_eq!(app.get("/api/v1/onboarding").await.assert_ok()["state"], "pending");
}

/// The migration an existing database runs when it upgrades, replayed here on
/// purpose: every test database starts empty, so the file ran before any
/// instance existed.
#[tokio::test]
async fn an_installation_set_up_already_starts_with_the_guide_done() {
    const MIGRATION: &str = include_str!("../../migrations/002_onboarding.sql");

    let fresh = TestApp::new().await;
    sqlx::raw_sql(MIGRATION).execute(&fresh.state.pool).await.unwrap();
    assert_eq!(fresh.get("/api/v1/onboarding").await.assert_ok()["state"], "pending");

    let set_up = TestApp::new().await;
    set_up.seed_library().await;
    sqlx::raw_sql(MIGRATION).execute(&set_up.state.pool).await.unwrap();
    assert_eq!(set_up.get("/api/v1/onboarding").await.assert_ok()["state"], "done");

    // A choice already made is kept.
    let skipped = TestApp::new().await;
    skipped.put("/api/v1/onboarding", json!({ "state": "dismissed" })).await.assert_ok();
    skipped.seed_library().await;
    sqlx::raw_sql(MIGRATION).execute(&skipped.state.pool).await.unwrap();
    assert_eq!(skipped.get("/api/v1/onboarding").await.assert_ok()["state"], "dismissed");
}
