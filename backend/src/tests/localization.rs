//! End-to-end behaviour of the translation layer.
//!
//! The interesting property is not that the interface shows French words but
//! that everything Routarr *produces* (justifications, diagnostics, guardrail
//! refusals) follows the same setting.

use axum::http::StatusCode;

use super::{TestApp, warning_messages};
use crate::localization::Localizer;

async fn speak_french(app: &TestApp) {
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "ui_language": "fr" } }))
        .await
        .assert_ok();
}

/// `key` in French, which a message is held to. Compared with the French
/// alone, a key not translated yet would pass on its English fallback.
fn in_french(key: &str, values: &[(&str, &str)]) -> String {
    let french = Localizer::new("fr").translate(key, values);
    assert_ne!(french, Localizer::new("en").translate(key, values), "{key} is not translated");
    french
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

/// The theme travels with the strings, unauthenticated, so the sign-in screen and a reader,
/// who cannot read the settings, paint in the one the owner set.
#[tokio::test]
async fn the_dictionary_carries_the_theme_set() {
    let app = TestApp::new().await;
    assert_eq!(app.get("/api/v1/localization").await.assert_ok()["theme"], "dark");

    app.store_setting("ui_theme", "light").await;
    assert_eq!(app.get("/api/v1/localization").await.assert_ok()["theme"], "light");
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

/// A task writes its detail while it runs, and the Tasks screen reads it later
/// in whatever language the interface speaks by then.
#[tokio::test]
async fn a_task_detail_is_read_in_the_interface_language() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    speak_french(&app).await;

    let jobs = app.get("/api/v1/tasks").await;
    let detail = jobs.assert_ok()["data"][0]["detail"].as_str().unwrap().to_string();

    let params = [("total", "1"), ("moves", "0")];
    let french = crate::localization::Localizer::new("fr").translate("JobDetailSimulated", &params);
    let english =
        crate::localization::Localizer::new("en").translate("JobDetailSimulated", &params);
    assert_ne!(french, english, "the French dictionary lacks the key");
    assert_eq!(detail, french);
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

    let response = app.get("/api/v1/library/m-1/explain").await;
    let trace = &response.assert_ok()["rule_traces"][0];

    assert!(trace["conditions"][0]["expected"].as_str().unwrap().contains("langue d'origine"));
    // The structured form travels alongside the wording.
    assert_eq!(trace["conditions"][0]["key"], "ConditionOriginalLanguage");
}

/// The caption names what the condition reads, so the sentence carries no
/// identifier of the engine, in any language.
#[tokio::test]
async fn the_no_source_warning_names_no_engine_identifier() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response = app
        .post(
            "/api/v1/rules/validate",
            serde_json::json!({
                "name": "X",
                "media_type": "both",
                "target_category": "anime",
                "conditions": [{ "type": "origin_country", "value": ["JP"] }]
            }),
        )
        .await;
    let issues = response.assert_ok()["issues"].as_array().unwrap().clone();

    let unanswered = issues
        .iter()
        .find(|i| i["key"] == "ValidationConditionNoSource")
        .expect("the Arr alone answers no origin country");
    let message = unanswered["message"].as_str().unwrap();
    assert!(!message.contains("origin_countries"), "{message}");
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

/// The global dry run is a state of the installation, not a fault in the
/// request: told apart by its reason, the one refusal a setting clears, and
/// said in the interface's language.
#[tokio::test]
async fn guardrail_refusals_are_translated() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let response =
        app.post("/api/v1/decisions/apply", serde_json::json!({ "decision_ids": ["x"] })).await;

    assert_eq!(response.assert_status(StatusCode::CONFLICT)["reason"], "dry_run");
    assert!(response.message().contains("essai à blanc global"), "{}", response.message());
}

/// A sync or a removal asked while the instance syncs is refused in the
/// interface language: the first names the instance the operator typed.
#[tokio::test]
async fn a_refusal_during_a_sync_is_translated() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    speak_french(&app).await;
    let _held = app.state.jobs.try_lock("sync:inst-1").expect("the sync lock");

    let synced = app.post("/api/v1/instances/inst-1/sync", serde_json::json!({})).await;
    let removed = app.delete("/api/v1/instances/inst-1").await;

    let name = [("name", "Fake radarr")];
    for (refused, key, params) in
        [(&synced, "ErrorSyncInProgress", &name[..]), (&removed, "ErrorInstanceSyncing", &[][..])]
    {
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.json);
        assert_eq!(refused.message(), in_french(key, params));
    }
}

/// A run keeps why a title goes where it goes as values, not as sentences:
/// read after the language changed, the reasons, the rules that lost and the
/// exception's name are all in the new one, and each line says whether it
/// states what decided.
#[tokio::test]
async fn a_decision_is_said_in_the_language_it_is_read_in() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let mut second = serde_json::json!({
        "name": "Animation too", "media_type": "both", "target_category": "anime",
        "conditions": [{ "type": "genre_contains", "value": ["Animation"] }]
    });
    second["priority"] = serde_json::json!(500);
    app.post("/api/v1/rules", second).await.assert_ok();
    app.simulate().await;

    speak_french(&app).await;
    let listed = app.get("/api/v1/decisions").await;
    let decision = listed.assert_ok()["data"][0].clone();

    // What a matched line says between its two values, in each language.
    let between = |language: &str| {
        let line = crate::localization::Localizer::new(language)
            .translate("ReasonMatched", &[("expected", "<"), ("observed", ">")]);
        line[line.find('<').unwrap() + 1..line.find('>').unwrap()].to_string()
    };
    let (french, english) = (between("fr"), between("en"));
    assert_ne!(french, english, "the French line is the English one");
    let reasons: Vec<&str> =
        decision["reasons"].as_array().unwrap().iter().filter_map(|r| r.as_str()).collect();
    assert!(!reasons.is_empty(), "{decision}");
    assert!(reasons.iter().all(|reason| reason.contains(&french)), "{reasons:?}");
    let lines = decision["reason_lines"].as_array().unwrap();
    assert!(lines.iter().all(|line| line["matched"] == true), "{lines:?}");
    let lost = decision["alternatives"][0]["reason"].as_str().unwrap();
    assert!(lost.contains(&french), "{lost}");
}

/// A row may hold its reasons as sentences: read as they are, with no claim
/// about what they state.
#[tokio::test]
async fn a_decision_stored_as_sentences_is_read_as_it_is() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                       target_category, action, status, decided_at, reasons, alternatives)
                   VALUES ('d-said', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move',
                           'pending', '2026-09-01 10:00:00', '[\"✓ Genre contains Animation\"]',
                           '[{\"rule_name\":\"Kids\",\"category\":\"kids\",\"reason\":\"✓ Family\"}]')"])
        .await;

    let listed = app.get("/api/v1/decisions").await;
    let decision = &listed.assert_ok()["data"][0];

    assert_eq!(decision["reasons"], serde_json::json!(["✓ Genre contains Animation"]));
    assert_eq!(decision["reason_lines"][0]["matched"], serde_json::Value::Null);
    assert_eq!(decision["alternatives"][0]["reason"], "✓ Family");
}

/// A failed move is kept as what went wrong rather than as a sentence: read
/// after the language changed, the move log and the history say it in the
/// new one, as the instance's own test says it.
#[tokio::test]
async fn a_failed_move_is_said_in_the_language_it_is_read_in() {
    let arr = super::fake_arr::FakeArr::start().await;
    let (app, decision) = super::one_move_ready(&arr).await;
    app.execute(&["UPDATE instances SET base_url = 'http://127.0.0.1:1'"]).await;
    let body = serde_json::json!({ "decision_ids": [decision] });
    let applied = app.post("/api/v1/decisions/apply", body).await;
    assert_eq!(applied.assert_ok()["failed"], 1, "{}", applied.json);

    speak_french(&app).await;
    let tested = app.post("/api/v1/instances/inst-1/test", serde_json::json!({})).await;
    let logs = app.get("/api/v1/move-log").await;
    let history = app.get("/api/v1/decisions?status=failed").await;
    let export = app.text("/api/v1/move-log/export").await;

    let french = tested.message();
    assert_eq!(logs.assert_ok()["data"][0]["error_message"].as_str(), Some(french.as_str()));
    assert_eq!(history.assert_ok()["data"][0]["error_message"].as_str(), Some(french.as_str()));
    let quoted = french.replace('"', "\"\"");
    assert!(export.contains(&quoted), "{export}");
}

#[tokio::test]
async fn diagnostics_warnings_are_translated() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let warnings = warning_messages(app.get("/api/v1/status").await.assert_ok());

    assert!(warnings.iter().any(|w| w.contains("n'est pas authentifiée")), "{warnings:?}");
}

/// Where the line falls between a translated refusal and an English one.
///
/// A refusal that names something the operator typed and says what to change is
/// content produced for them, and reads under the field that produced it. An
/// id the interface itself sent is a client fault and stays English, as it does
/// in Radarr and Sonarr. Translating those would put the whole dictionary
/// behind every 404.
/// A probe that cannot reach an Arr says what to change, in the interface
/// language, as the instance's own test says it, and the warning built from
/// it carries neither a state code nor the transport's English.
#[tokio::test]
async fn a_probed_instance_failure_is_explained_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;
    app.seed_instance_at("i-1", "radarr", "http://127.0.0.1:1").await;

    let health = app.get("/api/v1/health").await;
    let instance = health.assert_ok()["instances"][0].clone();
    let tested = app.post("/api/v1/instances/i-1/test", serde_json::json!({})).await;
    assert_eq!(instance["detail"].as_str(), Some(tested.message().as_str()), "{instance}");
    assert_eq!(instance["status"], "error");

    let status = app.get("/api/v1/status").await;
    let warnings = status.assert_ok()["warnings"].as_array().unwrap().clone();
    let unreachable = warnings
        .iter()
        .find(|warning| warning["code"] == "instance_unreachable")
        .expect("the probe's finding");
    let message = unreachable["message"].as_str().unwrap();
    assert!(message.contains(&tested.message()), "{message}");
    assert!(!message.contains("error:") && !message.contains("is unreachable"), "{message}");
}

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

    // A category name the operator typed, which another category holds.
    let duplicate = app.post("/api/v1/categories", serde_json::json!({ "name": "Anime" })).await;
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.message(), in_french("ErrorCategoryNameTaken", &[("name", "anime")]));

    // A category the operator deletes while a folder still answers to it.
    let in_use = app.delete("/api/v1/categories/cat-anime").await;
    assert_eq!(in_use.status, 409);
    let counts =
        [("name", "anime"), ("rules", "0"), ("folders", "1"), ("overrides", "0"), ("tests", "0")];
    assert_eq!(in_use.message(), in_french("ErrorCategoryInUse", &counts));

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

    app.execute(&["UPDATE root_folders SET accessible = 0,
                last_accessible_at = datetime('now', '-3 hours', '-5 minutes')
          WHERE id = 'rf-1'"])
        .await;

    let response = app.get("/api/v1/root-folders/conflicts").await;
    let conflicts = response.assert_ok().as_array().unwrap().clone();

    let message = conflicts
        .iter()
        .find(|c| c["kind"] == "unreachable_root_folder")
        .map(|c| c["message"].as_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(message.contains("n'a pas répondu"), "{conflicts:?}");
    // How long travels with the sentence, in its words: it is what separates
    // a nap from a fault, and a stored UTC time is not how anyone reads one.
    // French keeps the figure and its unit together, with a no-break space.
    assert!(message.contains("3\u{a0}h"), "{conflicts:?}");
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(!message.contains(&today), "{conflicts:?}");
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
/// empty for every reader to fill in with a fallback of its own, and speaks
/// it: a run's reasons read in English.
#[tokio::test]
async fn a_fresh_installation_speaks_english_by_default() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let settings = app.get("/api/v1/settings").await;
    assert_eq!(settings.assert_ok()["ui_language"], "en");

    let response = app.post("/api/v1/simulate", serde_json::json!({})).await;
    let reasons = response.assert_ok()["decisions"][0]["reasons"].as_array().unwrap().clone();
    assert!(reasons[0].as_str().unwrap().contains("Original language"), "{reasons:?}");
}

/// A key a language leaves untranslated is English, both in the dictionary
/// the interface loads and in the refusals the server words. Every shipped
/// language is complete, so a partial one only the tests know stands in.
#[tokio::test]
async fn a_key_a_language_leaves_untranslated_is_served_in_english() {
    let english = TestApp::new().await.get("/api/v1/localization").await.json["strings"].clone();
    let app = TestApp::new().await;
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "ui_language": "x-partial" } }))
        .await
        .assert_ok();

    let served = app.get("/api/v1/localization").await;
    let strings = &served.assert_ok()["strings"];
    assert_eq!(strings["Dashboard"], "Tableau partiel");
    assert_eq!(strings["Settings"], english["Settings"]);
    assert_eq!(strings.as_object().unwrap().len(), english.as_object().unwrap().len());

    let empty = app.post("/api/v1/categories", serde_json::json!({ "name": "" })).await;
    assert_eq!(empty.json["message"], "Nom partiel requis.");
    let refused = app.post("/api/v1/categories", serde_json::json!({ "name": "a b" })).await;
    assert_eq!(refused.json["message"], english["CategoryNameRefused"]);
}

/// A setting typed by hand and refused by the server alone is refused in the
/// interface language, naming the field: Settings shows it in a banner, away
/// from the field.
#[tokio::test]
async fn a_typed_setting_is_refused_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    for (key, value, said) in [
        (
            "notification_webhook_url",
            "ftp://example.org",
            "Webhook de notification accepte une adresse http:// ou https://.",
        ),
        ("certification_regions", " , ", "Régions de certification\u{a0}: au moins un code pays"),
        ("certification_regions", "FR, FRA", "«\u{a0}FRA\u{a0}»"),
    ] {
        let refused =
            app.put("/api/v1/settings", serde_json::json!({ "settings": { key: value } })).await;
        refused.assert_status(StatusCode::BAD_REQUEST);
        let message = refused.json["message"].as_str().unwrap_or_default();
        assert!(message.contains(said), "{key} = {value:?} answered {message:?}");
    }
}

/// The name of a rule test is typed on the Tests screen, so its refusal is too
/// read in the interface language.
#[tokio::test]
async fn a_rule_test_without_a_name_is_refused_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;

    let refused =
        app.post("/api/v1/rule-tests", serde_json::json!({ "name": " ", "media_id": "m-1" })).await;

    refused.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(refused.json["message"], "Un test a besoin d'un nom.");
}

/// A bundle is the operator's file: what an import leaves out of it is listed
/// in the interface's language, the reason included.
#[tokio::test]
async fn a_configuration_import_reports_its_skips_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;
    let bundle = serde_json::json!({
        "version": 1,
        "instances": [{
            "name": "Lidarr",
            "instance_type": "lidarr",
            "base_url": "http://127.0.0.1:1",
            "enabled": true,
            "sync_interval_minutes": 60
        }],
        "root_folders": [{ "instance_name": "Radarr", "path": "/music", "category": "concerts" }],
    });

    let report = app
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    let unknown_type = in_french("InstanceTypeUnknown", &[("type", "lidarr")]);
    let unmapped = [("path", "/music"), ("category", "concerts"), ("instance", "Radarr")];
    let expected = [
        in_french("ImportSkippedInstance", &[("name", "Lidarr"), ("reason", &unknown_type)]),
        in_french("ImportSkippedMappingNoCategory", &unmapped),
    ];
    assert_eq!(report["skipped"], serde_json::json!(expected), "{report}");
}

/// A file from a newer Routarr is refused in the interface's language, by
/// either import.
#[tokio::test]
async fn a_bundle_from_a_newer_routarr_is_refused_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;
    let newer = serde_json::json!({ "bundle": { "version": 3, "rules": [] } });

    let rules = app.post("/api/v1/rules/import", newer.clone()).await;
    let configuration = app.post("/api/v1/config/import", newer).await;

    assert_eq!(rules.status, 400);
    let read = [("version", "3"), ("last", "2")];
    assert_eq!(rules.message(), in_french("ErrorRuleBundleVersion", &read));
    assert_eq!(configuration.status, 400);
    let read = [("version", "3"), ("supported", "1")];
    assert_eq!(configuration.message(), in_french("ErrorBundleVersion", &read));
}

/// A rule test whose stored title no longer reads says so in the interface's
/// language, with a key a program reads, never with what the parser said.
#[tokio::test]
async fn a_rule_test_that_cannot_be_replayed_says_why_in_the_interface_language() {
    let app = TestApp::new().await;
    speak_french(&app).await;
    app.execute(&["INSERT INTO rule_tests (id, name, media_type, media_json, evaluated_at,
                                           expected_category)
                   VALUES ('t-1', 'Totoro', 'movie', '{\"title\":', '2026-09-21 10:00:00',
                           'anime')"])
        .await;

    let run = app.post("/api/v1/rule-tests/run", serde_json::json!({})).await.assert_ok().clone();

    let result = &run["results"][0];
    assert_eq!(result["passed"], false, "{run}");
    assert_eq!(result["error_key"], "RuleTestSnapshotUnreadable", "{run}");
    assert_eq!(result["error"], in_french("RuleTestSnapshotUnreadable", &[]), "{run}");
}

/// A damaged archive is refused under the Restore button, in the interface's
/// language.
#[tokio::test]
async fn a_restore_refusal_reads_in_the_interface_language() {
    let (app, dir) = super::backup::app_with_files("restore-in-french").await;
    app.seed_library().await;
    speak_french(&app).await;
    let by = crate::jobs::Attribution::manual(None);
    let file = crate::services::backup::create(&app.state, &by).await.unwrap();
    super::backup::damage_entry(&dir.join("backups").join(&file.name), "routarr.db");

    let refused =
        app.post(&format!("/api/v1/backups/{}/restore", file.name), serde_json::json!({})).await;

    assert_eq!(refused.status, 400, "{}", refused.json);
    assert_eq!(refused.message(), in_french("ErrorBackupDamaged", &[("entry", "routarr.db")]));
}
