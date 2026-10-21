//! The paths the contract renamed, answered under their old names as deprecated aliases: the
//! same handlers, the same scopes, the same answers, said deprecated on each one.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;

use super::TestApp;
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

/// An application key is let through an old path exactly where it is let through the new one,
/// whatever the method: a read key and a write key, each refused somewhere and admitted
/// somewhere, so the comparison is not of two refusals.
#[tokio::test]
async fn an_application_key_reaches_an_old_path_as_it_reaches_its_new_one() {
    const OWNER: &str = "owner-key-of-the-aliases";
    let app = TestApp::with_api_key(OWNER).await;
    let mut refused_somewhere = 0;
    let mut admitted_somewhere = 0;
    for (name, scopes) in [("dashboard", json!([])), ("bot", json!(["write"]))] {
        let minted = app
            .send(
                Request::post("/api/v1/applications")
                    .header("x-api-key", OWNER)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "name": name, "scopes": scopes }).to_string()))
                    .unwrap(),
            )
            .await;
        let token = minted.assert_ok()["token"].as_str().unwrap().to_string();
        for (old, new) in DEPRECATED_PATHS {
            for method in ["GET", "POST", "PUT", "DELETE"] {
                let refused = |path: &str| {
                    app.state.key_rates.refill();
                    let request = Request::builder()
                        .method(method)
                        .uri(format!("/api/v1{}", path.replace("{id}", "x")))
                        .header("x-api-key", &token)
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap();
                    async { app.send(request).await.status == StatusCode::FORBIDDEN }
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
