//! The getting-started guide reads where an installation stands from its data.

use axum::http::StatusCode;
use serde_json::json;

use super::{AN_INSTANCE, TestApp, database_through};
use crate::jobs::JobKind;
use crate::localization::Localizer;
use crate::state::AppState;

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
/// the operator to read the proposals, which a pass nobody watched does not do,
/// and a task someone started is a simulation only when it is one.
#[tokio::test]
async fn only_a_simulation_someone_started_ticks_the_step() {
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
    // Recorded as a task, the pass would still be nobody's.
    let pass = app
        .state
        .jobs
        .start(
            JobKind::Simulate,
            &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
            None,
            crate::jobs::Detail::new("JobDetailSimulating"),
        )
        .await
        .unwrap();
    pass.succeed(crate::jobs::Detail::new("JobDetailSimulated").with("total", 1).with("moves", 1))
        .await;
    crate::services::maintenance::run(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let manual: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE trigger = 'manual' AND status = 'success'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(manual > 0, "no task someone started succeeded, so this proves nothing");

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

/// A test database starts empty, so an upgrade is the one path on which the
/// guide's migration meets an installation already set up.
#[tokio::test]
async fn an_installation_set_up_before_the_guide_starts_with_it_done() {
    let pool = database_through("001_initial_schema").await;
    sqlx::query(AN_INSTANCE).execute(&pool).await.unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let app = TestApp::around(AppState::for_tests_on(pool));
    assert_eq!(app.get("/api/v1/onboarding").await.assert_ok()["state"], "done");
}

/// The scheduler's pass after the next sync supersedes the proposals of a
/// manual run, and the hourly purge deletes them. The run itself happened.
#[tokio::test]
async fn a_manual_simulation_stays_counted_after_the_next_scheduled_pass() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", json!({ "persist": true })).await.assert_ok();
    assert!(done(&steps(&app).await, "simulation"));

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
    crate::services::maintenance::run(
        &app.state,
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await
    .unwrap();
    let manual: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE actor = 'manual'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(manual, 0, "the manual proposals outlived the pass, so this proves nothing");

    assert!(done(&steps(&app).await, "simulation"));
}

/// A library sorted by hand before Routarr: the run proposes nothing, and
/// reading that nothing moves is what the step asks for.
#[tokio::test]
async fn a_library_already_in_place_ticks_the_simulation_step() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    sqlx::query(
        "UPDATE media SET current_root_folder = '/movies/anime',
                          current_path = '/movies/anime/My Neighbor Totoro (1988)'
         WHERE id = 'm-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let run = app.post("/api/v1/simulate", json!({ "persist": true })).await;
    assert_eq!(
        run.assert_ok()["moves_required"],
        0,
        "the item still moves, so this proves nothing"
    );

    assert!(done(&steps(&app).await, "simulation"));
}

/// A source beside the Arr that can answer today: listed, and holding its key.
#[tokio::test]
async fn a_source_listed_with_its_key_ticks_the_metadata_step() {
    let keyless = TestApp::new().await;
    keyless.list_tmdb().await;
    assert!(!done(&steps(&keyless).await, "metadata"), "TMDb without a key answers nothing");

    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    let keyed = TestApp::around(AppState::for_tests().await.with_config(config));
    keyed.list_tmdb().await;

    assert!(done(&steps(&keyed).await, "metadata"));
}

/// A disabled instance routes nothing, whatever it holds.
#[tokio::test]
async fn a_disabled_instance_ticks_neither_of_the_first_two_steps() {
    let app = TestApp::new().await;
    app.seed_library().await;
    mark_synced(&app).await;
    let enabled = steps(&app).await;
    assert!(done(&enabled, "instance") && done(&enabled, "categories"));

    sqlx::query("UPDATE instances SET enabled = 0 WHERE id = 'inst-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let disabled = steps(&app).await;
    assert!(!done(&disabled, "instance"));
    assert!(!done(&disabled, "categories"));

    // A second instance, enabled and synced, with nothing mapped on it.
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
                                webhook_token, last_sync_at)
         VALUES ('inst-2', 'Sonarr', 'sonarr', 'http://sonarr:8989', 'secret', 1, 'tok-2',
                 datetime('now'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let beside = steps(&app).await;
    assert!(done(&beside, "instance"));
    assert!(!done(&beside, "categories"), "only the disabled instance maps a folder");
}

/// The warnings of a `/status` answer: `(message, guide step)`.
async fn warnings(app: &TestApp) -> Vec<(String, Option<String>)> {
    let body = app.get("/api/v1/status").await.assert_ok().clone();
    body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["message"].as_str().unwrap_or_default().to_string(),
                w["guide_step"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// A warning that restates a step names it, so the shell can leave it to the
/// guide while that step is open. Every other warning names none.
#[tokio::test]
async fn a_warning_names_the_step_it_restates() {
    let app = TestApp::new().await;
    app.list_tmdb().await;
    let en = Localizer::new("en");
    let tmdb = crate::services::metadata::info("tmdb").unwrap().display_name;
    let step = |id: &str| Some(id.to_string());

    let fresh = warnings(&app).await;
    for expected in [
        (en.translate("WarnNoEnabledInstance", &[]), step("instance")),
        (en.translate("WarnProviderNeedsKey", &[("provider", tmdb)]), step("metadata")),
        (en.translate("WarnApiUnauthenticated", &[]), None),
    ] {
        assert!(fresh.contains(&expected), "{expected:?} is not in {fresh:?}");
    }

    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.post("/api/v1/categories", json!({ "name": "kids" })).await.assert_ok();
    sqlx::query(
        "INSERT INTO probe_results (subject, reachable, detail, checked_at)
         VALUES ('instance:inst-1', 0, 'error: connection refused', datetime('now'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    // No folder is mapped at all, so every category reaches none.
    let categories: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();

    let set_up = warnings(&app).await;
    for expected in [
        (en.translate("WarnInstanceNoMapping", &[("name", "Fake radarr")]), step("categories")),
        (
            en.translate("WarnUnmappedCategories", &[("count", &categories.to_string())]),
            step("categories"),
        ),
        (
            en.translate(
                "WarnInstanceUnreachable",
                &[("name", "Fake radarr"), ("status", "error: connection refused")],
            ),
            None,
        ),
    ] {
        assert!(set_up.contains(&expected), "{expected:?} is not in {set_up:?}");
    }
}
