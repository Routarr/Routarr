//! The contract an outside application relies on: the document the code
//! describes, the file that pins it, and the answers the API really gives.

use std::collections::BTreeSet;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::fake_arr::FakeArr;
use super::security::{declared_routes, route_template};
use super::{TestApp, TestResponse, finished, preferring_async};
use crate::api::applications::GRANTS;
use crate::api::contract::{self, SCOPE_EXTENSION, for_each_operation};
use crate::config::{Config, normalise_base_path};
use crate::state::AppState;

/// Where the pinned document lives, and how to write it again.
const PINNED: &str = "openapi/v1.json";
const WRITE: &str = "ROUTARR_WRITE_CONTRACT";

fn described() -> Value {
    serde_json::to_value(contract::document("")).expect("the document serializes")
}

/// Every operation the document describes: method, path, and the scope it
/// states.
fn operations() -> Vec<(Method, String, Option<String>)> {
    let mut document = contract::document("");
    let mut found = Vec::new();
    for_each_operation(&mut document, |method, path, operation| {
        let scope = operation
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get(SCOPE_EXTENSION))
            .and_then(Value::as_str)
            .map(str::to_string);
        found.push((method.clone(), path.to_string(), scope));
    });
    found
}

/// The pinned file is what the code describes. A change to a type a handler
/// returns changes the contract, and it has to be seen in review as a change
/// to this file, where CI compares it with the last release's.
#[test]
fn the_pinned_contract_is_the_one_the_code_describes() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PINNED);
    let written = serde_json::to_string_pretty(&described()).unwrap() + "\n";
    if std::env::var_os(WRITE).is_some() {
        std::fs::write(&path, &written).expect("the pinned contract is writable");
    }
    let pinned = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        pinned == written,
        "{PINNED} no longer matches the code. Review the difference, then pin it with \
         `{WRITE}=1 cargo test contract`."
    );
}

/// The document names every route an application key may call, each with the
/// scope the middleware asks, and no route the owner keeps. `/ping` is the
/// one operation that needs no key.
#[test]
fn every_granted_route_is_documented_with_its_scope_and_nothing_else_is() {
    let granted: BTreeSet<(String, String, String)> = GRANTS
        .iter()
        .map(|(method, route, scope)| {
            (method.to_string(), route.to_string(), scope.as_str().to_string())
        })
        .collect();
    let mut documented = BTreeSet::new();
    let mut open = Vec::new();
    for (method, path, scope) in operations() {
        match scope {
            Some(scope) => {
                documented.insert((method.to_string(), path, scope));
            }
            None => open.push(format!("{method} {path}")),
        }
    }
    assert_eq!(documented, granted);
    assert_eq!(open, ["GET /ping"], "only the liveness probe is documented without a scope");
    assert_eq!(described()["paths"]["/ping"]["get"]["security"], json!([]));
}

#[test]
fn every_documented_operation_is_a_declared_route() {
    let declared: BTreeSet<(String, String)> = declared_routes()
        .into_iter()
        .map(|(method, path)| (method.to_string(), route_template(&path)))
        .collect();
    for (method, path, _) in operations() {
        assert!(
            declared.contains(&(method.to_string(), path.clone())),
            "{method} {path} is documented but main.rs does not declare it"
        );
    }
}

/// A client reads the contract before it holds a key, and behind a sub-path
/// the server it names is the one it answers under.
#[tokio::test]
async fn the_contract_is_served_without_a_key_under_its_mount_point() {
    let app = TestApp::with_api_key("owner-key").await;
    let served = app.get("/api/v1/openapi.json").await;
    assert_eq!(served.assert_ok()["servers"], json!([{ "url": "/api/v1" }]));
    assert_eq!(served.json["paths"], described()["paths"]);

    let mut config = Config::for_tests();
    config.base_path = normalise_base_path("/routarr");
    let mounted = TestApp::around(AppState::for_tests().await.with_config(config));
    let served = mounted.get("/routarr/api/v1/openapi.json").await;
    assert_eq!(served.assert_ok()["servers"], json!([{ "url": "/routarr/api/v1" }]));
}

/// Holds each answer to the status the call expects and to the schema the
/// contract states for it, and remembers which successes it saw: an error
/// validated against the shared envelope does not stand for a success.
struct Checker {
    document: Value,
    seen: BTreeSet<(String, String, String)>,
}

impl Checker {
    fn new() -> Self {
        Self { document: described(), seen: BTreeSet::new() }
    }

    fn check(&mut self, method: &str, route: &str, expected: u16, response: &TestResponse) {
        let status = response.status.as_str().to_string();
        assert_eq!(
            response.status.as_u16(),
            expected,
            "{method} {route} answered {status}: {}",
            response.json
        );
        let responses = &self.document["paths"][route][method.to_lowercase()]["responses"];
        assert!(responses.is_object(), "{method} {route} is not documented");
        let documented = if responses[&status].is_object() { &status } else { "default" };
        let schema = &responses[documented]["content"]["application/json"]["schema"];
        assert!(schema.is_object(), "{method} {route} documents no JSON body for {status}");

        let mut root = schema.clone();
        root["components"] = self.document["components"].clone();
        let validator = jsonschema::validator_for(&root).expect("the schema compiles");
        let errors: Vec<String> = validator
            .iter_errors(&response.json)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{method} {route} answered {status} outside its schema:\n{}\n{}",
            errors.join("\n"),
            response.json
        );
        if response.status.is_success() {
            self.seen.insert((method.to_string(), route.to_string(), status));
        }
    }

    /// Every success status the contract documents, by operation.
    fn documented_successes(&self) -> BTreeSet<(String, String, String)> {
        let mut successes = BTreeSet::new();
        for (path, item) in self.document["paths"].as_object().unwrap() {
            for (method, operation) in item.as_object().unwrap() {
                let Some(responses) = operation["responses"].as_object() else { continue };
                for status in responses.keys().filter(|status| status.starts_with('2')) {
                    successes.insert((method.to_uppercase(), path.clone(), status.clone()));
                }
            }
        }
        successes
    }
}

/// Every operation the contract documents is called once, and what it
/// answers matches the schema stated for it, names and types both. A handler
/// returning another type than its documentation names fails here.
#[tokio::test]
async fn every_documented_operation_answers_as_its_schema_says() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 3).await;
    let mut checker = Checker::new();
    let every_guardrail = json!(["capacity", "threshold", "batch", "unreachable"]);

    for (route, path) in [
        ("/ping", "/api/v1/ping"),
        ("/auth/me", "/api/v1/auth/me"),
        ("/status", "/api/v1/status"),
        ("/health", "/api/v1/health?probe=false"),
        ("/categories", "/api/v1/categories"),
        ("/media", "/api/v1/media?per_page=2"),
        ("/media/{id}", "/api/v1/media/m-0"),
        ("/media/{id}/explain", "/api/v1/media/m-0/explain"),
        ("/route", "/api/v1/route?type=movie&tmdb=8392&instance=inst-1"),
    ] {
        checker.check("GET", route, 200, &app.get(path).await);
    }
    // A refusal answers the shared envelope.
    let missing = app.get("/api/v1/media/nothing-here").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    checker.check("GET", "/media/{id}", 404, &missing);

    let metrics = app.raw("/api/v1/metrics").await;
    assert_eq!(metrics.status(), StatusCode::OK);
    let content_type = metrics.headers()[axum::http::header::CONTENT_TYPE].to_str().unwrap();
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    checker.seen.insert(("GET".into(), "/metrics".into(), "200".into()));

    let simulated = app.post("/api/v1/simulate", json!({ "persist": true })).await;
    checker.check("POST", "/simulate", 200, &simulated);
    let simulation = simulated.json["simulation_id"].as_str().unwrap().to_string();
    let pending = app.get("/api/v1/decisions?status=pending").await;
    checker.check("GET", "/decisions", 200, &pending);
    let first = pending.json["data"][0]["id"].as_str().unwrap().to_string();

    let body = json!({ "decision_ids": [first], "confirm": every_guardrail });
    checker.check(
        "POST",
        "/decisions/apply",
        200,
        &app.post("/api/v1/decisions/apply", body).await,
    );
    let asked = app.post("/api/v1/decisions/apply-all", json!({ "simulation_id": simulation }));
    let asked = asked.await;
    assert_eq!(asked.status, StatusCode::CONFLICT);
    checker.check("POST", "/decisions/apply-all", 409, &asked);
    let body = json!({ "simulation_id": simulation, "confirm": every_guardrail });
    let applied = app.post("/api/v1/decisions/apply-all", body).await;
    checker.check("POST", "/decisions/apply-all", 200, &applied);
    let body = json!({ "decision_ids": [first], "confirm": every_guardrail });
    checker.check(
        "POST",
        "/decisions/revert",
        200,
        &app.post("/api/v1/decisions/revert", body).await,
    );

    let pin = json!({ "media_id": "m-1", "target_category": "anime", "reason": "a test" });
    let set = app.post("/api/v1/overrides", pin).await;
    checker.check("POST", "/overrides", 200, &set);
    checker.check("GET", "/overrides", 200, &app.get("/api/v1/overrides").await);
    let exception = set.json["id"].as_str().unwrap();
    let removed = app.delete(&format!("/api/v1/overrides/{exception}")).await;
    checker.check("DELETE", "/overrides/{id}", 200, &removed);
    let external = "/api/v1/overrides/external?type=movie&tmdb=8392&instance=inst-1";
    let pin = json!({ "target_category": "anime" });
    checker.check("PUT", "/overrides/external", 200, &app.put(external, pin).await);
    checker.check("DELETE", "/overrides/external", 200, &app.delete(external).await);

    let synced = app.post("/api/v1/instances/sync", json!({})).await;
    checker.check("POST", "/instances/sync", 200, &synced);
    let synced = app.post("/api/v1/instances/inst-1/sync", json!({})).await;
    checker.check("POST", "/instances/{id}/sync", 200, &synced);
    // An answer that does not wait has a schema of its own.
    let started = app
        .send(
            axum::http::Request::post("/api/v1/instances/inst-1/sync")
                .header("prefer", "respond-async")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    checker.check("POST", "/instances/{id}/sync", 202, &started);

    let tasks = app.get("/api/v1/jobs").await;
    checker.check("GET", "/jobs", 200, &tasks);
    let task = started.json["job_id"].as_str().unwrap();
    checker.check("GET", "/jobs/{id}", 200, &app.get(&format!("/api/v1/jobs/{task}")).await);

    // The sync is followed by the work after it, whose simulation holds the
    // lock a persisting one takes: waited for, or `/simulate` below meets it.
    finished(&app, task).await;
    let followed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let chain = app.state.post_sync.lock().await.take();
            if let Some(chain) = chain {
                return chain.await.unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    followed.await.expect("the sync was never followed");

    // The answers that do not wait, each a schema of its own.
    let pending = app.get("/api/v1/decisions?status=pending").await;
    let ids: Vec<Value> =
        pending.json["data"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect();
    let proposals = app.post("/api/v1/simulate", json!({ "persist": true })).await;
    assert_eq!(proposals.status, StatusCode::OK, "{}", proposals.json);
    for (route, body) in [
        ("/simulate", json!({ "persist": false })),
        (
            "/decisions/apply",
            json!({ "decision_ids": [ids[0].clone()], "confirm": every_guardrail }),
        ),
        (
            "/decisions/revert",
            json!({ "decision_ids": [ids[0].clone()], "confirm": every_guardrail }),
        ),
        (
            "/decisions/apply-all",
            json!({ "simulation_id": proposals.json["simulation_id"], "confirm": every_guardrail }),
        ),
    ] {
        let request = axum::http::Request::post(format!("/api/v1{route}"))
            .header("prefer", "respond-async")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let started = app.send(request).await;
        checker.check("POST", route, 202, &started);
        finished(&app, started.json["job_id"].as_str().unwrap()).await;
    }
    // Last: the simulation that follows a sync holds the lock the ones above take.
    let started = app.send(preferring_async("/api/v1/instances/sync", json!({}))).await;
    checker.check("POST", "/instances/sync", 202, &started);
    finished(&app, started.json["job_id"].as_str().unwrap()).await;

    let documented = checker.documented_successes();
    let missed: Vec<_> = documented.difference(&checker.seen).collect();
    assert!(missed.is_empty(), "documented successes never checked: {missed:?}");
}
