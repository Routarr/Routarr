//! Pinned expectations, and the guarantee that they keep protecting something. A suite that
//! only ever goes green proves nothing, so most tests here prove a case fails when it should.

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

/// The button that creates a case from an explanation needs no input beyond a name.
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

/// A rule change that moves a routing nobody was watching is reported.
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
    // Where the item lands now, since the useful question is where it goes instead.
    assert!(result["actual_category"].is_string(), "no actual category reported: {result}");
    assert_ne!(result["actual_category"], "anime");
}

/// Read against the wall clock, an `added_within_days` case would fail alone one morning.
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

/// A sync deletes the rows the Arr stops returning, so a case tied to its `media_id` would
/// vanish with the film it protects.
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

#[tokio::test]
async fn a_case_written_by_the_first_release_still_runs() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    // `Media` and `MediaMetadata` as the first release serialised them.
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

/// An override short-circuits the engine, so folding it into a case would hide a broken rule.
/// Here the exception says what the case expects, so only the rules, gone, can fail it.
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

/// The cases come a page at a time, by name, however long the suite grows.
#[tokio::test]
async fn a_case_can_be_listed_and_deleted() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let id = pin(&app, "Akira stays in anime", "m-1", Some("anime")).await;
    pin(&app, "Totoro stays in anime", "m-1", Some("anime")).await;
    pin(&app, "Ghibli stays in anime", "m-1", Some("anime")).await;

    let first = app.get("/api/v1/rule-tests?per_page=1").await;
    let first = first.assert_ok();
    assert_eq!(first["data"].as_array().map(Vec::len), Some(1), "{first}");
    assert_eq!(first["data"][0]["name"], "Akira stays in anime");
    assert_eq!(first["pagination"]["total"], 3);
    let second = app.get("/api/v1/rule-tests?per_page=1&page=2").await;
    assert_eq!(second.assert_ok()["data"][0]["name"], "Ghibli stays in anime");

    app.delete(&format!("/api/v1/rule-tests/{id}")).await.assert_ok();
    let listed = app.get("/api/v1/rule-tests").await;
    assert_eq!(listed.assert_ok()["pagination"]["total"], 2);
    assert_eq!(app.delete(&format!("/api/v1/rule-tests/{id}")).await.status, 404);
}

/// A case needs a name a list can show, a rule's at most, and an item.
#[tokio::test]
async fn a_case_needs_a_name_and_a_media_item() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let limit = crate::services::rule_engine::MAX_NAME_LENGTH;

    for (name, status) in [("  ".to_string(), 400), ("x".repeat(limit + 1), 400)] {
        let body = serde_json::json!({ "name": name, "media_id": "m-1" });
        assert_eq!(app.post("/api/v1/rule-tests", body).await.status, status, "{name}");
    }
    let body = serde_json::json!({ "name": "x".repeat(limit), "media_id": "m-1" });
    app.post("/api/v1/rule-tests", body).await.assert_ok();
    assert_eq!(
        app.post("/api/v1/rule-tests", serde_json::json!({ "name": "x", "media_id": "nope" }))
            .await
            .status,
        404
    );
}

/// Stored as typed, `"Anime "` would never equal the engine's `anime`, and the case would fail
/// for ever against the decision it pinned.
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

/// Deleted, the category would leave the case unable ever to pass.
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

/// A pin is judged at once, so a failing case shows without anyone pressing Run.
#[tokio::test]
async fn a_failing_case_is_kept_failing_warned_of_and_counted() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let codes = |body: &serde_json::Value| -> Vec<String> {
        body["warnings"].as_array().unwrap().iter().map(|w| w["code"].to_string()).collect()
    };

    let failing = pin(&app, "Totoro in anime", "m-1", Some("anime")).await;

    let listed = app.get("/api/v1/rule-tests").await;
    let case = &listed.assert_ok()["data"][0];
    assert_eq!(case["last_passed"], false);
    assert!(case["last_checked_at"].is_string(), "{case}");
    let status = app.get("/api/v1/status").await;
    assert!(codes(status.assert_ok()).contains(&"\"rule_tests_failing\"".to_string()));
    let metrics = app.text("/api/v1/metrics").await;
    assert!(metrics.contains("routarr_rule_tests 1"), "{metrics}");
    assert!(metrics.contains("routarr_rule_tests_failing 1"), "{metrics}");

    app.delete(&format!("/api/v1/rule-tests/{failing}")).await.assert_ok();
    pin(&app, "Totoro stays", "m-1", None).await;

    let status = app.get("/api/v1/status").await;
    assert!(!codes(status.assert_ok()).contains(&"\"rule_tests_failing\"".to_string()));
    let listed = app.get("/api/v1/rule-tests").await;
    assert_eq!(listed.assert_ok()["data"][0]["last_passed"], true);
}

/// Where the title goes now is the question a failure raises, kept for the next visit.
#[tokio::test]
async fn a_case_keeps_where_its_title_went_on_its_last_run() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Totoro stays in anime", "m-1", Some("anime")).await;
    let listed = async || app.get("/api/v1/rule-tests").await.assert_ok()["data"][0].clone();
    assert_eq!(listed().await["last_actual_category"], "anime");

    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();
    run(&app).await;

    let case = listed().await;
    assert_eq!(case["last_passed"], false);
    assert_eq!(case["last_actual_category"], "standard", "{case}");
}

/// The warning about failing tests leads to them alone.
#[tokio::test]
async fn the_list_narrows_to_the_failing_cases() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Totoro stays in anime", "m-1", None).await;
    pin(&app, "Totoro in standard", "m-1", Some("standard")).await;

    let failing = app.get("/api/v1/rule-tests?failing=true").await;
    let failing = failing.assert_ok();
    assert_eq!(failing["pagination"]["total"], 1, "{failing}");
    assert_eq!(failing["data"][0]["name"], "Totoro in standard");
    let all = app.get("/api/v1/rule-tests?failing=false").await;
    assert_eq!(all.assert_ok()["pagination"]["total"], 2);
}

/// The id opens the title's explanation from its case.
#[tokio::test]
async fn a_case_names_its_title_while_the_library_holds_it() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let id = pin(&app, "Totoro stays", "m-1", None).await;
    assert_eq!(
        app.get("/api/v1/rule-tests").await.assert_ok()["data"][0]["source_media_id"],
        "m-1"
    );
    let one = app.get(&format!("/api/v1/rule-tests/{id}")).await;
    assert_eq!(one.assert_ok()["source_media_id"], "m-1");

    sqlx::query("DELETE FROM media").execute(&app.state.pool).await.unwrap();

    let case = app.get("/api/v1/rule-tests").await.assert_ok()["data"][0].clone();
    assert!(case["source_media_id"].is_null(), "{case}");
}

/// A test that fails after a deliberate rule change is settled by expecting the new routing.
#[tokio::test]
async fn a_case_can_be_told_to_expect_where_its_title_goes_now() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let id = pin(&app, "Totoro in anime", "m-1", Some("anime")).await;
    let path = format!("/api/v1/rule-tests/{id}/expected-category");

    let changed = app.put(&path, serde_json::json!({ "expected_category": "Standard " })).await;

    let changed = changed.assert_ok();
    assert_eq!(changed["expected_category"], "standard");
    assert_eq!(changed["last_passed"], true, "the verdict was not read again: {changed}");
    let status = app.get("/api/v1/status").await;
    let codes: Vec<_> = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].clone())
        .collect();
    assert!(!codes.contains(&"rule_tests_failing".into()), "{codes:?}");
}

/// An expectation no category answers could only fail, and a case that is gone has none.
#[tokio::test]
async fn a_case_expects_only_a_category_that_exists() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let id = pin(&app, "Totoro stays", "m-1", None).await;

    let nowhere = serde_json::json!({ "expected_category": "nowhere" });
    let path = format!("/api/v1/rule-tests/{id}/expected-category");
    app.put(&path, nowhere.clone()).await.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let missing = app.put("/api/v1/rule-tests/nope/expected-category", nowhere).await;
    missing.assert_status(axum::http::StatusCode::NOT_FOUND);
}

/// With every instance kept to proposals, nothing is waiting.
#[tokio::test]
async fn a_failing_case_says_it_holds_auto_apply_only_while_an_instance_is_armed() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    pin(&app, "Totoro in anime", "m-1", Some("anime")).await;
    let message = || async {
        let status = app.get("/api/v1/status").await;
        let body = status.assert_ok().clone();
        let warnings = body["warnings"].as_array().unwrap().clone();
        let failing = warnings.iter().find(|w| w["code"] == "rule_tests_failing").unwrap();
        failing["message"].as_str().unwrap().to_string()
    };

    assert!(message().await.contains("Automatic application waits"));
    app.execute(&["UPDATE instances SET auto_apply = 0"]).await;
    assert!(!message().await.contains("Automatic application waits"));
}
