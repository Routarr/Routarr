//! Pinned expectations, and the guarantee that they keep protecting something.
//!
//! The point of the feature is that editing one rule cannot silently change
//! what another rule routes. So the tests here are mostly about a case
//! *failing* when it should. A suite that only ever goes green proves nothing.

use super::TestApp;

async fn pin(app: &TestApp, name: &str, media_id: &str, expected: Option<&str>) -> String {
    let mut body = serde_json::json!({ "name": name, "media_id": media_id });
    if let Some(category) = expected {
        body["expected_category"] = serde_json::json!(category);
    }
    let response = app.post("/api/v1/rule-tests", body).await;
    response.assert_ok()["id"].as_str().expect("id").to_string()
}

async fn run(app: &TestApp) -> serde_json::Value {
    app.post("/api/v1/rule-tests/run", serde_json::json!({})).await.assert_ok().clone()
}

/// Pinning takes the engine's current answer, so the button that creates a case
/// from an explanation needs no input beyond a name.
#[tokio::test]
async fn a_pinned_decision_defaults_to_what_the_engine_says_today() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    pin(&app, "Akira stays in anime", "m-1", None).await;

    let outcome = run(&app).await;
    assert_eq!(outcome["total"], 1);
    assert_eq!(outcome["passed"], 1, "a case pinned from today must pass today: {outcome}");
    assert_eq!(outcome["results"][0]["expected_category"], "anime");
}

/// The whole reason the feature exists: a rule change that moves a routing
/// nobody was watching has to be reported.
#[tokio::test]
async fn deleting_the_rule_a_case_depends_on_fails_that_case() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Akira stays in anime", "m-1", None).await;
    assert_eq!(run(&app).await["passed"], 1);

    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();

    let outcome = run(&app).await;
    assert_eq!(outcome["failed"], 1, "the case went green with no rules left: {outcome}");
    let result = &outcome["results"][0];
    assert_eq!(result["expected_category"], "anime");
    // It reports where the item lands now rather than a bare failure, because
    // the useful question is "so where does it go instead".
    assert!(result["actual_category"].is_string(), "no actual category reported: {result}");
    assert_ne!(result["actual_category"], "anime");
}

/// A case is judged at the instant it was pinned. `added_within_days` answers
/// differently from one day to the next, and a case read against the wall
/// clock would fail alone one morning, with nothing changed. Here the case is
/// moved back to the day after the film arrived, when the rule held.
#[tokio::test]
async fn a_case_is_judged_at_the_instant_it_was_pinned() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET added_at = '2020-01-01 10:00:00' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('rule-new', 'New arrivals', 10, 1, 'both',
                 '[{\"type\":\"added_within_days\",\"value\":7}]', 'anime', 'all')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let id = pin(&app, "A new arrival goes to anime", "m-1", Some("anime")).await;
    assert_eq!(run(&app).await["failed"], 1, "judged today, the film is years old");

    sqlx::query("UPDATE rule_tests SET evaluated_at = '2020-01-02 10:00:00' WHERE id = ?")
        .bind(&id)
        .execute(&app.state.pool)
        .await
        .unwrap();

    let outcome = run(&app).await;
    assert_eq!(outcome["passed"], 1, "the case was judged at another instant: {outcome}");
}

/// A case survives the library. `sync` deletes rows the Arr stops returning and
/// that cascades to overrides, so a case referencing `media_id` would vanish
/// with the film it was written to protect.
#[tokio::test]
async fn a_case_still_runs_after_its_media_row_is_gone() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Akira stays in anime", "m-1", None).await;

    sqlx::query("DELETE FROM media").execute(&app.state.pool).await.unwrap();

    let outcome = run(&app).await;
    assert_eq!(outcome["total"], 1, "the case disappeared with the media row");
    assert_eq!(outcome["passed"], 1, "the snapshot stopped being enough: {outcome}");
}

/// A case is kept across upgrades, so a snapshot written by the first release,
/// in the shape its media and metadata had then, still reads and still runs.
#[tokio::test]
async fn a_case_written_by_the_first_release_still_runs() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    // `Media` and `MediaMetadata` as v0.1.0 serialised them.
    let media = r#"{"id":"m-1","instance_id":"inst-1","arr_id":10,"media_type":"movie",
        "title":"My Neighbor Totoro","sort_title":null,"year":1988,"tmdb_id":8392,
        "tvdb_id":null,"imdb_id":null,"current_path":"/movies/standard/Totoro",
        "current_root_folder":"/movies/standard","monitored":true,"has_files":true,
        "status":"released","added_at":null,"series_type":null,"size_on_disk":null,
        "season_count":null,"tags":null,"genres":null,"original_language":null,
        "certification":null,"last_synced_at":"2026-09-21 10:00:00"}"#;
    let metadata = r#"{"genres":["Animation"],"keywords":[],"original_language":"ja",
        "origin_countries":["JP"],"certification":"G","status":"Released","overview":null,
        "poster_path":null,"field_sources":{"genres":"tmdb","original_language":"tmdb"},
        "sources":["tmdb"]}"#;
    sqlx::query(
        "INSERT INTO rule_tests (id, name, media_type, media_json, metadata_json, evaluated_at,
                                 expected_category, source_media_title)
         VALUES ('t-1', 'Totoro stays in anime', 'movie', ?, ?, '2026-09-21 10:00:00', 'anime',
                 'My Neighbor Totoro')",
    )
    .bind(media)
    .bind(metadata)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let outcome = run(&app).await;

    assert_eq!(outcome["passed"], 1, "{outcome}");
}

/// An override is a human decision about one item and short-circuits the
/// engine. Folding it into a case would let a pinned exception hide a rule that
/// had stopped working: here the exception says what the case expects, so
/// only the rules, gone, can fail it.
#[tokio::test]
async fn an_override_does_not_mask_what_the_rules_decide() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Akira stays in anime", "m-1", None).await;

    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o1', 'm-1', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();

    let outcome = run(&app).await;
    assert_eq!(
        outcome["failed"], 1,
        "the override answered for the rules and hid their absence: {outcome}"
    );
}

#[tokio::test]
async fn a_case_can_be_listed_and_deleted() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let id = pin(&app, "Akira stays in anime", "m-1", Some("anime")).await;

    let listed = app.get("/api/v1/rule-tests").await;
    assert_eq!(listed.assert_ok().as_array().expect("array").len(), 1);

    app.delete(&format!("/api/v1/rule-tests/{id}")).await.assert_ok();
    assert!(app.get("/api/v1/rule-tests").await.assert_ok().as_array().unwrap().is_empty());
    assert_eq!(app.delete(&format!("/api/v1/rule-tests/{id}")).await.status, 404);
}

#[tokio::test]
async fn a_case_needs_a_name_and_a_media_item() {
    let app = TestApp::new().await;
    app.seed_library().await;

    assert_eq!(
        app.post("/api/v1/rule-tests", serde_json::json!({ "name": "  ", "media_id": "m-1" }))
            .await
            .status,
        400
    );
    assert_eq!(
        app.post("/api/v1/rule-tests", serde_json::json!({ "name": "x", "media_id": "nope" }))
            .await
            .status,
        404
    );
}

/// A pinned category is a category name, stored the way every writer of one
/// stores it: `"Anime "` would never equal the `anime` the engine answers, and
/// the case would fail for ever against the very decision it pinned.
#[tokio::test]
async fn a_pinned_category_is_stored_as_a_category_name() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    pin(&app, "Akira stays in anime", "m-1", Some("Anime ")).await;

    let outcome = run(&app).await;
    assert_eq!(outcome["results"][0]["expected_category"], "anime");
    assert_eq!(outcome["passed"], 1, "{outcome}");
}

/// A case expecting a category that does not exist can only ever fail.
#[tokio::test]
async fn a_case_cannot_expect_a_category_that_does_not_exist() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app
        .post(
            "/api/v1/rule-tests",
            serde_json::json!({ "name": "Lost", "media_id": "m-1", "expected_category": "nowhere" }),
        )
        .await;

    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

/// A category a case expects is in use: deleted, it leaves the case unable to
/// pass, as a deleted category leaves a rule unable to route.
#[tokio::test]
async fn a_category_a_case_expects_is_not_deleted() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-kids', 'kids')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    pin(&app, "Totoro is for kids", "m-1", Some("kids")).await;

    let refused = app.delete("/api/v1/categories/cat-kids").await;

    refused.assert_status(axum::http::StatusCode::CONFLICT);
    assert!(refused.message().contains("rule tests: 1"), "{}", refused.message());
}
