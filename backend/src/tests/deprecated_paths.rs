//! The paths the contract renamed, answered under their old names as deprecated aliases: the
//! same handlers, the same scopes, the same answers, said deprecated on each one.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;

use super::{TestApp, TestResponse};
use crate::api::deprecated::DEPRECATED_PATHS;

/// The handlers a `.route(...)` of `main.rs` mounts at `path`, as written, comments left out.
fn handlers_at(block: &str, path: &str) -> Option<String> {
    let code: String = block.lines().map(|line| line.split("//").next().unwrap_or("")).collect();
    let code: String = code.split_whitespace().collect();
    let handlers =
        code.split(".route(").find_map(|route| route.strip_prefix(&format!("\"{path}\",")))?;
    let mut depth = 0;
    let end = handlers.find(|c| {
        depth += match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        depth < 0
    })?;
    Some(handlers[..end].to_owned())
}

/// Each old path is mounted with exactly its new path's handlers, so the two cannot drift.
#[test]
fn every_old_path_is_mounted_with_its_new_path_handlers() {
    let block = super::security::route_block();
    for (old, new) in DEPRECATED_PATHS {
        let (old_handlers, new_handlers) = (handlers_at(block, old), handlers_at(block, new));
        assert!(new_handlers.is_some(), "{new} is not mounted");
        assert_eq!(old_handlers, new_handlers, "{old} answers otherwise than {new}");
    }
}

/// Each old path is documented as a copy of its replacement: marked deprecated, under an
/// operation id of its own, with the same scope, parameters, body and answers.
#[test]
fn every_old_path_is_documented_as_a_deprecated_copy_of_its_new_one() {
    let document = serde_json::to_value(crate::api::contract::document("")).unwrap();
    let paths = &document["paths"];
    for (old, new) in DEPRECATED_PATHS {
        let (old_item, new_item) = (&paths[old], &paths[new]);
        // An owner's route is documented under neither name.
        if !new_item.is_object() {
            assert!(!old_item.is_object(), "{old} is documented and {new} is not");
            continue;
        }
        for (method, operation) in new_item.as_object().unwrap() {
            let copy = &old_item[method];
            assert_eq!(copy["deprecated"], true, "{method} {old}");
            assert_ne!(operation["deprecated"], true, "{method} {new} is marked deprecated");
            let id = operation["operationId"].as_str().unwrap();
            assert_eq!(copy["operationId"], format!("{id}_deprecated"), "{method} {old}");
            for field in ["x-routarr-scope", "parameters", "requestBody", "responses", "tags"] {
                assert_eq!(copy[field], operation[field], "{method} {old}: {field}");
            }
        }
    }
}

/// Every old path read answers what its new path does, said deprecated, and the new one is not.
#[tokio::test]
async fn an_old_path_answers_as_its_new_one_and_says_it_is_deprecated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    for (old, new) in [
        ("/overrides", "/exceptions"),
        ("/jobs", "/tasks"),
        ("/logs", "/move-log"),
        ("/media", "/library"),
        ("/media/m-1", "/library/m-1"),
        ("/media/facets", "/library/facets"),
        ("/metadata/providers", "/sources"),
    ] {
        let read = |path: &str| {
            let request = Request::get(format!("/api/v1{path}")).body(Body::empty()).unwrap();
            app.send(request)
        };
        let (was, now) = (read(old).await, read(new).await);
        assert_eq!(now.status, StatusCode::OK, "{new}: {}", now.json);
        assert_eq!((was.status, &was.json), (now.status, &now.json), "{old} answered otherwise");
        assert_eq!(was.header("deprecation").as_deref(), Some("@1792540800"), "{old}");
        let link = was.header(header::LINK.as_str()).unwrap_or_default();
        assert!(link.contains("rel=\"deprecation\""), "{old}: {link}");
        assert_eq!(now.header("deprecation"), None, "{new} is said deprecated");
    }
}

/// A script that created through an old path follows a `Location` under the old names, and one
/// on the new path one under the new names. A task started is located under `/tasks`, its id
/// under both names.
#[tokio::test]
async fn a_location_follows_the_names_the_request_used() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let create = |path: &'static str| {
        app.post(path, json!({ "media_id": "m-1", "target_category": "anime" }))
    };

    let old = create("/api/v1/overrides").await;
    let location = old.header(header::LOCATION.as_str()).unwrap_or_default();
    assert!(location.starts_with("/api/v1/overrides/"), "{location}");
    app.delete(&location).await.assert_ok();
    let new = create("/api/v1/exceptions").await;
    let location = new.header(header::LOCATION.as_str()).unwrap_or_default();
    assert!(location.starts_with("/api/v1/exceptions/"), "{location}");

    let started = app
        .send(
            Request::post("/api/v1/simulate")
                .header("prefer", "respond-async")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.json);
    let location = started.header(header::LOCATION.as_str()).unwrap_or_default();
    let task = started.json["task_id"].as_str().unwrap_or_default();
    assert_eq!(location, format!("/api/v1/tasks/{task}"));
    assert_eq!(started.json["job_id"], task, "{}", started.json);
}

const OWNER: &str = "owner-key-of-the-aliases";

/// `method path` sent with `key`, a JSON body for any method but GET.
async fn as_key(app: &TestApp, method: &str, path: &str, key: &str) -> TestResponse {
    app.state.key_rates.refill();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", key)
        .header("content-type", "application/json");
    let body = if method == "GET" { Body::empty() } else { Body::from("{}") };
    app.send(request.body(body).unwrap()).await
}

/// A new application key named `name`, holding `scopes` beside read.
async fn mint(app: &TestApp, name: &str, scopes: serde_json::Value) -> String {
    let request = Request::post("/api/v1/applications")
        .header("x-api-key", OWNER)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "name": name, "scopes": scopes }).to_string()))
        .unwrap();
    app.send(request).await.assert_ok()["token"].as_str().unwrap().to_string()
}

/// An application key is let through an old path exactly where it is let through the new one,
/// whatever the method: a read key and a write key, each refused somewhere and admitted
/// somewhere, so the comparison is not of two refusals.
#[tokio::test]
async fn an_application_key_reaches_an_old_path_as_it_reaches_its_new_one() {
    let app = TestApp::with_api_key(OWNER).await;
    let mut refused_somewhere = 0;
    let mut admitted_somewhere = 0;
    for (name, scopes) in [("dashboard", json!([])), ("bot", json!(["write"]))] {
        let token = mint(&app, name, scopes.clone()).await;
        for (old, new) in DEPRECATED_PATHS {
            for method in ["GET", "POST", "PUT", "DELETE"] {
                let refused = |path: &str| {
                    let path = format!("/api/v1{}", path.replace("{id}", "x"));
                    let token = &token;
                    let app = &app;
                    async move {
                        as_key(app, method, &path, token).await.status == StatusCode::FORBIDDEN
                    }
                };
                let (was, now) = (refused(old).await, refused(new).await);
                assert_eq!(was, now, "{method} {old} and {method} {new}, a key holding {scopes}");
                if now {
                    refused_somewhere += 1;
                } else {
                    admitted_somewhere += 1;
                }
            }
        }
    }
    assert!(refused_somewhere > 0 && admitted_somewhere > 0, "the walk proves nothing");
}

/// The `deprecated_api_called` warnings `/status` holds.
async fn deprecation_warnings(app: &TestApp) -> Vec<serde_json::Value> {
    let status = as_key(app, "GET", "/api/v1/status", OWNER).await;
    let warnings = status.assert_ok()["warnings"].as_array().unwrap().clone();
    warnings.into_iter().filter(|w| w["code"] == "deprecated_api_called").collect()
}

/// A key calling an old path is named in a warning beside the path, and the same call on the
/// new path warns of nothing.
#[tokio::test]
async fn a_key_calling_an_old_path_is_named_in_a_warning() {
    let app = TestApp::with_api_key(OWNER).await;
    let token = mint(&app, "homepage", json!([])).await;

    as_key(&app, "GET", "/api/v1/exceptions", &token).await.assert_ok();
    assert_eq!(deprecation_warnings(&app).await, Vec::<serde_json::Value>::new());

    as_key(&app, "GET", "/api/v1/overrides", &token).await.assert_ok();
    as_key(&app, "GET", "/api/v1/jobs", &token).await.assert_ok();
    let warnings = deprecation_warnings(&app).await;
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["severity"], "info");
    let message = warnings[0]["message"].as_str().unwrap();
    for named in ["homepage", "GET /api/v1/jobs", "GET /api/v1/overrides"] {
        assert!(message.contains(named), "{named} missing from: {message}");
    }
}

/// The warning reads only what a run stored, so it stands after a restart: a key's call of two
/// days ago warns, one older than 30 days no longer does, and a call without an application key
/// is said apart.
#[tokio::test]
async fn the_warning_reads_the_calls_a_previous_run_stored() {
    let app = TestApp::with_api_key(OWNER).await;
    app.execute(&["INSERT INTO deprecated_calls
                (caller_kind, caller_id, caller_name, method, path, calls, last_called_at)
         VALUES ('application', 'k-1', 'nightly', 'GET', '/media', 4, datetime('now', '-2 days')),
                ('application', 'k-2', 'retired', 'GET', '/logs', 9, datetime('now', '-31 days')),
                ('person', '', 'apikey', 'POST', '/overrides', 1, datetime('now', '-1 day'))"])
        .await;

    let messages: Vec<String> = deprecation_warnings(&app)
        .await
        .iter()
        .map(|warning| warning["message"].as_str().unwrap().to_owned())
        .collect();
    let two_days_ago = (chrono::Utc::now() - chrono::Duration::days(2)).format("%Y-%m-%d");
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].contains("nightly"), "{messages:?}");
    assert!(messages[0].contains(&format!("GET /api/v1/media. Last call on {two_days_ago}")));
    assert!(messages[1].starts_with("Routarr's own API key or a session"), "{messages:?}");
    assert!(messages[1].contains("POST /api/v1/overrides"), "{messages:?}");
}

/// What `request` logged.
async fn logged(app: &TestApp, method: &str, path: &str, key: &str) -> String {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;
    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    as_key(app, method, path, key).with_subscriber(tracing::Dispatch::new(subscriber)).await;
    capture.contents()
}

/// A call to an old path is logged with its caller and the path to call instead, once a day per
/// key and route, so a script polling every minute writes one line a day.
#[tokio::test]
async fn a_call_to_an_old_path_is_logged_once_a_day_per_key_and_route() {
    const LINE: &str = "A deprecated API path was called";
    let app = TestApp::with_api_key(OWNER).await;
    let token = mint(&app, "homepage", json!([])).await;

    let first = logged(&app, "GET", "/api/v1/media", &token).await;
    assert!(first.contains(LINE), "{first}");
    for field in ["caller=\"homepage\"", "path=\"/media\"", "replacement=\"/library\""] {
        assert!(first.contains(field), "{field} missing from: {first}");
    }
    assert!(!logged(&app, "GET", "/api/v1/media", &token).await.contains(LINE));
    assert!(logged(&app, "GET", "/api/v1/logs", &token).await.contains(LINE), "another route");

    app.execute(&["UPDATE deprecated_calls SET logged_on = date('now', '-1 day')"]).await;
    assert!(logged(&app, "GET", "/api/v1/media", &token).await.contains(LINE), "the next day");
}

/// The counter adds every caller's calls of each old path, and a new path adds none.
#[tokio::test]
async fn the_counter_adds_the_calls_of_each_old_path() {
    let app = TestApp::with_api_key(OWNER).await;
    let (one, other) = (mint(&app, "one", json!([])).await, mint(&app, "other", json!([])).await);
    for (path, key) in [
        ("/api/v1/media", &one),
        ("/api/v1/media", &other),
        ("/api/v1/jobs", &one),
        ("/api/v1/library", &one),
    ] {
        as_key(&app, "GET", path, key).await.assert_ok();
    }

    let response = app
        .send_raw(
            Request::get("/api/v1/metrics").header("x-api-key", OWNER).body(Body::empty()).unwrap(),
        )
        .await;
    let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    let samples: Vec<&str> = body
        .lines()
        .filter(|line| line.starts_with("routarr_deprecated_api_requests_total{"))
        .collect();
    assert_eq!(
        samples,
        [
            "routarr_deprecated_api_requests_total{method=\"GET\",path=\"/jobs\"} 1",
            "routarr_deprecated_api_requests_total{method=\"GET\",path=\"/media\"} 2",
        ]
    );
}
