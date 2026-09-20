//! End-to-end behaviour of the translation layer.
//!
//! The interesting property is not that the interface shows French words — it is that
//! everything Routarr *produces* (justifications, diagnostics, guardrail
//! refusals) follows the same setting.

use axum::http::StatusCode;

use super::TestApp;

async fn speak_french(app: &TestApp) {
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "ui_language": "fr" } }))
        .await
        .assert_ok();
}

#[tokio::test]
async fn the_dictionary_is_served_without_authentication() {
    // The shell needs its strings before it can even render an auth error.
    let app = TestApp::with_api_key("s3cret").await;

    let response = app.get("/api/v1/localization").await;
    let body = response.assert_ok();

    assert_eq!(body["language"], "en");
    assert!(body["strings"]["Dashboard"].is_string());
}

#[tokio::test]
async fn the_dictionary_follows_the_configured_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response = app.get("/api/v1/localization").await;
    let body = response.assert_ok();

    assert_eq!(body["language"], "fr");

    // Asserted as a proportion rather than by pinning one key to one sentence:
    // a partial translation is allowed, so any individual key may legitimately
    // still be English, and a test that pins one would forbid what the policy
    // permits.
    let english = TestApp::new().await.get("/api/v1/localization").await;
    let french_strings = body["strings"].as_object().unwrap();
    let english_strings = english.json["strings"].as_object().unwrap();

    let translated = french_strings
        .iter()
        .filter(|(key, value)| english_strings.get(*key) != Some(*value))
        .count();
    assert!(
        translated * 2 > english_strings.len(),
        "most of the dictionary should be French, got {translated} of {}",
        english_strings.len()
    );
}

#[tokio::test]
async fn shipped_languages_are_advertised() {
    let app = TestApp::new().await;
    let response = app.get("/api/v1/localization/languages").await;
    let body = response.assert_ok();

    let codes: Vec<&str> =
        body["languages"].as_array().unwrap().iter().map(|l| l["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"en") && codes.contains(&"fr"));
    assert_eq!(body["default"], "en");
}

#[tokio::test]
async fn an_unsupported_language_is_refused() {
    let app = TestApp::new().await;
    let response = app
        .put("/api/v1/settings", serde_json::json!({ "settings": { "ui_language": "kl" } }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn decision_justifications_are_written_in_the_configured_language() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    speak_french(&app).await;

    let response = app.post("/api/v1/simulate", serde_json::json!({})).await;
    let reasons = response.assert_ok()["decisions"][0]["reasons"].as_array().unwrap().clone();

    assert!(
        reasons.iter().any(|r| r.as_str().unwrap().contains("La langue d'origine")),
        "explanations must be translated too: {reasons:?}"
    );
    assert!(reasons.iter().all(|r| !r.as_str().unwrap().contains("found [")));
}

#[tokio::test]
async fn the_fallback_explanation_is_translated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    speak_french(&app).await;

    let response =
        app.post("/api/v1/simulate", serde_json::json!({ "persist_unchanged": true })).await;
    let reasons = response.assert_ok()["decisions"][0]["reasons"].as_array().unwrap().clone();

    assert!(reasons[0].as_str().unwrap().contains("Aucune règle"), "{reasons:?}");
}

#[tokio::test]
async fn the_explanation_panel_is_translated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    speak_french(&app).await;

    let response = app.get("/api/v1/media/m-1/explain").await;
    let trace = &response.assert_ok()["rule_traces"][0];

    assert!(trace["conditions"][0]["expected"].as_str().unwrap().contains("langue d'origine"));
    // The structured form travels alongside the wording.
    assert_eq!(trace["conditions"][0]["key"], "ConditionOriginalLanguage");
}

#[tokio::test]
async fn validation_messages_are_translated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    speak_french(&app).await;

    let response = app
        .post(
            "/api/v1/rules/validate",
            serde_json::json!({
                "name": "X",
                "media_type": "both",
                "target_category": "anime",
                "conditions": [
                    { "type": "has_files", "value": true },
                    { "type": "has_files", "value": false }
                ]
            }),
        )
        .await;
    let issues = response.assert_ok()["issues"].as_array().unwrap().clone();

    let contradiction = issues.iter().find(|i| i["key"] == "ValidationContradiction").unwrap();
    assert!(contradiction["message"].as_str().unwrap().contains("se contredisent"));
}

#[tokio::test]
async fn guardrail_refusals_are_translated() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response =
        app.post("/api/v1/decisions/apply", serde_json::json!({ "decision_ids": ["x"] })).await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.message().contains("simulation globale"), "{}", response.message());
}

#[tokio::test]
async fn diagnostics_warnings_are_translated() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response = app.get("/api/v1/status").await;
    let warnings = response.assert_ok()["warnings"].as_array().unwrap().clone();

    assert!(
        warnings.iter().any(|w| w.as_str().unwrap().contains("n'est pas authentifiée")),
        "{warnings:?}"
    );
}

/// Where the line falls between a translated refusal and an English one.
///
/// A refusal that names something the operator typed and says what to change is
/// content produced for them, and reads under the field that produced it. An
/// id the interface itself sent is a client fault and stays English, as it does
/// in Radarr and Sonarr — translating those would put the whole dictionary
/// behind every 404.
#[tokio::test]
async fn a_refusal_about_what_was_typed_is_translated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    speak_french(&app).await;

    // Not an absolute path: a form-level refusal.
    let refused = app
        .post("/api/v1/root-folders", serde_json::json!({ "instance_id": "i-1", "path": "films" }))
        .await;
    assert_eq!(refused.status, 400);
    assert!(
        refused.message().contains("chemin absolu"),
        "the form said its piece in English: {}",
        refused.message()
    );

    // A category the operator picked, already used by another folder.
    let taken = app
        .put("/api/v1/root-folders/rf-2/category", serde_json::json!({ "category": "standard" }))
        .await;
    assert_eq!(taken.status, 409);
    assert!(taken.message().contains("déjà associée"), "{}", taken.message());

    // And an id the interface sent, which is nobody's typing: still English.
    let missing = app.delete("/api/v1/root-folders/rf-does-not-exist").await;
    assert_eq!(missing.status, 404);
    assert!(missing.message().contains("No such folder"), "{}", missing.message());
}

#[tokio::test]
async fn mapping_conflicts_are_translated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    speak_french(&app).await;

    sqlx::query(
        "UPDATE root_folders SET accessible = 0, last_accessible_at = '2026-09-05 03:00:00'
         WHERE id = 'rf-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let response = app.get("/api/v1/root-folders/conflicts").await;
    let conflicts = response.assert_ok().as_array().unwrap().clone();

    let message = conflicts
        .iter()
        .find(|c| c["kind"] == "unreachable_root_folder")
        .map(|c| c["message"].as_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(message.contains("n'a pas répondu"), "{conflicts:?}");
    // The date travels with the sentence: it is what separates a nap from a
    // fault, and it must not be left in English beside a French clause.
    assert!(message.contains("2026-09-05"), "{conflicts:?}");
}

#[tokio::test]
async fn the_condition_catalog_is_translated() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response = app.get("/api/v1/rules/conditions").await;
    let conditions = response.assert_ok()["conditions"].as_array().unwrap().clone();

    let genre = conditions.iter().find(|c| c["type"] == "genre_contains").unwrap();
    assert_eq!(genre["label"], "Le genre contient");
    assert_eq!(genre["label_key"], "ConditionLabelGenreContains");
}

#[tokio::test]
async fn english_remains_the_default() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let response = app.post("/api/v1/simulate", serde_json::json!({})).await;
    let reasons = response.assert_ok()["decisions"][0]["reasons"].as_array().unwrap().clone();

    assert!(reasons[0].as_str().unwrap().contains("Original language"), "{reasons:?}");
}

/// A translation may land incomplete.
///
/// This is the contribution contract: requiring 453 keys before a language can
/// be merged means no language ever gets merged. What makes it safe is that
/// every gap is filled with English before the dictionary leaves the server.
#[tokio::test]
async fn the_served_dictionary_is_complete_even_for_a_partial_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let french = app.get("/api/v1/localization").await;
    let english = {
        let app = TestApp::new().await;
        app.get("/api/v1/localization").await
    };

    let french_keys = french.json["strings"].as_object().unwrap();
    let english_keys = english.json["strings"].as_object().unwrap();

    assert_eq!(
        french_keys.len(),
        english_keys.len(),
        "a language must never be served with fewer keys than English"
    );
    for key in english_keys.keys() {
        assert!(french_keys.contains_key(key), "{key} would render blank");
    }
}

/// The picker states how complete each language is, so an incomplete one can be
/// offered honestly rather than hidden or refused.
#[tokio::test]
async fn each_language_advertises_its_completion() {
    let app = TestApp::new().await;

    let response = app.get("/api/v1/localization/languages").await;
    let languages = response.assert_ok()["languages"].as_array().unwrap().clone();

    for language in &languages {
        let completion = language["completion"].as_u64().expect("completion is reported");
        assert!(completion <= 100, "{language:?}");
    }
    let english = languages.iter().find(|l| l["code"] == "en").unwrap();
    assert_eq!(english["completion"], 100, "the source language is complete by definition");
}

/// A fresh installation states its language rather than leaving the field
/// empty for every reader to fill in with a fallback of its own.
#[tokio::test]
async fn a_fresh_installation_speaks_english_by_default() {
    let app = TestApp::new().await;
    let settings = app.get("/api/v1/settings").await;
    assert_eq!(settings.assert_ok()["ui_language"], "en");
}
