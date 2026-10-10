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

/// Every published schema is one an operation answers or takes, or the
/// notification the webhook receives, which no operation names: a schema no
/// call reaches is a model a generated client builds for nothing, and an item
/// described inline beside its own schema is one type defined twice.
#[test]
fn every_schema_is_reached_from_an_operation_or_is_the_webhook_payload() {
    fn references(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    match (key.as_str(), inner.as_str()) {
                        ("$ref", Some(target)) => {
                            found.insert(target.rsplit('/').next().unwrap().to_string());
                        }
                        _ => references(inner, found),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| references(item, found)),
            _ => {}
        }
    }
    let document = described();
    let schemas = document["components"]["schemas"].as_object().unwrap();
    let mut reached = BTreeSet::new();
    references(&document["paths"], &mut reached);
    let mut waiting: Vec<String> = reached.iter().cloned().collect();
    while let Some(name) = waiting.pop() {
        let mut found = BTreeSet::new();
        references(&schemas[&name], &mut found);
        for name in found {
            if reached.insert(name.clone()) {
                waiting.push(name);
            }
        }
    }

    let unreached: Vec<&String> = schemas.keys().filter(|name| !reached.contains(*name)).collect();
    assert_eq!(unreached, ["AllowedMentions", "Notification"]);
}

/// The types that report the state of an installation and its mappings say
/// what each field holds, its values among them: an integrator reads them
/// here, not in the source.
#[test]
fn every_field_of_the_status_and_mapping_types_is_described() {
    let document = described();
    for name in [
        "HealthResponse",
        "InstanceHealth",
        "Warning",
        "MappingConflict",
        "LogEntry",
        "CreateCategoryRequest",
    ] {
        let properties = document["components"]["schemas"][name]["properties"].as_object();
        let undescribed: Vec<&String> = properties
            .unwrap()
            .iter()
            .filter(|(_, field)| field.get("description").is_none() && field.get("$ref").is_none())
            .map(|(field, _)| field)
            .collect();
        assert!(undescribed.is_empty(), "{name} leaves {undescribed:?} undescribed");
    }
}

/// A timestamp is published as RFC 3339, which says its zone: written
/// without one, a client reads it in its own time zone, or not at all.
#[test]
fn every_timestamp_is_published_as_a_date_time() {
    let document = described();
    let mut undated = Vec::new();
    for (schema, body) in document["components"]["schemas"].as_object().unwrap() {
        let Some(properties) = body["properties"].as_object() else { continue };
        for (name, field) in properties {
            let stamped = name.ends_with("_at")
                || ["last_sync", "since", "timestamp"].contains(&name.as_str());
            if stamped && field["format"] != "date-time" {
                undated.push(format!("{schema}.{name}"));
            }
        }
    }
    assert!(undated.is_empty(), "published without a zone: {undated:?}");
}

/// The export takes the log's filters and no paging, which it never reads.
#[test]
fn the_log_export_documents_no_paging() {
    let document = described();
    let named = |path: &str| -> BTreeSet<String> {
        document["paths"][path]["get"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parameter| parameter["name"].as_str().unwrap().to_string())
            .collect()
    };
    let paged = named("/logs");
    let exported = named("/logs/export");
    assert!(paged.contains("page") && paged.contains("per_page"), "{paged:?}");
    assert!(!exported.contains("page") && !exported.contains("per_page"), "{exported:?}");
    let filters: BTreeSet<_> =
        paged.into_iter().filter(|name| name != "page" && name != "per_page").collect();
    assert_eq!(exported, filters);
}

/// A field a request body does not define is refused by name, on every
/// operation that takes a body: ignored, a misspelled field changes what is
/// written, as `categroy` unmapping a folder would. The body is refused
/// before the handler runs, so each one here only has to parse.
#[tokio::test]
async fn every_documented_request_body_refuses_a_field_it_does_not_define() {
    let app = TestApp::new().await;
    let rule = json!({
        "name": "Anime",
        "media_type": "both",
        "target_category": "anime",
        "conditions": [{ "type": "original_language", "value": ["ja"] }]
    });
    let bodies = [
        ("POST", "/categories", "/categories", json!({ "name": "kids" })),
        ("PUT", "/categories/{id}", "/categories/c-1", json!({ "name": "kids" })),
        ("POST", "/decisions/apply", "/decisions/apply", json!({ "decision_ids": ["d-1"] })),
        ("POST", "/decisions/apply-all", "/decisions/apply-all", json!({ "simulation_id": "s" })),
        ("POST", "/decisions/revert", "/decisions/revert", json!({ "decision_ids": ["d-1"] })),
        ("POST", "/overrides", "/overrides", json!({ "media_id": "m", "target_category": "a" })),
        (
            "PUT",
            "/overrides/external",
            "/overrides/external?type=movie&tmdb=8392",
            json!({ "target_category": "anime" }),
        ),
        ("POST", "/root-folders", "/root-folders", json!({ "instance_id": "i", "path": "/p" })),
        (
            "PUT",
            "/root-folders/{id}/category",
            "/root-folders/rf-1/category",
            json!({ "category": null }),
        ),
        ("POST", "/rule-tests", "/rule-tests", json!({ "name": "t", "media_id": "m" })),
        ("POST", "/rules", "/rules", rule.clone()),
        (
            "POST",
            "/rules/import",
            "/rules/import",
            json!({ "bundle": { "version": 2, "rules": [] } }),
        ),
        ("POST", "/rules/preview", "/rules/preview", json!({ "rule": rule })),
        ("POST", "/rules/reorder", "/rules/reorder", json!({ "rule_ids": [] })),
        ("POST", "/rules/validate", "/rules/validate", rule.clone()),
        ("PUT", "/rules/{id}", "/rules/r-1", rule),
        ("POST", "/simulate", "/simulate", json!({})),
    ];

    let mut documented = BTreeSet::new();
    for (path, item) in described()["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            if operation.get("requestBody").is_some() {
                documented.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    let tried: BTreeSet<_> =
        bodies.iter().map(|(method, route, ..)| (method.to_string(), route.to_string())).collect();
    assert_eq!(tried, documented, "an operation taking a body has no case here");

    for (method, route, path, mut body) in bodies {
        body["x_unknown"] = json!(1);
        let path = format!("/api/v1{path}");
        let refused = match method {
            "PUT" => app.put(&path, body).await,
            _ => app.post(&path, body).await,
        };
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{method} {route}: {}", refused.json);
        assert!(refused.message().contains("x_unknown"), "{method} {route}: {}", refused.json);
    }
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
        // A creation says where the new resource is, as the contract states.
        if response.status == StatusCode::CREATED {
            let location = response.location().unwrap_or_default();
            assert!(location.starts_with("/api/v1/"), "{method} {route} answered no Location");
            assert!(responses["201"]["headers"]["Location"].is_object(), "{method} {route}");
        }
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
    checker.check("POST", "/overrides", 201, &set);
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

    // The answers that do not wait, each a schema of its own: a proposal the
    // pass after the sync wrote, applied, reverted, then proposed again by a
    // fresh simulation and applied whole.
    let proposals = app.post("/api/v1/simulate", json!({ "persist": true })).await;
    assert_eq!(proposals.status, StatusCode::OK, "{}", proposals.json);
    let proposed = proposals.json["decisions"][0]["id"].clone();
    let asynchronously = |route: &str, body: Value| {
        axum::http::Request::post(format!("/api/v1{route}"))
            .header("prefer", "respond-async")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };
    for (route, body) in [
        ("/simulate", json!({ "persist": false })),
        ("/decisions/apply", json!({ "decision_ids": [proposed], "confirm": every_guardrail })),
        ("/decisions/revert", json!({ "decision_ids": [proposed], "confirm": every_guardrail })),
    ] {
        let started = app.send(asynchronously(route, body)).await;
        checker.check("POST", route, 202, &started);
        finished(&app, started.json["job_id"].as_str().unwrap()).await;
    }
    let again = app.post("/api/v1/simulate", json!({ "persist": true })).await;
    let body = json!({ "simulation_id": again.json["simulation_id"], "confirm": every_guardrail });
    let started = app.send(asynchronously("/decisions/apply-all", body)).await;
    checker.check("POST", "/decisions/apply-all", 202, &started);
    finished(&app, started.json["job_id"].as_str().unwrap()).await;
    // Last: the simulation that follows a sync holds the lock the ones above take.
    let started = app.send(preferring_async("/api/v1/instances/sync", json!({}))).await;
    checker.check("POST", "/instances/sync", 202, &started);
    finished(&app, started.json["job_id"].as_str().unwrap()).await;

    configure(&app, &mut checker).await;
    keep_backups(&mut checker).await;

    let documented = checker.documented_successes();
    let missed: Vec<_> = documented.difference(&checker.seen).collect();
    assert!(missed.is_empty(), "documented successes never checked: {missed:?}");
}

/// What a configuring application reads and writes: the instances, the
/// sources, the log, the folders and their categories, the rules and their
/// tests. Last of the walk: it changes what the moves above relied on.
async fn configure(app: &TestApp, checker: &mut Checker) {
    for (route, path) in [
        ("/instances", "/api/v1/instances"),
        ("/instances/{id}", "/api/v1/instances/inst-1"),
        ("/metadata/providers", "/api/v1/metadata/providers"),
        ("/media/facets", "/api/v1/media/facets"),
        ("/logs", "/api/v1/logs"),
        ("/root-folders", "/api/v1/root-folders"),
        ("/root-folders/conflicts", "/api/v1/root-folders/conflicts"),
        ("/rules", "/api/v1/rules"),
        ("/rules/conditions", "/api/v1/rules/conditions"),
        ("/rules/health", "/api/v1/rules/health"),
        ("/rules/export", "/api/v1/rules/export"),
        ("/rule-tests", "/api/v1/rule-tests"),
    ] {
        checker.check("GET", route, 200, &app.get(path).await);
    }
    let csv = app.raw("/api/v1/logs/export").await;
    assert_eq!(csv.status(), StatusCode::OK);
    let content_type = csv.headers()[axum::http::header::CONTENT_TYPE].to_str().unwrap();
    assert!(content_type.starts_with("text/csv"), "{content_type}");
    checker.seen.insert(("GET".into(), "/logs/export".into(), "200".into()));

    // The folders and the categories they lead to.
    let declared = app.post(
        "/api/v1/root-folders",
        json!({ "instance_id": "inst-1", "path": "/movies/anime/kids" }),
    );
    let declared = declared.await;
    checker.check("POST", "/root-folders", 201, &declared);
    let folder = declared.json["id"].as_str().unwrap().to_string();
    let mapping = format!("/api/v1/root-folders/{folder}/category");
    let mapped = app.put(&mapping, json!({ "category": null })).await;
    checker.check("PUT", "/root-folders/{id}/category", 200, &mapped);
    let removed = app.delete(&format!("/api/v1/root-folders/{folder}")).await;
    checker.check("DELETE", "/root-folders/{id}", 200, &removed);
    let created = app.post("/api/v1/categories", json!({ "name": "docs" })).await;
    checker.check("POST", "/categories", 201, &created);
    let category = created.json["id"].as_str().unwrap().to_string();
    let renaming = format!("/api/v1/categories/{category}");
    let renamed = app.put(&renaming, json!({ "name": "documentaries" })).await;
    checker.check("PUT", "/categories/{id}", 200, &renamed);
    let removed = app.delete(&format!("/api/v1/categories/{category}")).await;
    checker.check("DELETE", "/categories/{id}", 200, &removed);

    // The rules, written, judged, previewed and tested.
    let draft = json!({
        "name": "Contract",
        "media_type": "both",
        "target_category": "anime",
        "conditions": [{ "type": "genre_contains", "value": ["Animation"] }],
        "exclusions": [{ "type": "year_range", "value": { "min": 1900, "max": null } }],
    });
    checker.check(
        "POST",
        "/rules/validate",
        200,
        &app.post("/api/v1/rules/validate", draft.clone()).await,
    );
    let preview = app.post("/api/v1/rules/preview", json!({ "rule": draft.clone() })).await;
    checker.check("POST", "/rules/preview", 200, &preview);
    let created = app.post("/api/v1/rules", draft.clone()).await;
    checker.check("POST", "/rules", 201, &created);
    let rule = created.json["id"].as_str().unwrap().to_string();
    checker.check("GET", "/rules/{id}", 200, &app.get(&format!("/api/v1/rules/{rule}")).await);
    let updated = app.put(&format!("/api/v1/rules/{rule}"), draft).await;
    checker.check("PUT", "/rules/{id}", 200, &updated);
    let copied = app.post(&format!("/api/v1/rules/{rule}/duplicate"), json!({})).await;
    checker.check("POST", "/rules/{id}/duplicate", 201, &copied);
    let ids: Vec<Value> = app
        .get("/api/v1/rules")
        .await
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].clone())
        .collect();
    let reordered = app.post("/api/v1/rules/reorder", json!({ "rule_ids": ids })).await;
    checker.check("POST", "/rules/reorder", 200, &reordered);
    let bundle = app.get("/api/v1/rules/export").await.json.clone();
    let imported =
        app.post("/api/v1/rules/import", json!({ "bundle": bundle, "replace": false })).await;
    checker.check("POST", "/rules/import", 200, &imported);

    // The syncs above read the library again, under the Arr's own ids.
    let media = app.get("/api/v1/media?per_page=1").await.json["data"][0]["id"].clone();
    let pinned =
        app.post("/api/v1/rule-tests", json!({ "name": "A case", "media_id": media })).await;
    checker.check("POST", "/rule-tests", 201, &pinned);
    checker.check(
        "POST",
        "/rule-tests/run",
        200,
        &app.post("/api/v1/rule-tests/run", json!({})).await,
    );
    let case = pinned.json["id"].as_str().unwrap().to_string();
    let removed = app.delete(&format!("/api/v1/rule-tests/{case}")).await;
    checker.check("DELETE", "/rule-tests/{id}", 200, &removed);
    let removed = app.delete(&format!("/api/v1/rules/{rule}")).await;
    checker.check("DELETE", "/rules/{id}", 200, &removed);
}

/// The backups, on an installation that keeps its files on disk.
async fn keep_backups(checker: &mut Checker) {
    let (app, dir) = super::backup::app_with_files("contract").await;
    let taken = app.post("/api/v1/backups", json!({})).await;
    checker.check("POST", "/backups", 200, &taken);
    checker.check("GET", "/backups", 200, &app.get("/api/v1/backups").await);
    // Archives are named to the second, and the next one may be taken in it.
    std::fs::remove_file(dir.join("backups").join(taken.json["name"].as_str().unwrap())).unwrap();
    let started = app.send(super::preferring_async("/api/v1/backups", json!({}))).await;
    checker.check("POST", "/backups", 202, &started);
    super::finished(&app, started.json["job_id"].as_str().unwrap()).await;
}

/// A closed vocabulary is published as a string with its values, and those
/// are the ones its type writes. A variant added to one of these types goes
/// in its list here and in the `x-extensible-enum` its field publishes.
#[test]
fn each_published_vocabulary_is_the_one_its_type_writes() {
    use crate::api::media::TraceOutcome;
    use crate::models::{DecisionAction, DecisionStatus, RuleMediaType, Severity};
    fn written<T: serde::Serialize>(values: &[T]) -> Value {
        serde_json::to_value(values).expect("a vocabulary serializes")
    }
    let document = described();
    let actions = written(&[DecisionAction::Move, DecisionAction::None, DecisionAction::Skip]);
    for (schema, field, values) in [
        ("Decision", "action", actions.clone()),
        ("Explanation", "action", actions),
        (
            "Decision",
            "status",
            written(&[
                DecisionStatus::Pending,
                DecisionStatus::Requested,
                DecisionStatus::Applied,
                DecisionStatus::Failed,
                DecisionStatus::Skipped,
            ]),
        ),
        (
            "Rule",
            "media_type",
            written(&[RuleMediaType::Movie, RuleMediaType::Series, RuleMediaType::Both]),
        ),
        ("ValidationIssue", "severity", written(&[Severity::Error, Severity::Warning])),
        (
            "RuleTrace",
            "outcome",
            written(&[
                TraceOutcome::Winner,
                TraceOutcome::Excluded,
                TraceOutcome::MatchedLowerPriority,
                TraceOutcome::NotMatched,
            ]),
        ),
    ] {
        let published =
            &document["components"]["schemas"][schema]["properties"][field]["x-extensible-enum"];
        assert_eq!(published, &values, "{schema}.{field}");
    }
}
