//! Pinned expectations, and the guarantee that they keep protecting something.
//!
//! The point of the feature is that editing one rule cannot silently change
//! what another rule routes. So the tests here are mostly about a case
//! *failing* when it should — a suite that only ever goes green is a suite that
//! proves nothing.

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

/// A case survives the library. `sync` deletes rows the Arr stops returning and
/// that cascades to overrides — a case referencing `media_id` would vanish with
/// the film it was written to protect.
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

/// An override is a human decision about one item and short-circuits the
/// engine. Folding it into a case would let a pinned exception hide a rule that
/// had stopped working.
#[tokio::test]
async fn an_override_does_not_mask_what_the_rules_decide() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    pin(&app, "Akira stays in anime", "m-1", None).await;

    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o1', 'm-1', 'kids')",
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
