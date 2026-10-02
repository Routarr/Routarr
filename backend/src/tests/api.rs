//! End-to-end tests over the HTTP surface.

use axum::body::Body;
use axum::http::{Request, StatusCode};

use super::fake_arr::FakeArr;
use super::{AN_INSTANCE, TestApp, database_through, warning_messages};
use tower::ServiceExt;

// ------------------------------------------------------------ authentication

#[tokio::test]
async fn ping_needs_no_credentials() {
    let app = TestApp::with_api_key("s3cret").await;
    app.get("/api/v1/ping").await.assert_ok();
}

/// Every response names its request, so a line in the server log and the
/// request that produced it can be matched from either end.
#[tokio::test]
async fn every_response_carries_a_request_id() {
    let app = TestApp::new().await;
    let response = app.raw("/api/v1/ping").await;
    let id = response.headers().get("x-request-id").expect("x-request-id header").to_str().unwrap();
    assert!(uuid::Uuid::parse_str(id).is_ok(), "not a uuid: {id}");
}

/// An id the caller chose is kept: a proxy in front, or a client correlating
/// its own retries, must be able to follow one id through.
#[tokio::test]
async fn a_request_id_supplied_by_the_caller_is_propagated() {
    let app = TestApp::new().await;
    let response = app
        .router
        .clone()
        .oneshot(
            Request::get("/api/v1/ping")
                .header("x-request-id", "trace-4711")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers().get("x-request-id").unwrap(), "trace-4711");
}

// ------------------------------------------------------------ instances

#[tokio::test]
async fn instance_api_keys_are_encrypted_at_rest_and_never_returned() {
    let app = TestApp::new().await;

    let created = app
        .post(
            "/api/v1/instances",
            serde_json::json!({
                "name": "Radarr",
                "instance_type": "radarr",
                "base_url": "http://radarr:7878/",
                "api_key": "plaintext-arr-key",
            }),
        )
        .await;
    let body = created.assert_ok();

    assert_eq!(body["api_key_encrypted"], true);
    assert!(!body.to_string().contains("plaintext-arr-key"));
    assert_eq!(body["base_url"], "http://radarr:7878", "trailing slash is trimmed");

    let stored: String = sqlx::query_scalar("SELECT api_key FROM instances")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("enc:v1:"), "stored value must be ciphertext: {stored}");
    assert_eq!(app.state.secrets.open(&stored).unwrap(), "plaintext-arr-key");
}

#[tokio::test]
async fn an_instance_gets_a_webhook_url() {
    let app = TestApp::new().await;
    let created = app
        .post(
            "/api/v1/instances",
            serde_json::json!({
                "name": "Radarr",
                "instance_type": "radarr",
                "base_url": "http://radarr:7878",
                "api_key": "k",
            }),
        )
        .await;

    let url = created.assert_ok()["webhook_url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/v1/webhook/"));
}

#[tokio::test]
async fn updating_without_an_api_key_keeps_the_stored_one() {
    let app = TestApp::new().await;
    let created = app
        .post(
            "/api/v1/instances",
            serde_json::json!({
                "name": "Radarr",
                "instance_type": "radarr",
                "base_url": "http://radarr:7878",
                "api_key": "original",
            }),
        )
        .await;
    let id = created.assert_ok()["id"].as_str().unwrap().to_string();

    app.put(
        &format!("/api/v1/instances/{id}"),
        serde_json::json!({
            "name": "Radarr renamed",
            "instance_type": "radarr",
            "base_url": "http://radarr:7878",
            "api_key": "",
        }),
    )
    .await
    .assert_ok();

    let stored: String = sqlx::query_scalar("SELECT api_key FROM instances WHERE id = ?")
        .bind(&id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(app.state.secrets.open(&stored).unwrap(), "original");
}

#[tokio::test]
async fn an_unknown_instance_type_is_rejected() {
    let app = TestApp::new().await;
    app.post(
        "/api/v1/instances",
        serde_json::json!({
            "name": "Lidarr",
            "instance_type": "lidarr",
            "base_url": "http://lidarr:8686",
            "api_key": "k",
        }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

// ------------------------------------------------------------ rules

fn anime_rule_body() -> serde_json::Value {
    serde_json::json!({
        "name": "Anime",
        "media_type": "both",
        "target_category": "anime",
        "conditions": [
            { "type": "original_language", "value": ["ja"] },
            { "type": "genre_contains", "value": ["Animation"] }
        ]
    })
}

#[tokio::test]
async fn creating_a_rule_returns_the_stored_row() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = app.post("/api/v1/rules", anime_rule_body()).await;
    let rule = body.assert_ok();

    assert_eq!(rule["name"], "Anime");
    assert_eq!(rule["match_mode"], "all");
    assert!(!rule["created_at"].as_str().unwrap().is_empty(), "created_at must be real");
}

#[tokio::test]
async fn a_contradictory_rule_is_rejected() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let mut body = anime_rule_body();
    body["conditions"] = serde_json::json!([
        { "type": "has_files", "value": true },
        { "type": "has_files", "value": false }
    ]);

    let response = app.post("/api/v1/rules", body).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.message().contains("contradict"));
}

#[tokio::test]
async fn a_rule_without_conditions_is_rejected() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let mut body = anime_rule_body();
    body["conditions"] = serde_json::json!([]);
    app.post("/api/v1/rules", body).await.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn validation_reports_warnings_without_blocking() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // Keywords come from a fetched source only, and the fixture has no key: the
    // condition is answerable by nothing, which is a warning and not a refusal.
    let mut body = anime_rule_body();
    body["conditions"] = serde_json::json!([{ "type": "keyword_contains", "value": ["mecha"] }]);

    let response = app.post("/api/v1/rules/validate", body).await;
    let result = response.assert_ok();
    assert_eq!(result["valid"], true, "warnings must not make a rule invalid");
    assert!(!result["issues"].as_array().unwrap().is_empty());
}

/// A condition is named as the editor shows it: by its section, its place in
/// that section and its caption, never by the engine's identifier.
#[tokio::test]
async fn a_condition_is_named_by_its_section_place_and_caption() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let mut body = anime_rule_body();
    let empty = serde_json::json!({ "type": "certification_in", "value": [] });
    body["conditions"] = serde_json::json!([
        { "type": "genre_contains", "value": ["Animation"] },
        empty.clone(),
    ]);
    body["exclusions"] = serde_json::json!([empty]);

    let response = app.post("/api/v1/rules/validate", body).await;
    let issues = response.assert_ok()["issues"].as_array().unwrap().clone();

    let localizer = app.state.localizer().await;
    let caption = localizer.translate("ConditionLabelCertificationIn", &[]);
    for (section, reference, index) in
        [("conditions", "ConditionReference", "2"), ("exclusions", "ExclusionReference", "1")]
    {
        let issue = issues
            .iter()
            .find(|i| i["key"] == "ValidationConditionEmpty" && i["field"] == section)
            .unwrap_or_else(|| panic!("no empty condition reported under {section}: {issues:?}"));
        let message = issue["message"].as_str().unwrap();
        let named = localizer.translate(reference, &[("index", index), ("label", &caption)]);
        assert!(message.starts_with(&named), "{section}: {message}");
        assert!(!message.contains("certification_in"), "{message}");
    }
}

#[tokio::test]
async fn a_condition_both_required_and_excluded_is_named_by_its_caption() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let mut body = anime_rule_body();
    let genre = serde_json::json!({ "type": "genre_contains", "value": ["Animation"] });
    body["conditions"] = serde_json::json!([genre.clone()]);
    body["exclusions"] = serde_json::json!([genre]);

    let response = app.post("/api/v1/rules/validate", body).await;
    let issues = response.assert_ok()["issues"].as_array().unwrap().clone();

    let conflict = issues.iter().find(|i| i["key"] == "ValidationExclusionConflict").unwrap();
    let message = conflict["message"].as_str().unwrap();
    let caption = app.state.localizer().await.translate("ConditionLabelGenreContains", &[]);
    assert!(message.contains(&caption), "{message}");
    assert!(!message.contains("genre_contains"), "{message}");
}

/// Stored, each of these would be a rule that never matches and reads on
/// screen exactly like a rule that correctly matches nothing.
#[tokio::test]
async fn a_condition_that_can_never_match_is_refused() {
    let app = TestApp::new().await;
    app.seed_library().await;

    for conditions in [
        serde_json::json!([{ "type": "genre_contains", "value": ["  "] }]),
        serde_json::json!([{ "type": "tag_in", "value": ["   "] }]),
        serde_json::json!([{ "type": "size_on_disk_over_gb", "value": 0 }]),
        serde_json::json!([{ "type": "year_range", "value": { "min": 2020, "max": 2000 } }]),
    ] {
        let mut body = anime_rule_body();
        body["conditions"] = conditions.clone();
        let response = app.post("/api/v1/rules", body).await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert!(response.message().contains("never match"), "{conditions}: {}", response.message());
    }
}

/// An import is validated as the editor validates, or a bundle could carry
/// everything the editor refuses. A bad rule is named and skipped rather than
/// failing the whole import: a bundle written elsewhere is expected to fit
/// imperfectly.
#[tokio::test]
async fn an_imported_bundle_is_validated_like_anything_else() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let bundle = serde_json::json!({
        "bundle": {
            "version": 1,
            "categories": ["anime"],
            "rules": [
                {
                    "name": "Sound",
                    "media_type": "movie",
                    "target_category": "anime",
                    "conditions": [{ "type": "genre_contains", "value": ["Animation"] }]
                },
                {
                    "name": "Inverted",
                    "media_type": "movie",
                    "target_category": "anime",
                    "conditions": [
                        { "type": "year_range", "value": { "min": 2020, "max": 2000 } }
                    ]
                },
                {
                    "name": "Backwards days",
                    "media_type": "movie",
                    "target_category": "anime",
                    "conditions": [{ "type": "added_within_days", "value": -5 }]
                }
            ]
        }
    });

    let response = app.post("/api/v1/rules/import", bundle).await;
    let body = response.assert_ok();

    assert_eq!(body["imported"], 1, "{body}");
    let skipped = body["skipped"].as_array().expect("skipped list");
    assert_eq!(skipped.len(), 2, "{body}");

    let stored: Vec<String> =
        sqlx::query_scalar("SELECT name FROM rules").fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(stored, vec!["Sound"]);
}

/// A decision records its trigger, as `jobs` does, so the history screen can
/// answer "did the nightly sweep propose this, or did I".
#[tokio::test]
async fn a_decision_records_what_caused_it() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await.assert_ok();

    let actors: Vec<Option<String>> =
        sqlx::query_scalar("SELECT actor FROM decisions").fetch_all(&app.state.pool).await.unwrap();
    assert!(!actors.is_empty(), "the run stored nothing");
    assert!(actors.iter().all(|a| a.as_deref() == Some("manual")), "{actors:?}");

    let listed = app.get("/api/v1/decisions").await;
    assert_eq!(listed.assert_ok()["data"][0]["actor"], "manual");
}

/// A subset rewrites priorities in the 10, 20, 30 band beside rules that were
/// never listed, and the resulting order is one nobody chose.
#[tokio::test]
async fn a_reorder_must_name_every_rule() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let first = app.post("/api/v1/rules", anime_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut second = anime_rule_body();
    second["name"] = serde_json::json!("Second");
    let second =
        app.post("/api/v1/rules", second).await.assert_ok()["id"].as_str().unwrap().to_string();

    app.post("/api/v1/rules/reorder", serde_json::json!({ "rule_ids": [first.clone()] }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    // Deduplicated before the comparison, `[a, a, b]` would read as `[a, b]`
    // and be written as 10, 20, 30, with `a` ending at 20, beside `b` at 30.
    app.post(
        "/api/v1/rules/reorder",
        serde_json::json!({ "rule_ids": [first.clone(), first.clone(), second.clone()] }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    app.post("/api/v1/rules/reorder", serde_json::json!({ "rule_ids": [second, first] }))
        .await
        .assert_ok();
}

/// The stock extractor answers a body it cannot parse in text/plain, outside
/// the envelope every other failure uses. A client that unwraps `{error,
/// message}` would show serde's sentence about a Rust field instead.
#[tokio::test]
async fn a_body_that_cannot_be_parsed_still_gets_the_error_envelope() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let app = TestApp::new().await;
    app.seed_library().await;

    let cases = [
        ("malformed", "application/json", "{bad", StatusCode::BAD_REQUEST, "bad_request"),
        (
            "missing field",
            "application/json",
            r#"{"name":"x"}"#,
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            "wrong content type",
            "text/plain",
            "{}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
    ];
    for (label, content_type, body, status, error) in cases {
        let request = Request::post("/api/v1/rules")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{label}");
        let content_type = response.headers()[header::CONTENT_TYPE].to_str().unwrap().to_string();
        assert!(content_type.starts_with("application/json"), "{label}: {content_type}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"], error, "{label}: {json}");
        assert!(json["message"].as_str().is_some_and(|m| !m.is_empty()), "{label}: {json}");
    }
}

/// A query string the stock extractor cannot parse is answered in text/plain,
/// which the interface cannot unwrap into a translated message.
#[tokio::test]
async fn a_query_that_cannot_be_parsed_still_gets_the_error_envelope() {
    let app = TestApp::new().await;
    for path in [
        "/api/v1/decisions?page=abc",
        "/api/v1/media?per_page=-1",
        "/api/v1/logs?success=maybe",
        "/api/v1/logs/export?success=maybe",
        "/api/v1/jobs?page=abc",
        "/api/v1/health?probe=maybe",
    ] {
        let response = app.get(path).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(response.json["error"], "bad_request", "{path}: {}", response.json);
        assert!(response.message().contains("query"), "{path}: {}", response.json);
    }
}

/// The scan finds a handler added later that reads its query through the
/// stock extractor, which the test above would not know to ask.
#[test]
fn no_handler_takes_the_stock_query_extractor() {
    let takes_stock = |source: &str| {
        source.contains("axum::extract::Query")
            || source.lines().any(|line| {
                line.trim_start().starts_with("use axum::extract::{")
                    && line.split(|c: char| !c.is_alphanumeric()).any(|word| word == "Query")
            })
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        // Where `api::Query` wraps the stock extractor.
        if path.ends_with("mod.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        read += 1;
        assert!(!takes_stock(&source), "{} takes axum's Query: use api::Query", path.display());
    }
    assert!(read > 10, "read {read} files under {}", dir.display());
}

#[tokio::test]
async fn a_single_rule_can_be_fetched_back() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let id = app.post("/api/v1/rules", anime_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let fetched = app.get(&format!("/api/v1/rules/{id}")).await;
    assert_eq!(fetched.assert_ok()["name"], "Anime");

    // An unknown id is a 404, which a route with no GET would answer as a 405.
    app.get("/api/v1/rules/nope").await.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_single_instance_can_be_fetched_back() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let fetched = app.get("/api/v1/instances/inst-1").await;
    let body = fetched.assert_ok();
    assert_eq!(body["name"], "Radarr");
    assert!(!body.to_string().contains("secret"), "the key must stay masked");

    app.get("/api/v1/instances/nope").await.assert_status(StatusCode::NOT_FOUND);
}

/// An edit stores what it was sent: the name, the switch and the interval,
/// which `MAX_SYNC_INTERVAL_MINUTES` bounds.
#[tokio::test]
async fn editing_an_instance_stores_its_name_switch_and_interval() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let edit = |interval: i64| {
        serde_json::json!({
            "name": "Radarr 4K", "instance_type": "radarr", "base_url": "http://127.0.0.1:1",
            "api_key": "", "enabled": false, "sync_interval_minutes": interval
        })
    };

    app.put("/api/v1/instances/inst-1", edit(120)).await.assert_ok();
    let stored = app.get("/api/v1/instances/inst-1").await.assert_ok().clone();
    assert_eq!(
        (&stored["name"], &stored["enabled"], &stored["sync_interval_minutes"]),
        (&serde_json::json!("Radarr 4K"), &serde_json::json!(false), &serde_json::json!(120))
    );

    app.put("/api/v1/instances/inst-1", edit(100_000)).await.assert_ok();
    let stored = app.get("/api/v1/instances/inst-1").await.assert_ok().clone();
    assert_eq!(stored["sync_interval_minutes"], crate::jobs::MAX_SYNC_INTERVAL_MINUTES);
}

/// An instance pointed at another address or another kind of Arr has pending
/// proposals naming the ids of the Arr it was: they are withdrawn, and the next
/// simulation after a sync proposes again. A rename keeps them.
#[tokio::test]
async fn pointing_an_instance_elsewhere_withdraws_its_proposals() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                          current_root_folder, target_root_folder,
                                          target_category, action, status)
                   VALUES ('d-1', 'm-1', 'Totoro', 'movie', 'inst-1', '/movies/standard',
                           '/movies/anime', 'anime', 'move', 'pending')"])
        .await;
    let edit = |base_url: &str, api_key: &str| {
        serde_json::json!({
            "name": "Renamed", "instance_type": "radarr", "base_url": base_url,
            "api_key": api_key, "enabled": true, "sync_interval_minutes": 60
        })
    };
    let withdrawn = "SELECT superseded FROM decisions WHERE id = 'd-1'";

    app.put("/api/v1/instances/inst-1", edit("http://127.0.0.1:1", "")).await.assert_ok();
    assert_eq!(app.count(withdrawn).await, 0, "a rename withdrew the proposals");

    let elsewhere = edit("http://127.0.0.1:2", "the-other-arrs-key");
    app.put("/api/v1/instances/inst-1", elsewhere).await.assert_ok();
    assert_eq!(app.count(withdrawn).await, 1, "the proposals outlived the address they name");
}

#[tokio::test]
async fn updating_a_missing_rule_is_a_404() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.put("/api/v1/rules/nope", anime_rule_body()).await.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicating_a_rule_produces_a_disabled_copy() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let id = app.post("/api/v1/rules", anime_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let copy = app.post(&format!("/api/v1/rules/{id}/duplicate"), serde_json::json!({})).await;
    let copy = copy.assert_ok();

    assert_eq!(copy["name"], "Anime (copy)");
    assert_eq!(copy["enabled"], false, "a copy must not start routing on its own");
    assert_ne!(copy["id"], serde_json::json!(id));
}

#[tokio::test]
async fn reorder_rewrites_priorities_in_order() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let first = app.post("/api/v1/rules", anime_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut second_body = anime_rule_body();
    second_body["name"] = serde_json::json!("Anime 2");
    let second = app.post("/api/v1/rules", second_body).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();

    app.post(
        "/api/v1/rules/reorder",
        serde_json::json!({ "rule_ids": [second.clone(), first.clone()] }),
    )
    .await
    .assert_ok();

    for (rule, expected) in [(&second, 10), (&first, 20)] {
        let priority: i64 = sqlx::query_scalar("SELECT priority FROM rules WHERE id = ?")
            .bind(rule)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(priority, expected, "the reorder did not give each rule its own place");
    }
}

/// Every field a rule carries, none of them at its default.
fn full_rule_body() -> serde_json::Value {
    serde_json::json!({
        "name": "Ghibli films",
        "description": "Films from the studio, whatever their language",
        "priority": 30,
        "enabled": true,
        "media_type": "movie",
        "match_mode": "any",
        "target_category": "kids",
        "instance_ids": ["inst-1"],
        "conditions": [
            { "type": "keyword_contains", "value": ["studio ghibli"] },
            { "type": "genre_contains", "value": ["Animation"] }
        ],
        "exclusions": [{ "type": "genre_contains", "value": ["Horror"] }]
    })
}

/// The library, with the `kids` category `full_rule_body` routes to.
async fn library_with_kids() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await.assert_ok();
    app
}

/// Each field of `full_rule_body` as `rule` holds it, but those `changed` names.
fn assert_kept(rule: &serde_json::Value, changed: &[(&str, serde_json::Value)], what: &str) {
    for (field, sent) in full_rule_body().as_object().unwrap() {
        let expected = changed.iter().find(|(name, _)| name == field).map_or(sent, |(_, v)| v);
        assert_eq!(&rule[field], expected, "{what} lost {field}");
    }
}

#[tokio::test]
async fn editing_a_rule_stores_every_field_it_was_sent() {
    let app = library_with_kids().await;
    let id = app.post("/api/v1/rules", anime_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();

    app.put(&format!("/api/v1/rules/{id}"), full_rule_body()).await.assert_ok();

    let stored = app.get(&format!("/api/v1/rules/{id}")).await;
    assert_kept(stored.assert_ok(), &[], "the edit");
}

#[tokio::test]
async fn a_duplicate_and_an_export_keep_every_field_of_a_rule() {
    let app = library_with_kids().await;
    let id = app.post("/api/v1/rules", full_rule_body()).await.assert_ok()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let copy = app.post(&format!("/api/v1/rules/{id}/duplicate"), serde_json::json!({})).await;
    let copy_changes =
        [("name", serde_json::json!("Ghibli films (copy)")), ("enabled", serde_json::json!(false))];
    assert_kept(copy.assert_ok(), &copy_changes, "the duplicate");

    let bundle = app.get("/api/v1/rules/export").await.assert_ok().clone();
    let replace = serde_json::json!({ "bundle": bundle, "replace": true });
    app.post("/api/v1/rules/import", replace).await.assert_ok();
    let rules = app.get("/api/v1/rules").await.assert_ok().clone();
    let imported = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["name"] == "Ghibli films")
        .expect("the rule came back");
    // Instance ids name this installation's instances, and a bundle travels.
    let host_specific = [("instance_ids", serde_json::Value::Null)];
    assert_kept(imported, &host_specific, "the export and import");
}

#[tokio::test]
async fn rules_round_trip_through_export_and_import() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/rules", anime_rule_body()).await.assert_ok();

    let bundle = app.get("/api/v1/rules/export").await.assert_ok().clone();
    assert_eq!(bundle["version"], 1);
    assert_eq!(bundle["rules"].as_array().unwrap().len(), 1);

    let imported = app
        .post("/api/v1/rules/import", serde_json::json!({ "bundle": bundle, "replace": true }))
        .await;
    assert_eq!(imported.assert_ok()["imported"], 1);

    let rules = app.get("/api/v1/rules").await.assert_ok().clone();
    assert_eq!(rules.as_array().unwrap().len(), 1, "replace must not duplicate");
}

/// A replace whose every rule is refused would delete the rules in place and
/// add none, and the next pass would route the whole library to the fallback
/// category. It is refused whole, and the rules stay.
#[tokio::test]
async fn a_replacing_import_that_keeps_nothing_deletes_nothing() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/rules", anime_rule_body()).await.assert_ok();

    let bundle = serde_json::json!({
        "version": 1,
        "rules": [{
            "name": "Inverted",
            "media_type": "movie",
            "target_category": "anime",
            "conditions": [{ "type": "year_range", "value": { "min": 2020, "max": 2000 } }]
        }]
    });
    let response = app
        .post("/api/v1/rules/import", serde_json::json!({ "bundle": bundle, "replace": true }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let rules = app.get("/api/v1/rules").await.assert_ok().clone();
    assert_eq!(rules.as_array().unwrap().len(), 1, "the rules in place were deleted: {rules}");
}

/// A bundle brings a category with it, and the rules targeting it arrive too.
/// Judged against a list read before the category existed, every rule would be
/// skipped as naming a category that does not exist, `imported: 0` beside a
/// freshly created row.
#[tokio::test]
async fn importing_creates_missing_categories() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let bundle = serde_json::json!({
        "version": 1,
        "rules": [{
            "name": "Concerts",
            "media_type": "movie",
            "target_category": "concerts",
            "conditions": [{ "type": "keyword_contains", "value": ["concert"] }]
        }],
        "categories": ["concerts"]
    });

    let body = app
        .post("/api/v1/rules/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["imported"], 1, "{body}");
    assert_eq!(body["skipped"].as_array().map(Vec::len), Some(0), "{body}");

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = 'concerts')")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(exists);
    let target: Option<String> =
        sqlx::query_scalar("SELECT target_category FROM rules WHERE name = 'Concerts'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(target.as_deref(), Some("concerts"), "the rule was not stored");
}

/// An export lists the categories its rules target, and a bundle written by
/// hand need not. The rule is then the only thing naming its category.
#[tokio::test]
async fn a_rule_whose_category_only_it_names_is_imported_with_it() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let bundle = serde_json::json!({
        "version": 1,
        "rules": [{
            "name": "Concerts",
            "media_type": "movie",
            "target_category": "Concerts",
            "conditions": [{ "type": "keyword_contains", "value": ["concert"] }]
        }]
    });

    let body = app
        .post("/api/v1/rules/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["imported"], 1, "{body}");

    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM categories ORDER BY name")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        names,
        ["anime", "concerts", "standard"],
        "created lowercased, as `POST /categories` stores it"
    );
}

/// The import creates categories through the gate `POST /categories` applies.
/// Trimmed and lowercased alone, `Kids & Family` would land in the table as a
/// name `rename` refuses to touch, with a rule stored against it.
#[tokio::test]
async fn a_category_the_api_would_refuse_is_not_created_by_a_bundle() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let bundle = serde_json::json!({
        "version": 1,
        "rules": [{
            "name": "Family",
            "media_type": "movie",
            "target_category": "Kids & Family",
            "conditions": [{ "type": "keyword_contains", "value": ["family"] }]
        }],
        "categories": ["Kids & Family"]
    });

    let body = app
        .post("/api/v1/rules/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["imported"], 0, "{body}");
    let skipped: Vec<&str> =
        body["skipped"].as_array().unwrap().iter().filter_map(|s| s.as_str()).collect();
    assert_eq!(skipped.len(), 2, "the category once, the rule once: {skipped:?}");
    assert!(
        skipped.iter().any(|s| s.contains("Kids & Family") && s.contains("letters, digits")),
        "the refusal names the category and the reason: {skipped:?}"
    );
    assert!(
        skipped.iter().any(|s| s.starts_with("'Family'") && s.contains("does not exist")),
        "the rule is refused for the category it cannot have: {skipped:?}"
    );

    let created: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE name LIKE '%family%'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(created, 0, "the refused name reached the table");
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rules WHERE name = 'Family'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);
}

/// Told to create nothing, the import judges against the table as it stands.
#[tokio::test]
async fn an_import_that_creates_no_category_refuses_a_rule_targeting_an_unknown_one() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let bundle = serde_json::json!({
        "version": 1,
        "rules": [{
            "name": "Concerts",
            "media_type": "movie",
            "target_category": "concerts",
            "conditions": [{ "type": "keyword_contains", "value": ["concert"] }]
        }],
        "categories": ["concerts"]
    });

    let body = app
        .post(
            "/api/v1/rules/import",
            serde_json::json!({ "bundle": bundle, "create_missing_categories": false }),
        )
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["imported"], 0, "{body}");
    let skipped = body["skipped"][0].as_str().unwrap_or_default();
    assert!(skipped.contains("'concerts' does not exist"), "{body}");

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = 'concerts')")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(!exists, "nothing was to be created");
}

#[tokio::test]
async fn an_unsupported_bundle_version_is_rejected() {
    let app = TestApp::new().await;
    app.post(
        "/api/v1/rules/import",
        serde_json::json!({ "bundle": { "version": 99, "rules": [] } }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_shows_the_impact_without_persisting() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response =
        app.post("/api/v1/rules/preview", serde_json::json!({ "rule": anime_rule_body() })).await;
    let preview = response.assert_ok();

    assert_eq!(preview["changed_total"], 1);
    assert_eq!(preview["changed"][0]["to_category"], "anime");
    assert_eq!(preview["changed"][0]["from_category"], "standard");

    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, 0, "a preview must not write decisions");

    let rules: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rules").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(rules, 0, "a preview must not create the rule");
}

#[tokio::test]
async fn the_condition_catalog_is_served() {
    let app = TestApp::new().await;
    let catalog = app.get("/api/v1/rules/conditions").await;
    let catalog = catalog.assert_ok();
    assert!(catalog["conditions"].as_array().unwrap().len() >= 15);
    assert_eq!(catalog["match_modes"], serde_json::json!(["all", "any"]));
}

// ------------------------------------------------------------ categories

/// Categories are joined by name with no foreign key, so each thing naming
/// one keeps it: a rule, a folder mapping, an exception and a rule test, each
/// alone. One nothing names goes.
#[tokio::test]
async fn a_category_in_use_cannot_be_deleted() {
    let uses: [&'static str; 4] = [
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, exclusions,
                            match_mode, target_category)
         VALUES ('r-k', 'Kids', 10, 1, 'both', '[]', '[]', 'all', 'kids')",
        "UPDATE root_folders SET category = 'kids' WHERE id = 'rf-1'",
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-k', 'm-1', 'kids')",
        "INSERT INTO rule_tests (id, name, media_type, media_json, evaluated_at, expected_category)
         VALUES ('t-k', 'Totoro is for kids', 'movie', '{}', '2026-09-01 00:00:00', 'kids')",
    ];
    for using in uses {
        let app = TestApp::new().await;
        app.seed_library().await;
        app.execute(&["INSERT INTO categories (id, name) VALUES ('cat-kids', 'kids')", using])
            .await;

        let response = app.delete("/api/v1/categories/cat-kids").await;

        response.assert_status(StatusCode::CONFLICT);
        assert!(response.message().contains("still in use"), "{using}: {}", response.message());
    }
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["INSERT INTO categories (id, name) VALUES ('cat-kids', 'kids')"]).await;
    app.delete("/api/v1/categories/cat-kids").await.assert_ok();
}

#[tokio::test]
async fn a_category_name_is_trimmed_and_lowercased() {
    let app = TestApp::new().await;

    let created = app.post("/api/v1/categories", serde_json::json!({ "name": "  Kids  " })).await;
    assert_eq!(created.assert_ok()["name"], "kids");
}

// ------------------------------------------------------------ root folders

#[tokio::test]
async fn mapping_the_same_category_twice_on_one_instance_is_refused() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app
        .put("/api/v1/root-folders/rf-1/category", serde_json::json!({ "category": "anime" }))
        .await;
    response.assert_status(StatusCode::CONFLICT);
}

/// The ambiguity is within one instance: a category mapped on each of two
/// instances is one folder per instance, which is what routing reads.
#[tokio::test]
async fn one_category_is_mapped_once_on_each_instance() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_instance_at("inst-2", "radarr", "http://127.0.0.1:1").await;
    app.execute(&["INSERT INTO root_folders (id, instance_id, arr_id, path, accessible)
                   VALUES ('rf-other', 'inst-2', 1, '/data/anime', 1)"])
        .await;

    let mapped = app
        .put("/api/v1/root-folders/rf-other/category", serde_json::json!({ "category": "anime" }))
        .await;

    mapped.assert_ok();
    assert_eq!(app.count("SELECT COUNT(*) FROM root_folders WHERE category = 'anime'").await, 2);
}

#[tokio::test]
async fn conflicts_report_unmapped_categories() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/rules", anime_rule_body()).await.assert_ok();

    // Unmap the anime folder so the rule now targets nothing.
    app.put("/api/v1/root-folders/rf-2/category", serde_json::json!({ "category": null }))
        .await
        .assert_ok();

    let conflicts = app.get("/api/v1/root-folders/conflicts").await;
    let conflicts = conflicts.assert_ok().as_array().unwrap().clone();
    assert!(conflicts.iter().any(|c| c["kind"] == "unmapped_category"));
}

/// A movie-only rule cannot ever route anything into a Sonarr, so warning that
/// Sonarr has no folder for its category is a warning nobody can act on. A
/// perfectly ordinary setup, movie categories on Radarr and series categories
/// on Sonarr, would fill the page with warnings that can never be cleared,
/// which is how users learn to ignore them.
#[tokio::test]
async fn a_movie_rule_does_not_warn_about_a_sonarr_that_could_never_receive_it() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-2', 'Sonarr', 'sonarr', 'http://sonarr:8989', 'k', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-concerts', 'concerts')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let mut rule = anime_rule_body();
    rule["name"] = "Concert films".into();
    rule["media_type"] = "movie".into();
    rule["target_category"] = "concerts".into();
    app.post("/api/v1/rules", rule).await.assert_ok();

    let conflicts = app.get("/api/v1/root-folders/conflicts").await;
    let conflicts = conflicts.assert_ok().as_array().unwrap().clone();

    let against_sonarr: Vec<_> = conflicts
        .iter()
        .filter(|c| c["kind"] == "unmapped_category" && c["instance_name"] == "Sonarr")
        .collect();
    assert!(against_sonarr.is_empty(), "unactionable warning: {against_sonarr:?}");

    // Radarr genuinely has no `concerts` folder, so that one must still show.
    assert!(
        conflicts.iter().any(|c| c["kind"] == "unmapped_category"
            && c["instance_name"] == "Radarr"
            && c["category"] == "concerts"),
        "the actionable warning disappeared: {conflicts:?}"
    );
}

// ------------------------------------------------------------ overrides

#[tokio::test]
async fn removing_an_override_hands_the_media_back_to_the_rules() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let created = app
        .post(
            "/api/v1/overrides",
            serde_json::json!({ "media_id": "m-1", "target_category": "anime" }),
        )
        .await;
    let id = created.assert_ok()["id"].as_str().unwrap().to_string();

    app.delete(&format!("/api/v1/overrides/{id}")).await.assert_ok();

    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM overrides WHERE id = ?")
        .bind(&id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);

    // Twice is a 404, not a 500: the screen offers the button until it reloads.
    app.delete(&format!("/api/v1/overrides/{id}"))
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
}

/// The list the Overrides screen renders, read as the screen reads it: with
/// the key, under a mode that demands one. Each entry carries the item's title
/// and the instance it lives on, which the list joins in.
#[tokio::test]
async fn the_override_list_answers_with_the_key_and_names_each_item() {
    let app = TestApp::with_api_key("s3cret").await;
    app.seed_library().await;
    let keyed = |method: &str, body: serde_json::Value| {
        Request::builder()
            .method(method)
            .uri("/api/v1/overrides")
            .header("x-api-key", "s3cret")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    let body = serde_json::json!({ "media_id": "m-1", "target_category": "anime" });
    app.send(keyed("POST", body)).await.assert_ok();

    let listed = app.send(keyed("GET", serde_json::Value::Null)).await;
    let listed = listed.assert_ok().as_array().expect("a list").clone();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["media_id"], "m-1");
    assert_eq!(listed[0]["target_category"], "anime");
    assert_eq!(listed[0]["media_title"], "My Neighbor Totoro");
    assert_eq!(listed[0]["instance_name"], "Radarr");

    app.get("/api/v1/overrides").await.assert_status(StatusCode::UNAUTHORIZED);
}

/// An override pins a title to a category, and that is all it does: it wins
/// over every rule already. A lock on it would protect nothing, so the API
/// answers none, and an older client still sending one is heard.
#[tokio::test]
async fn an_override_carries_no_lock() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let created = app
        .post(
            "/api/v1/overrides",
            serde_json::json!({ "media_id": "m-1", "target_category": "anime", "locked": true }),
        )
        .await;

    let created = created.assert_ok().clone();
    assert!(created.get("locked").is_none(), "{created}");
    let listed = app.get("/api/v1/overrides").await.assert_ok().clone();
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert!(listed[0].get("locked").is_none(), "{listed}");
    let detail = app.get("/api/v1/media/m-1").await.assert_ok().clone();
    assert!(detail["override"].get("locked").is_none(), "{detail}");
}

/// An upgrade drops the column and keeps every override.
#[tokio::test]
async fn an_upgrade_drops_the_override_lock_and_keeps_the_override() {
    let pool = database_through("005_jikan_film_misses").await;
    sqlx::query(AN_INSTANCE).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title) VALUES ('m-1', 'inst-1', 1, 'movie', 'Akira')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, locked) VALUES ('o-1', 'm-1', 'anime', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('overrides')")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!columns.contains(&"locked".to_string()), "{columns:?}");
    let kept: String = sqlx::query_scalar("SELECT target_category FROM overrides WHERE id = 'o-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, "anime");
}

#[tokio::test]
async fn an_override_returns_the_stored_id_on_upsert() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let first = app
        .post(
            "/api/v1/overrides",
            serde_json::json!({ "media_id": "m-1", "target_category": "anime" }),
        )
        .await;
    let first_id = first.assert_ok()["id"].as_str().unwrap().to_string();

    let second = app
        .post(
            "/api/v1/overrides",
            serde_json::json!({ "media_id": "m-1", "target_category": "standard" }),
        )
        .await;
    let second_body = second.assert_ok();

    assert_eq!(second_body["id"].as_str().unwrap(), first_id, "the row id must not change");
    assert_eq!(second_body["target_category"], "standard");
}

#[tokio::test]
async fn an_override_on_an_unknown_category_is_rejected() {
    let app = TestApp::new().await;
    app.seed_library().await;

    app.post(
        "/api/v1/overrides",
        serde_json::json!({ "media_id": "m-1", "target_category": "nope" }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creating_an_override_supersedes_pending_decisions() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    app.post(
        "/api/v1/overrides",
        serde_json::json!({ "media_id": "m-1", "target_category": "standard" }),
    )
    .await
    .assert_ok();

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}

/// The proposal an override made is wrong once the override goes: the rules
/// decide again, and a stale move would still be applied from Simulation.
#[tokio::test]
async fn removing_an_override_supersedes_pending_decisions() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let created = app
        .post(
            "/api/v1/overrides",
            serde_json::json!({ "media_id": "m-1", "target_category": "anime" }),
        )
        .await
        .assert_ok()
        .clone();
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    let pending = app.get("/api/v1/decisions?status=pending").await.assert_ok().clone();
    assert_eq!(pending["pagination"]["total"], 1, "the override must propose a move");

    let id = created["id"].as_str().unwrap();
    app.delete(&format!("/api/v1/overrides/{id}")).await.assert_ok();

    let pending = app.get("/api/v1/decisions?status=pending").await.assert_ok().clone();
    assert_eq!(pending["pagination"]["total"], 0, "{pending}");
}

// ------------------------------------------------------------ settings

#[tokio::test]
async fn unknown_settings_are_rejected() {
    let app = TestApp::new().await;
    let response = app
        .put("/api/v1/settings", serde_json::json!({ "settings": { "global_dryrun": "false" } }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.message().contains("Unknown setting"));
}

#[tokio::test]
async fn invalid_setting_values_are_rejected_before_any_write() {
    let app = TestApp::new().await;

    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "batch_limit": "10", "global_dry_run": "maybe" } }),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);

    let batch_limit: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'batch_limit'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(batch_limit, "50", "the valid half must not be applied either");
}

#[tokio::test]
async fn valid_settings_are_stored() {
    let app = TestApp::new().await;
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "global_dry_run": "false", "batch_limit": "5" } }),
    )
    .await
    .assert_ok();

    assert!(!app.state.bool_setting("global_dry_run", true).await);
    assert_eq!(app.state.setting("batch_limit", 0usize).await, 5);
}

/// A client reading every setting and writing them all back, as a script
/// backing them up does, is not refused, and leaves a stored credential as it
/// was: the credential is never read back, so it is never written over.
#[tokio::test]
async fn every_setting_read_can_be_written_back_as_it_came() {
    let app = TestApp::new().await;
    let key = serde_json::json!({ "settings": { "tmdb_api_key": "a-tmdb-key" } });
    app.put("/api/v1/settings", key).await.assert_ok();
    let read = app.get("/api/v1/settings").await.assert_ok().clone();
    let values: serde_json::Map<String, serde_json::Value> = read
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, value)| value.is_string())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    app.put("/api/v1/settings", serde_json::json!({ "settings": values })).await.assert_ok();
    let after = app.get("/api/v1/settings").await;
    assert_eq!(after.assert_ok()["tmdb_api_key_configured"], true, "the write-back erased it");
}

// ------------------------------------------------------------ decisions

#[tokio::test]
async fn applying_is_blocked_while_dry_run_is_on() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    let ids = pending_ids(&app).await;
    let response =
        app.post("/api/v1/decisions/apply", serde_json::json!({ "decision_ids": ids })).await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.message().contains("dry-run"));
}

#[tokio::test]
async fn applying_more_than_the_batch_limit_is_refused() {
    let app = TestApp::new().await;
    disable_dry_run(&app).await;
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "batch_limit": "1" } }))
        .await
        .assert_ok();

    let response = app
        .post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": ["a", "b"], "confirm": ["capacity", "threshold"] }),
        )
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.message().contains("above the batch limit"));
}

#[tokio::test]
async fn a_large_batch_needs_explicit_confirmation() {
    let app = TestApp::new().await;
    disable_dry_run(&app).await;
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "confirmation_threshold": "1", "batch_limit": "50" } }),
    )
    .await
    .assert_ok();

    let response = app
        .post("/api/v1/decisions/apply", serde_json::json!({ "decision_ids": ["a", "b"] }))
        .await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json["error"], "confirmation_required",
        "the client keys on this code, not on the message text"
    );
}

#[tokio::test]
async fn applying_nothing_is_a_bad_request() {
    let app = TestApp::new().await;
    disable_dry_run(&app).await;
    app.post("/api/v1/decisions/apply", serde_json::json!({ "decision_ids": [] }))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn superseded_decisions_are_hidden_by_default() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    let visible = app.get("/api/v1/decisions").await.assert_ok().clone();
    assert_eq!(visible["pagination"]["total"], 1, "only the newest proposal is actionable");

    let all = app.get("/api/v1/decisions?include_superseded=true").await.assert_ok().clone();
    assert_eq!(all["pagination"]["total"], 2);
}

/// Deleting an instance takes its proposals with it: they would move titles
/// that no longer exist, and the dashboard would go on counting them. What was
/// applied stays, since the history is the record of a write that happened.
#[tokio::test]
async fn deleting_an_instance_retires_its_proposals_and_keeps_its_history() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
         target_category, action, status)
         VALUES ('d-applied', 'm-1', 'My Neighbor Totoro', 'movie', 'inst-1', 'anime', 'move',
                 'applied')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let pending = app.get("/api/v1/decisions?status=pending").await.assert_ok().clone();
    assert_eq!(pending["pagination"]["total"], 1, "the fixture must propose a move");

    app.delete("/api/v1/instances/inst-1").await.assert_ok();

    let pending = app.get("/api/v1/decisions?status=pending").await.assert_ok().clone();
    assert_eq!(pending["pagination"]["total"], 0, "{pending}");
    let status = app.get("/api/v1/status").await.assert_ok().clone();
    assert_eq!(status["pending_decisions"], 0);
    let applied = app.get("/api/v1/decisions?status=applied").await.assert_ok().clone();
    assert_eq!(applied["pagination"]["total"], 1, "the history is kept: {applied}");
}

/// An upgrade retires the pending proposals of an instance already deleted,
/// which a database through migration 003 can hold, and leaves alone the
/// proposals of an instance that exists and whatever was applied.
#[tokio::test]
async fn an_upgrade_retires_the_proposals_of_an_instance_already_deleted() {
    let pool = database_through("003_metadata_sources").await;
    sqlx::query(AN_INSTANCE).execute(&pool).await.unwrap();
    for (id, instance, status) in [
        ("d-applied", "inst-gone", "applied"),
        ("d-kept", "inst-1", "pending"),
        ("d-orphan", "inst-gone", "pending"),
    ] {
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
             target_category, action, status)
             VALUES (?, 'm-1', 'My Neighbor Totoro', 'movie', ?, 'anime', 'move', ?)",
        )
        .bind(id)
        .bind(instance)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    let retired: Vec<(String, bool)> =
        sqlx::query_as("SELECT id, superseded FROM decisions ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let expected = [("d-applied", false), ("d-kept", false), ("d-orphan", true)];
    assert_eq!(retired, expected.map(|(id, flag)| (id.to_string(), flag)));
}

// ------------------------------------------------------------ media

#[tokio::test]
async fn explain_traces_every_applicable_rule() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let response = app.get("/api/v1/media/m-1/explain").await;
    let explanation = response.assert_ok();

    assert_eq!(explanation["target_category"], "anime");
    assert_eq!(explanation["action"], "move");
    assert_eq!(explanation["target_root_folder"], "/movies/anime");
    assert_eq!(explanation["winning_rule"], "Anime");

    let trace = &explanation["rule_traces"][0];
    assert_eq!(trace["outcome"], "winner");
    assert_eq!(trace["conditions"].as_array().unwrap().len(), 2);
    assert_eq!(trace["conditions"][0]["matched"], true);
}

#[tokio::test]
async fn explain_reports_the_default_when_nothing_matches() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/media/m-1/explain").await;
    let explanation = response.assert_ok();
    assert_eq!(explanation["target_category"], "standard");
    assert_eq!(explanation["action"], "none");
    assert_eq!(explanation["confidence"], 0.0);
}

#[tokio::test]
async fn explain_on_unknown_media_is_a_404() {
    let app = TestApp::new().await;
    app.get("/api/v1/media/nope/explain").await.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_media_list_exposes_the_computed_category() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    let list = app.get("/api/v1/media").await.assert_ok().clone();
    assert_eq!(list["data"][0]["computed_category"], "anime");
    assert_eq!(list["data"][0]["has_metadata"], true);
    assert_eq!(list["data"][0]["instance_name"], "Radarr");
}

#[tokio::test]
async fn pagination_is_clamped() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let list = app.get("/api/v1/media?per_page=100000&page=0").await.assert_ok().clone();
    assert_eq!(list["pagination"]["per_page"], 200);
    assert_eq!(list["pagination"]["page"], 1);
}

// ------------------------------------------------------------ webhooks

/// The only route an unauthenticated party reaches, and each accepted call
/// costs a request to the Arr, a metadata fetch, a simulation and, with
/// automatic application armed, a write. The token travels in the URL, so it
/// sits in the proxy's log and in Radarr's own, and once it is read, the lock
/// each instance's deliveries share is what bounds what it can start.
///
/// Serialised rather than dropped: an Arr does not retry a webhook it considers
/// delivered, and the run this guards covers *one* media item, so a skipped
/// delivery would leave that item unsynced until the next sweep, or for ever
/// with `auto_sync_enabled` off.
#[tokio::test]
async fn a_second_delivery_waits_for_the_first_rather_than_being_dropped() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    // Held the way a delivery in progress holds it, then released while the
    // second is waiting, which is what an ordinary sequential import does.
    let held = app.state.jobs.try_lock("webhook:inst-1").expect("the key is free");
    let releasing = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drop(held);
    });

    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "Download", "movie": { "id": 10 } }),
        )
        .await;
    releasing.await.unwrap();

    // It reached the Arr: the item was synced, which a dropped delivery never
    // does. A body without the row would pass a check on `ignored` alone.
    let body = response.assert_ok();
    assert_eq!(
        body["media_id"], "m-inst-1-10",
        "the delivery was dropped instead of waiting: {body}"
    );
    assert!(
        app.state.jobs.try_lock("webhook:inst-1").is_some(),
        "the lock must be released with the delivery"
    );
}

/// The wait is what bounds this route's cost to an unauthenticated caller, and
/// a queue of waiters with no bound of its own hands that cost straight back:
/// each one is a connection and a task for as long as the wait lasts.
#[tokio::test]
async fn a_delivery_past_the_queue_bound_is_acknowledged_without_waiting() {
    use crate::api::webhook::{DELIVERY_WAIT, MAX_WAITING_DELIVERIES};

    let app = TestApp::new().await;
    app.seed_library().await;

    let _held = app.state.jobs.try_lock("webhook:inst-1").expect("the key is free");
    let _places: Vec<_> = (0..MAX_WAITING_DELIVERIES)
        .map(|_| {
            app.state.jobs.try_wait("webhook:inst-1", MAX_WAITING_DELIVERIES).expect("a place")
        })
        .collect();

    let started = std::time::Instant::now();
    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "Download", "movie": { "id": 1 } }),
        )
        .await;
    let body = response.assert_ok();
    assert_eq!(body["ignored"], "too many deliveries are waiting for the instance", "{body}");
    // Answered before any timer, so half the budget is a margin that cannot
    // go flaky, and a delivery that waited the budget out is caught.
    assert!(
        started.elapsed() < DELIVERY_WAIT / 2,
        "it waited instead of answering: {:?}",
        started.elapsed()
    );
    assert_eq!(
        app.state.jobs.waiting_for("webhook:inst-1"),
        MAX_WAITING_DELIVERIES,
        "the refused delivery must not have taken a place"
    );
}

/// Removing the last episode file flips `has_files`, and a rule reading it then
/// routes the series elsewhere. Sonarr's `EpisodeFileDelete` is acted on as
/// Radarr's `MovieFileDelete` is.
#[tokio::test]
async fn sonarr_deleting_an_episode_file_is_an_event_worth_acting_on() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "EpisodeFileDelete", "series": { "id": 1 } }),
        )
        .await;

    // Not in the ignored list. The instance points nowhere, so the sync behind
    // it fails, which is itself the proof the event got through.
    let ignored =
        response.json.get("ignored").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    assert_ne!(ignored, "EpisodeFileDelete", "the event was still being skipped");
}

/// Rotating is only worth offering if it actually revokes: without this,
/// nothing says the previous token stops being accepted, which is the entire
/// point of the button.
#[tokio::test]
async fn rotating_the_webhook_token_revokes_the_previous_one() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // The seeded token works.
    app.post("/api/v1/webhook/inst-1/tok", serde_json::json!({ "eventType": "Test" }))
        .await
        .assert_status(StatusCode::OK);

    let rotated = app.post("/api/v1/instances/inst-1/webhook-token", serde_json::json!({})).await;
    let url = rotated.assert_ok()["webhook_url"].as_str().unwrap().to_string();
    let new_token = url.rsplit('/').next().unwrap().to_string();
    assert_ne!(new_token, "tok", "rotation must produce a different token");

    // The old one is dead...
    app.post("/api/v1/webhook/inst-1/tok", serde_json::json!({ "eventType": "Test" }))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    // ...and the new one is live.
    app.post(
        &format!("/api/v1/webhook/inst-1/{new_token}"),
        serde_json::json!({ "eventType": "Test" }),
    )
    .await
    .assert_status(StatusCode::OK);
}

/// The rotation response must never expose the Arr key, like every other
/// instance payload.
#[tokio::test]
async fn rotating_the_webhook_token_does_not_expose_the_arr_key() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://radarr:7878").await;

    let rotated = app.post("/api/v1/instances/inst-1/webhook-token", serde_json::json!({})).await;
    let body = rotated.assert_ok().to_string();

    assert!(!body.contains("arr-key"), "the Arr key leaked: {body}");
    assert!(!body.contains("enc:v1:"), "ciphertext leaked: {body}");
    assert!(body.contains("api_key_masked"), "the masked field is part of the contract: {body}");
}

/// Rotating an instance that does not exist is a 404, not a silent no-op that
/// leaves the caller thinking a token was replaced.
#[tokio::test]
async fn rotating_the_token_of_an_unknown_instance_is_a_not_found() {
    let app = TestApp::new().await;

    app.post("/api/v1/instances/nope/webhook-token", serde_json::json!({}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_test_webhook_is_acknowledged_without_touching_the_arr() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let response =
        app.post("/api/v1/webhook/inst-1/tok", serde_json::json!({ "eventType": "Test" })).await;
    assert_eq!(response.assert_ok()["message"], "Webhook reachable");
    assert!(arr.recorded().api_keys.is_empty(), "a test event read the Arr");
}

#[tokio::test]
async fn a_webhook_syncs_and_re_evaluates_only_the_media_it_names() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    // A second item that also needs a move, which the webhook must leave alone.
    seed_akira_needing_a_move(&app).await;

    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "Download", "movie": { "id": 10, "tmdbId": 8392 } }),
        )
        .await;
    let body = response.assert_ok();

    assert_eq!(body["media_id"], "m-inst-1-10", "the named item was synced");
    assert_eq!(body["moves_required"], 1);

    let touched: Vec<String> = sqlx::query_scalar("SELECT media_id FROM decisions")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(touched, vec!["m-inst-1-10"], "a webhook must not re-simulate the whole library");

    let reads = arr.recorded().reads.clone();
    assert!(reads.iter().any(|p| p == "/api/v3/movie/10"), "the item was read by id: {reads:?}");
    assert!(!reads.iter().any(|p| p == "/api/v3/movie"), "the library was listed: {reads:?}");
}

#[tokio::test]
async fn irrelevant_webhook_events_are_ignored() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response =
        app.post("/api/v1/webhook/inst-1/tok", serde_json::json!({ "eventType": "Grab" })).await;
    assert_eq!(response.assert_ok()["ignored"], "Grab");
}

#[tokio::test]
async fn webhooks_bypass_the_api_key_middleware() {
    // Radarr cannot send custom headers, so the token in the path is the only
    // credential, and this must keep working when ROUTARR_API_KEY is set.
    let app = TestApp::with_api_key("s3cret").await;
    app.seed_library().await;

    let response =
        app.post("/api/v1/webhook/inst-1/tok", serde_json::json!({ "eventType": "Test" })).await;
    response.assert_ok();
}

// ------------------------------------------------------------ jobs & logs

#[tokio::test]
async fn a_simulation_is_recorded_as_a_job() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    let jobs = app.get("/api/v1/jobs").await.assert_ok().clone();
    assert_eq!(jobs["pagination"]["total"], 1);
    assert_eq!(jobs["data"][0]["kind"], "simulate");
    assert_eq!(jobs["data"][0]["status"], "success");
}

#[tokio::test]
async fn an_unknown_job_is_a_404() {
    let app = TestApp::new().await;
    app.get("/api/v1/jobs/nope").await.assert_status(StatusCode::NOT_FOUND);
}

/// The list is what the Logs screen reads: a filter spelled wrong in
/// `build_filters` would show every row, or none, on the one screen an operator
/// opens because something failed.
#[tokio::test]
async fn logs_are_listed_newest_first_and_filtered_as_asked() {
    let app = TestApp::new().await;
    for (id, action, success, title, at) in [
        ("log-1", "move", 1, "Totoro", "2026-09-01 10:00:00"),
        ("log-2", "move", 0, "Akira", "2026-09-02 10:00:00"),
        ("log-3", "revert", 1, "Totoro", "2026-09-03 10:00:00"),
    ] {
        sqlx::query(
            "INSERT INTO execution_logs (id, action, details, success, media_title, executed_at)
             VALUES (?, ?, 'd', ?, ?, ?)",
        )
        .bind(id)
        .bind(action)
        .bind(success)
        .bind(title)
        .bind(at)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    let all = app.get("/api/v1/logs").await;
    let body = all.assert_ok();
    let ids: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["log-3", "log-2", "log-1"], "newest first");
    assert_eq!(body["pagination"]["total"], 3);

    let failed = app.get("/api/v1/logs?success=false").await;
    let failed = failed.assert_ok();
    let ids: Vec<&str> =
        failed["data"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["log-2"]);

    let reverts = app.get("/api/v1/logs?action=revert&search=toto").await;
    let reverts = reverts.assert_ok();
    let ids: Vec<&str> =
        reverts["data"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["log-3"]);

    // The LIKE wildcard is data, not a wildcard.
    let literal = app.get("/api/v1/logs?search=%25").await;
    assert_eq!(literal.assert_ok()["pagination"]["total"], 0);
}

#[tokio::test]
async fn logs_can_be_exported_as_csv() {
    let app = TestApp::new().await;
    sqlx::query(
        "INSERT INTO execution_logs (id, action, details, success, media_title)
         VALUES ('log-1', 'move', 'a,b \"quoted\"', 1, 'Totoro')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let response = app.raw("/api/v1/logs/export").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/csv; charset=utf-8");
    let csv = app.text("/api/v1/logs/export").await;
    // The comma and the quotes stay inside one field, the quotes doubled.
    assert!(csv.contains(r#","Totoro","a,b ""quoted""","#), "{csv}");
}

/// Every write says what set it off and, when a mode vouched for a name, who:
/// the log answers "did the nightly sweep do this, or did somebody" on
/// screen and in the file an operator hands on.
#[tokio::test]
async fn a_log_entry_names_its_trigger_and_its_account() {
    let app = TestApp::new().await;
    sqlx::query(
        "INSERT INTO execution_logs (id, action, details, success, media_title, actor, subject,
                                     executed_at)
         VALUES ('log-1', 'move', 'd', 1, 'Totoro', 'manual', 'alice', '2026-09-01 10:00:00'),
                ('log-2', 'move', 'd', 1, 'Akira', 'schedule', NULL, '2026-09-01 09:00:00')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let page = app.get("/api/v1/logs").await;
    let entries = page.assert_ok()["data"].as_array().unwrap().clone();
    assert_eq!(
        (&entries[0]["actor"], &entries[0]["subject"]),
        (&serde_json::json!("manual"), &serde_json::json!("alice"))
    );
    assert_eq!(
        (&entries[1]["actor"], &entries[1]["subject"]),
        (&serde_json::json!("schedule"), &serde_json::Value::Null)
    );

    let csv = app.text("/api/v1/logs/export").await;
    let mut lines = csv.lines();
    assert!(lines.next().unwrap().contains(",actor,subject,"), "{csv}");
    assert!(lines.next().unwrap().contains(r#","manual","alice","#), "{csv}");
    assert!(lines.next().unwrap().contains(r#","schedule","","#), "{csv}");
}

// ------------------------------------------------------------ status

#[tokio::test]
async fn status_makes_no_outbound_calls() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let status = app.get("/api/v1/status").await.assert_ok().clone();
    assert_eq!(status["dry_run"], true);
    assert_eq!(status["running_jobs"], 0);
    assert!(warning_messages(&status).iter().any(|w| w.contains("unauthenticated")));
    assert!(arr.recorded().api_keys.is_empty(), "/status asked the Arr");

    // The control: the probing route does reach this Arr, and it is recorded.
    app.get("/api/v1/health").await.assert_ok();
    assert!(!arr.recorded().api_keys.is_empty(), "the fixture records nothing");
}

#[tokio::test]
async fn status_counts_pending_decisions() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();

    let response = app.get("/api/v1/status").await;
    assert_eq!(response.assert_ok()["pending_decisions"], 1);
}

#[tokio::test]
async fn status_warns_when_a_category_has_no_root_folder() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.put("/api/v1/root-folders/rf-2/category", serde_json::json!({ "category": null }))
        .await
        .assert_ok();

    let warnings = warning_messages(app.get("/api/v1/status").await.assert_ok());
    assert!(warnings.iter().any(|w| w.contains("not mapped to any root folder")));
}

/// The badge counts `/status` and the page it links to renders `/health`. Two
/// hand-built lists drift in both directions, and the badge then reads
/// "1 warning" over a page listing three.
#[tokio::test]
async fn the_badge_never_claims_fewer_warnings_than_the_page_shows() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // A misconfiguration with several distinct findings: two categories left
    // unmapped, which also leaves the instance itself mapped to nothing.
    app.put("/api/v1/root-folders/rf-1/category", serde_json::json!({ "category": null }))
        .await
        .assert_ok();
    app.put("/api/v1/root-folders/rf-2/category", serde_json::json!({ "category": null }))
        .await
        .assert_ok();

    // The page first: its probe finds the fixture's Arr unreachable, one more
    // finding, and the badge polled after it has to count that one too.
    let health = app.get("/api/v1/health").await;
    let on_the_page = warning_messages(health.assert_ok());

    let status = app.get("/api/v1/status").await;
    let from_badge = warning_messages(status.assert_ok());

    assert!(on_the_page.len() > 1, "the fixture should produce several warnings");

    // Every warning the page shows is one the badge counts, word for word, and
    // the badge counts nothing the page leaves out.
    for warning in &on_the_page {
        assert!(
            from_badge.contains(warning),
            "the page shows a warning the badge does not count: {warning:?}"
        );
    }
    assert_eq!(from_badge.len(), on_the_page.len(), "{from_badge:?} against {on_the_page:?}");

    // And the instance with nothing mapped is among them: the finding a
    // hand-built badge list misses entirely.
    assert!(
        from_badge.iter().any(|w| w.contains("Radarr")),
        "the badge missed the unmapped instance: {from_badge:?}"
    );
}

/// An unreachable Arr or metadata source is a finding only a probe can make,
/// and `/status` may not probe: it is polled, and one dead host costs the full
/// connect timeout. A probe writes down what it saw and the endpoint that
/// cannot look reads it back, or the dashboard reports a source that has
/// stopped answering while the navigation beside it, unable to know, counts
/// zero.
#[tokio::test]
async fn the_badge_reports_what_the_last_probe_found() {
    let app = TestApp::new().await;
    // A port nothing is listening on: the probe fails the way an Arr that is
    // switched off does.
    app.seed_instance_at("i-dead", "radarr", "http://127.0.0.1:1").await;

    let before = warning_messages(app.get("/api/v1/status").await.assert_ok());
    // Matched on the finding itself: the fixture already warns about this
    // instance for an unrelated reason, so a name alone proves nothing.
    assert!(
        !before.iter().any(|w| w.contains("is unreachable")),
        "nothing has looked yet, so nothing may be claimed: {before:?}"
    );

    app.get("/api/v1/health").await.assert_ok();

    let after = warning_messages(app.get("/api/v1/status").await.assert_ok());
    assert!(
        after.iter().any(|w| w.contains("Fake radarr") && w.contains("is unreachable")),
        "the badge still ignores what the probe found: {after:?}"
    );
}

/// A look without a probe leaves the last probe's findings as they were: the
/// dashboard opens `/health?probe=false`, and erasing them there would hide an
/// Arr that is down.
#[tokio::test]
async fn a_look_without_a_probe_keeps_what_the_last_probe_found() {
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", "http://127.0.0.1:1").await;

    app.get("/api/v1/health").await.assert_ok();
    app.get("/api/v1/health?probe=false").await.assert_ok();

    let warnings = warning_messages(app.get("/api/v1/status").await.assert_ok());
    let down = warnings.iter().any(|w| w.contains("Fake radarr") && w.contains("is unreachable"));
    assert!(down, "a look without a probe erased the probe's finding: {warnings:?}");
}

/// A verdict is replaced, never accumulated: an instance that answers again has
/// to stop being reported, or the warning outlives the fault that caused it.
#[tokio::test]
async fn a_subject_that_answers_again_stops_being_reported() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", "http://127.0.0.1:1").await;
    let unreachable = || async {
        let warnings = warning_messages(app.get("/api/v1/status").await.assert_ok());
        warnings.iter().any(|w| w.contains("Fake radarr") && w.contains("is unreachable"))
    };

    app.get("/api/v1/health").await.assert_ok();
    assert!(unreachable().await, "the first probe recorded no failure to replace");

    // The Arr comes back, and the next probe finds it.
    sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'i-1'")
        .bind(&arr.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();
    app.get("/api/v1/health").await.assert_ok();
    assert!(!unreachable().await, "the verdict of the first probe outlived the second");
}

// ------------------------------------------------------------ health

/// The dashboard opens `/health?probe=false`, and the difference is not
/// cosmetic: probing an unreachable Arr costs the full connect timeout, and an
/// unreachable Arr is exactly why somebody opens the dashboard.
#[tokio::test]
async fn health_without_a_probe_answers_from_the_database_alone() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/health?probe=false").await;
    let health = response.assert_ok();

    // Everything a database can answer is still there.
    assert_eq!(health["database"], "connected");
    assert!(health["stats"]["total_media"].as_i64().unwrap() > 0);

    let instances = health["instances"].as_array().unwrap();
    assert!(!instances.is_empty(), "the instance rows are still reported");
    for instance in instances {
        // `unchecked`, not a guess. Reporting the last sync's outcome here
        // would read as a live connection state and be wrong the moment an Arr
        // goes down between two syncs.
        assert_eq!(instance["status"], "unchecked");
        assert!(instance["version"].is_null());
        // The counts come from the database, so they are real either way.
        assert!(instance["media_count"].as_i64().unwrap() >= 0);
    }
}

/// By default Diagnostics probes, and that is what it is for. A parameter that
/// silently became the default would turn the one screen that answers "is it
/// connected" into one that never asks.
#[tokio::test]
async fn health_probes_by_default() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/health").await;
    let health = response.assert_ok();

    for instance in health["instances"].as_array().unwrap() {
        assert_ne!(instance["status"], "unchecked", "the default must reach the Arr");
    }
}

#[tokio::test]
async fn health_reports_actionable_warnings() {
    let app = TestApp::new().await;
    // Listed while it had a key, gone since: the API refuses to add a source
    // with no key, and a key removed afterwards is how one stays listed.
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('metadata_providers', 'arr,tmdb')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let response = app.get("/api/v1/health").await;
    let health = response.assert_ok();

    assert_eq!(health["database"], "connected");

    // Listed, the Arr answers without a key and TMDb does not: the source list
    // says so rather than the page claiming metadata is simply unavailable.
    let providers = health["metadata"]["providers"].as_array().unwrap();
    assert_eq!(providers[0]["id"], "arr");
    assert_eq!(providers[0]["configured"], true);
    assert_eq!(providers[1]["id"], "tmdb");
    assert_eq!(providers[1]["configured"], false);

    let warnings = warning_messages(health);
    assert!(warnings.iter().any(|w| w.contains("TMDb")));
    assert!(warnings.iter().any(|w| w.contains("unauthenticated")));
    assert_eq!(health["status"], "degraded");
}

// ------------------------------------------------------------ helpers

async fn pending_ids(app: &TestApp) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM decisions WHERE status = 'pending'")
        .fetch_all(&app.state.pool)
        .await
        .unwrap()
}

async fn disable_dry_run(app: &TestApp) {
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "global_dry_run": "false" } }))
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_search_wildcard_is_matched_literally() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path, current_root_folder)
         VALUES ('m-pct', 'inst-1', 990, 'movie', '100% Wolf', '/movies/standard/100', '/movies/standard')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    // "100%" must find the film called "100% Wolf" and nothing else: an
    // unescaped LIKE would treat the percent as "anything after 100".
    let response = app.get("/api/v1/media?search=100%25").await;
    let items = response.assert_ok()["data"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "100% Wolf");

    // A percent that matches no literal percent must return nothing, even
    // though the wildcard interpretation would have matched every title.
    let response = app.get("/api/v1/media?search=o%25o").await;
    assert_eq!(
        response.assert_ok()["data"].as_array().unwrap().len(),
        0,
        "o%o is not a substring of any title"
    );
}

/// A retention of 0 keeps everything, as the setting's help says: a purge
/// reading it as a window deletes every log, task and proposal at once.
#[tokio::test]
async fn a_retention_of_zero_keeps_everything() {
    let app = aged_rows().await;
    app.store_setting("log_retention_days", "0").await;
    app.store_setting("decision_retention_days", "0").await;

    let response = app.post("/api/v1/maintenance/purge", serde_json::json!({})).await;

    let report = response.assert_ok();
    for removed in ["logs_removed", "jobs_removed", "decisions_removed"] {
        assert_eq!(report[removed], 0, "{removed}: {report}");
    }
}

/// What outlived its retention goes, what is younger stays, and the report the
/// Settings screen renders says how much went. Under the default windows, 90
/// days for logs and jobs and 30 for proposals, each table holds a row a day
/// past its window and a row a day inside it. A running job is never purged,
/// however old.
/// A row a day past each default window and a row a day inside it, and a
/// running job older than any.
async fn aged_rows() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    for statement in [
        "INSERT INTO execution_logs (id, action, success, executed_at)
         VALUES ('log-old', 'move', 1, datetime('now', '-91 days')),
                ('log-new', 'move', 1, datetime('now', '-89 days'))",
        "INSERT INTO jobs (id, kind, status, started_at)
         VALUES ('job-old', 'sync', 'success', datetime('now', '-91 days')),
                ('job-new', 'sync', 'success', datetime('now', '-89 days')),
                ('job-running', 'sync', 'running', datetime('now', '-91 days'))",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
         target_category, action, status, decided_at)
         VALUES ('d-old', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move', 'pending',
                 datetime('now', '-31 days')),
                ('d-new', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move', 'pending',
                 datetime('now', '-29 days'))",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }
    app
}

#[tokio::test]
async fn purging_removes_what_outlived_its_retention_and_reports_it() {
    let app = aged_rows().await;

    let response = app.post("/api/v1/maintenance/purge", serde_json::json!({})).await;
    let report = response.assert_ok();

    assert_eq!(report["logs_removed"], 1, "{report}");
    assert_eq!(report["jobs_removed"], 1, "{report}");
    assert_eq!(report["decisions_removed"], 1, "{report}");
    for (table, kept) in [
        ("execution_logs", vec!["log-new"]),
        ("jobs", vec!["job-new", "job-running"]),
        ("decisions", vec!["d-new"]),
    ] {
        let left: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT id FROM {table} WHERE id LIKE '%-old' OR id LIKE '%-new' OR id LIKE '%-running'
             ORDER BY id"
        )))
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(left, kept, "{table}");
    }
}

/// The catalogue drives the rule builder, so a condition offered there is a
/// condition a user can put on a rule. Radarr's payload carries no `tvdbId`,
/// no `seriesType` and no season list, so `upsert_media` leaves those columns
/// null for every film, and offering them on a movie rule is offering something
/// that can only ever fail.
#[tokio::test]
async fn the_condition_catalogue_says_which_media_types_each_condition_applies_to() {
    let app = TestApp::new().await;

    let response = app.get("/api/v1/rules/conditions").await;
    let body = response.assert_ok();
    let conditions = body["conditions"].as_array().expect("conditions");

    let scope = |kind: &str| -> Vec<String> {
        conditions
            .iter()
            .find(|c| c["type"] == kind)
            .unwrap_or_else(|| panic!("{kind} missing from the catalogue"))["media_types"]
            .as_array()
            .expect("media_types")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };

    for kind in ["series_type_is", "season_count_over", "tvdb_id_in"] {
        assert_eq!(scope(kind), vec!["series"], "{kind} cannot match on a film");
    }
    for kind in ["genre_contains", "original_language", "certification_in", "tmdb_id_in"] {
        assert_eq!(scope(kind), vec!["movie", "series"], "{kind} applies to both");
    }

    // Every entry must say something: a missing scope would read as "no media
    // type" in the builder and hide the condition from every rule.
    assert!(
        conditions.iter().all(|c| c["media_types"].as_array().is_some_and(|a| !a.is_empty())),
        "a condition carries no media type at all"
    );
}

// ------------------------------------------------ settings bounds, retroactive

/// The Settings screen sends every field, so one value stored before its bound
/// existed travels with whatever the operator changed and the whole save is
/// refused (nothing written, the field they changed included) until a start
/// converges the stale value. What each key converges to is the matrix test's
/// claim, below, and this one is the screen's.
#[tokio::test]
async fn a_save_the_screen_sends_is_refused_until_a_start_converges_the_stale_value() {
    let app = TestApp::new().await;

    // What an installation running the previous version could hold.
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('backup_interval_hours', '5000'),
                                                  ('scheduler_interval_minutes', '2880')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": {
            "backup_interval_hours": "5000", "scheduler_interval_minutes": "2880",
            "batch_limit": "25"
        }}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    let batch: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'batch_limit'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert_ne!(batch.as_deref(), Some("25"), "a refused save wrote a field");

    crate::services::maintenance::converge_setting_bounds(&app.state).await.unwrap();

    // And the screen can save again, sending every field as it always does.
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": {
            "backup_interval_hours": "168", "scheduler_interval_minutes": "1440",
            "batch_limit": "25"
        }}),
    )
    .await
    .assert_ok();
}

/// Every ranged key is raised to its floor at a start, and every `Bounded` key
/// is lowered to its ceiling. No retention count is: lowering one removes what
/// is beyond it, which is the operator's own act. Read off the table, so
/// a key added to it is covered without being named here.
#[tokio::test]
async fn every_ranged_key_is_raised_and_only_a_bounded_one_is_lowered() {
    let app = TestApp::new().await;
    let ranged = crate::services::settings::ranged_keys();
    assert!(
        ranged.iter().any(|r| r.3) && ranged.iter().any(|r| !r.3),
        "the table is what this reads, and it holds both kinds"
    );

    async fn store(pool: &sqlx::SqlitePool, key: &str, value: i64) {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value.to_string())
        .execute(pool)
        .await
        .unwrap();
    }
    async fn stored(pool: &sqlx::SqlitePool, key: &str) -> String {
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    for above in [true, false] {
        let side = if above { "above" } else { "below" };
        for (key, min, max, _) in &ranged {
            store(&app.state.pool, key, if above { max + 1 } else { min - 1 }).await;
        }

        let converged =
            crate::services::maintenance::converge_setting_bounds(&app.state).await.unwrap();
        let expected = if above { ranged.iter().filter(|r| r.3).count() } else { ranged.len() };
        assert_eq!(converged, expected, "{side}: the count of keys converged");

        for (key, min, max, lowered) in &ranged {
            let expected = match (above, lowered) {
                (true, true) => *max,
                (true, false) => max + 1,
                (false, _) => *min,
            };
            assert_eq!(stored(&app.state.pool, key).await, expected.to_string(), "{side}: {key}");
        }
    }
}

/// A value that is not a number is left alone: guessing one would be inventing
/// a setting nobody chose, and `check` refuses it with its own message.
#[tokio::test]
async fn a_setting_that_is_not_a_number_is_left_for_validation_to_refuse() {
    let app = TestApp::new().await;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('batch_limit', 'many')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    assert_eq!(crate::services::maintenance::converge_setting_bounds(&app.state).await.unwrap(), 0);
    let still: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'batch_limit'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(still, "many");
}

/// Akira at `/movies/standard` with the rule, the mapping and the metadata
/// that route it to `/movies/anime`: one item a simulation has a move for. Its
/// row is named the way the sync names rows, so a delivery can find it.
async fn seed_akira_needing_a_move(app: &TestApp) {
    app.seed_route_to_anime().await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id,
         current_root_folder, monitored, has_files)
         VALUES ('m-inst-1-99', 'inst-1', 99, 'movie', 'Akira', 8392, '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
}

/// `sync_single_media` answers `None` when the Arr no longer has the item,
/// which is exactly what a delete event reports. With no media filter the
/// evaluation is the whole instance, persisted and automatically applied,
/// outside the lock that exists to bound precisely that.
#[tokio::test]
async fn a_delete_event_does_not_evaluate_the_whole_instance() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    seed_akira_needing_a_move(&app).await;

    let decisions = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM decisions")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    };
    let before = decisions().await;

    // An id the fake Arr does not serve: the delete has already happened.
    app.post(
        "/api/v1/webhook/inst-1/tok",
        serde_json::json!({ "eventType": "MovieDelete", "movie": { "id": 999_999 } }),
    )
    .await
    .assert_ok();
    assert_eq!(decisions().await, before, "a delete evaluated and persisted the whole instance");

    // The fixture can write, or the equality above proves nothing: a whole
    // pass has a move to record for Akira.
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    assert!(decisions().await > before, "the fixture never had a decision to write");
}

/// `MovieDelete` reports that the item is gone. Acknowledged and left there,
/// the row survives with its override and its pending proposal, the proposal
/// listed for ever and never applicable, since the executor joins `media`.
#[tokio::test]
async fn a_delete_event_retires_the_item_and_what_pointed_at_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    seed_akira_needing_a_move(&app).await;
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-1', 'm-inst-1-99', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.post("/api/v1/simulate", serde_json::json!({})).await.assert_ok();
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions
          WHERE media_id = 'm-inst-1-99' AND status = 'pending' AND superseded = 0",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "the fixture must have a proposal to retire");

    let body = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "MovieDelete", "movie": { "id": 99 } }),
        )
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["retired"], 1, "{body}");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-99'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "the row the Arr no longer has stayed");
    let overrides: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM overrides WHERE media_id = 'm-inst-1-99'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(overrides, 0, "the override did not go with its row");
    let (status, superseded): (String, bool) =
        sqlx::query_as("SELECT status, superseded FROM decisions WHERE media_id = 'm-inst-1-99'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), superseded), ("pending", true), "the proposal still stands");
}

/// A synchronisation that read the Arr before the deletion would put the row
/// back after this retired it, stamped current, and keep it until the next
/// full pass. Retiring waits for a running sync to end, so it runs after the
/// sync's commit whatever the order of the reads.
#[tokio::test]
async fn a_delete_event_waits_for_a_running_sync_before_retiring() {
    use std::time::Duration;

    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    seed_akira_needing_a_move(&app).await;

    let syncing = app.state.jobs.try_lock("sync:inst-1").expect("the key is free");

    // Cannot complete while the sync holds its key, so the timeout cannot be
    // flaky: a delivery that retired without waiting answers at once and
    // fails this.
    let mut delivering = Box::pin(app.post(
        "/api/v1/webhook/inst-1/tok",
        serde_json::json!({ "eventType": "MovieDelete", "movie": { "id": 99 } }),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut delivering).await.is_err(),
        "the delivery retired the row while a sync was running"
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-99'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "retired under a running sync");

    drop(syncing);
    let body = delivering.await.assert_ok().clone();
    assert_eq!(body["retired"], 1, "{body}");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-99'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "not retired once the sync was done");
    assert!(app.state.jobs.try_lock("sync:inst-1").is_some(), "the sync key was left held");
}

/// A 404 on any other event is not news of a deletion: a base URL pointing at
/// something that is not an Arr answers 404 to everything, and the full sync,
/// which reads the whole list and refuses to act on an empty one, is the safer
/// judge of what is gone. Acknowledged rather than refused, since Radarr would
/// retry a rejection it can do nothing about. And nothing is created for an id
/// nobody has.
#[tokio::test]
async fn an_item_the_arr_does_not_know_is_kept_unless_the_event_says_it_is_gone() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    seed_akira_needing_a_move(&app).await;

    let body = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "MovieAdded", "movie": { "id": 99 } }),
        )
        .await
        .assert_ok()
        .clone();
    assert!(body["media_id"].is_null(), "{body}");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-99'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "a 404 on an ordinary event retired the row");
    let all: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(all, 1, "a row was created for an item the Arr does not have");
}

/// `instance_ids: []` on the API is the whole library: what the interface's
/// own request, which sends no list at all, and a rule scoped to no instance
/// both mean by it.
#[tokio::test]
async fn an_empty_instance_list_on_the_api_is_the_whole_library() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = app
        .post("/api/v1/simulate", serde_json::json!({ "instance_ids": [] }))
        .await
        .assert_ok()
        .clone();
    assert_eq!(body["total_media"], 1, "{body}");

    let preview = app
        .post(
            "/api/v1/rules/preview",
            serde_json::json!({ "rule": anime_rule_body(), "instance_ids": [] }),
        )
        .await
        .assert_ok()
        .clone();
    assert_eq!(preview["changed_total"], 1, "{preview}");
}

/// SQLite binds at most 32 766 parameters to one statement. The batch limit,
/// which never exceeds 1 000, is checked before the capacity guard's
/// `IN (...)`, so a longer list from the request body is refused with a
/// sentence rather than failing there as a 500.
#[tokio::test]
async fn an_apply_naming_more_ids_than_one_statement_binds_is_refused_not_failed() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let ids: Vec<String> = (0..33_000).map(|n| format!("d-{n}")).collect();
    let response = app
        .post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": ids, "move_files": false, "confirm": [] }),
        )
        .await;
    assert_eq!(response.status, 400, "{}", response.json);
    assert!(response.message().contains("above the batch limit"), "{}", response.json);
}

/// The same list on a simulation's instance filter: refused as a request no
/// library can satisfy rather than failed inside the query.
#[tokio::test]
async fn a_simulation_scoped_to_more_instances_than_one_statement_binds_is_refused_not_failed() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let ids: Vec<String> = (0..33_000).map(|n| format!("inst-{n}")).collect();
    let response = app.post("/api/v1/simulate", serde_json::json!({ "instance_ids": ids })).await;
    assert_eq!(response.status, 400, "{}", response.json);
}

/// Every whole-library pass (a preview, a health report, a sweep) holds one of
/// two permits while it runs, and the third waits for one to end rather than
/// being refused. A wait is what a read can afford, and a 409 on the rules
/// page, which renders its health on every load, is not.
#[tokio::test]
async fn a_third_library_pass_waits_for_one_of_the_two_to_end() {
    use std::time::Duration;

    let app = TestApp::new().await;
    app.seed_library().await;

    let first = crate::services::routing::library_pass().await;
    let second = crate::services::routing::library_pass().await;

    // Neither can complete while both permits are held, so the elapsed
    // timeouts below cannot be flaky in either direction: completion is
    // impossible, and a response of any kind, a refusal included, would end
    // the wait early and fail the check.
    let mut preview =
        Box::pin(app.post("/api/v1/simulate", serde_json::json!({ "persist": false })));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut preview).await.is_err(),
        "the third pass did not wait"
    );
    let mut health = Box::pin(app.get("/api/v1/rules/health"));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut health).await.is_err(),
        "a health report did not wait either"
    );

    drop(first);
    preview.await.assert_ok();
    drop(second);
    health.await.assert_ok();

    // With the permits back, two previews run side by side and nothing
    // answers 409: the bound is a wait, not a refusal.
    let (one, two) = tokio::join!(
        app.post("/api/v1/rules/preview", serde_json::json!({ "rule": anime_rule_body() })),
        app.post("/api/v1/simulate", serde_json::json!({ "persist": false })),
    );
    one.assert_ok();
    two.assert_ok();
}

/// `persist: false` is documented as a pure read. It supersedes nothing, so
/// refusing it because something else is writing refuses a read for a conflict
/// it cannot have.
#[tokio::test]
async fn a_simulation_that_writes_nothing_is_not_refused_by_a_running_one() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let running = app.state.jobs.try_lock(crate::jobs::FULL_SIMULATION).expect("free");

    app.post("/api/v1/simulate", serde_json::json!({ "persist": false })).await.assert_ok();
    app.post("/api/v1/simulate", serde_json::json!({ "persist": true }))
        .await
        .assert_status(StatusCode::CONFLICT);

    drop(running);
}
