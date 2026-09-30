//! Application keys: what each one reaches, what it may answer, and what it
//! leaves its name on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

use super::fake_arr::FakeArr;
use super::security::{OPEN, declared_routes};
use super::{TestApp, TestResponse};
use crate::api::applications::{GRANTS, scope_for};
use crate::services::applications::Scope;

const MASTER: &str = "master-key-for-the-owner";

/// Make a key as the owner and return its token.
async fn mint(app: &TestApp, owner_key: Option<&str>, body: Value) -> String {
    let response = send(app, "POST", "/api/v1/applications", owner_key, Some(body)).await;
    response.assert_ok()["token"].as_str().expect("a token").to_string()
}

async fn send(
    app: &TestApp,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> TestResponse {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        request = request.header("x-api-key", key);
    }
    let request = match body {
        Some(body) => request
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    };
    app.send(request.unwrap()).await
}

/// The template a declared path was probed under, as `GRANTS` writes it.
fn template(path: &str) -> String {
    path.strip_prefix("/api/v1").unwrap_or(path).replace("probe", "{id}")
}

/// Every route but the open few, walked with `token`: each answers as the
/// scopes decide, and a route `GRANTS` does not name is refused whatever the
/// key holds.
async fn walk(app: &TestApp, token: &str, holds: &[Scope]) {
    let routes = declared_routes();
    assert!(routes.len() > 40, "only {} route(s) parsed out of main.rs", routes.len());
    let mut reached = 0;
    for (method, path) in routes {
        if OPEN.contains(&(method, path.as_str())) {
            continue;
        }
        let method_name: axum::http::Method = method.parse().unwrap();
        let granted = scope_for(&method_name, &template(&path))
            .is_some_and(|scope| scope == Scope::Read || holds.contains(&scope));
        let status = send(app, method, &path, Some(token), Some(json!({}))).await.status;
        if granted {
            reached += 1;
            assert!(
                status != StatusCode::FORBIDDEN && status != StatusCode::UNAUTHORIZED,
                "{method} {path} answered {status} to a key holding {holds:?}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} let a key holding {holds:?} in"
            );
        }
    }
    assert!(reached > 5, "the walk reached only {reached} route(s): it proves nothing");
}

#[tokio::test]
async fn a_read_key_reaches_the_read_routes_and_nothing_else() {
    let app = TestApp::with_api_key(MASTER).await;
    let token = mint(&app, Some(MASTER), json!({ "name": "homepage" })).await;
    walk(&app, &token, &[]).await;
}

#[tokio::test]
async fn a_key_with_every_scope_still_reaches_no_route_of_the_owner() {
    let app = TestApp::with_api_key(MASTER).await;
    let token =
        mint(&app, Some(MASTER), json!({ "name": "cron", "scopes": ["operate", "write"] })).await;
    walk(&app, &token, &[Scope::Operate, Scope::Write]).await;
}

/// A pinning bot cannot apply, and an applying cron cannot pin.
#[tokio::test]
async fn each_scope_is_granted_on_its_own() {
    let app = TestApp::with_api_key(MASTER).await;
    let pinning = mint(&app, Some(MASTER), json!({ "name": "bot", "scopes": ["write"] })).await;
    let operating =
        mint(&app, Some(MASTER), json!({ "name": "cron", "scopes": ["operate"] })).await;

    let apply = json!({ "decision_ids": [] });
    let pin = json!({ "media_id": "m-1", "target_category": "anime" });
    let refused = send(&app, "POST", "/api/v1/decisions/apply", Some(&pinning), Some(apply)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.json);
    assert!(refused.message().contains("operate"), "{}", refused.message());
    let refused = send(&app, "POST", "/api/v1/overrides", Some(&operating), Some(pin)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.json);
    assert!(refused.message().contains("write"), "{}", refused.message());
}

/// Every route the table grants exists, so no line of it is a promise the
/// router does not keep.
#[test]
fn every_granted_route_is_declared() {
    let declared: Vec<(String, String)> = declared_routes()
        .into_iter()
        .map(|(method, path)| (method.to_string(), template(&path)))
        .collect();
    for (method, route, _) in GRANTS {
        assert!(
            declared.contains(&(method.to_string(), route.to_string())),
            "{method} {route} is granted but main.rs does not declare it"
        );
    }
}

#[tokio::test]
async fn a_revoked_or_unknown_key_is_refused_even_where_nothing_is_asked() {
    // `none`: a request with no key at all is let through.
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "old-script" })).await;
    let id = send(&app, "GET", "/api/v1/applications", None, None).await.assert_ok()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&app, "GET", "/api/v1/status", Some(&token), None).await.assert_ok();

    let revoked = send(&app, "DELETE", &format!("/api/v1/applications/{id}"), None, None).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);

    let refused = send(&app, "GET", "/api/v1/status", Some(&token), None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "a revoked key fell through");
    let forged = format!("{}_{}", &token[..token.rfind('_').unwrap()], "0".repeat(64));
    let refused = send(&app, "GET", "/api/v1/status", Some(&forged), None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "a wrong secret was accepted");
    let refused = send(&app, "GET", "/api/v1/status", Some("rtr_nothing"), None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "a malformed key was accepted");
    send(&app, "GET", "/api/v1/status", None, None).await.assert_ok();
}

/// In an open mode a request without a key keeps what it has always had, and
/// one presenting a key is held to it.
#[tokio::test]
async fn in_an_open_mode_a_presented_key_is_held_to_its_scopes() {
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "homepage" })).await;

    let refused = send(&app, "POST", "/api/v1/simulate", Some(&token), Some(json!({}))).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    send(&app, "POST", "/api/v1/simulate", None, Some(json!({}))).await.assert_ok();
}

#[tokio::test]
async fn the_token_is_shown_once_and_only_its_hash_is_kept() {
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "n8n", "scopes": ["operate"] })).await;
    assert!(token.starts_with("rtr_"), "{token}");
    let secret = token.rsplit_once('_').unwrap().1;
    assert_eq!(secret.len(), 64);

    let listed = send(&app, "GET", "/api/v1/applications", None, None).await;
    let listed = listed.assert_ok().to_string();
    assert!(!listed.contains(secret), "the list gave the secret again: {listed}");
    assert!(listed.contains("\"scopes\":[\"operate\"]"), "{listed}");

    let stored: String = sqlx::query_scalar("SELECT secret_hash FROM api_keys")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_ne!(stored, secret);
    assert_eq!(stored.len(), 64);
}

#[tokio::test]
async fn a_name_is_required_short_and_unique_among_live_keys() {
    let app = TestApp::new().await;
    let blank = send(&app, "POST", "/api/v1/applications", None, Some(json!({ "name": "  " })));
    assert_eq!(blank.await.status, StatusCode::BAD_REQUEST);
    let long = json!({ "name": "x".repeat(65) });
    let long = send(&app, "POST", "/api/v1/applications", None, Some(long)).await;
    assert_eq!(long.status, StatusCode::BAD_REQUEST);
    let unknown = json!({ "name": "bot", "may_confirm": ["everything"] });
    let unknown = send(&app, "POST", "/api/v1/applications", None, Some(unknown)).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);

    mint(&app, None, json!({ "name": "bot" })).await;
    let taken = send(&app, "POST", "/api/v1/applications", None, Some(json!({ "name": " bot " })));
    let taken = taken.await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    assert!(taken.message().contains("bot"), "{}", taken.message());

    let id = send(&app, "GET", "/api/v1/applications", None, None).await.assert_ok()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    send(&app, "DELETE", &format!("/api/v1/applications/{id}"), None, None).await;
    mint(&app, None, json!({ "name": "bot" })).await;
}

#[tokio::test]
async fn an_application_leaves_its_name_on_the_tasks_and_decisions_it_starts() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let token = mint(&app, None, json!({ "name": "n8n", "scopes": ["operate"] })).await;

    let body = json!({ "persist": true });
    send(&app, "POST", "/api/v1/simulate", Some(&token), Some(body)).await.assert_ok();

    let job: (String, Option<String>) =
        sqlx::query_as("SELECT trigger, subject FROM jobs WHERE kind = 'simulate'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(job, ("api".to_string(), Some("n8n".to_string())));
    let jobs = send(&app, "GET", "/api/v1/jobs", Some(&token), None).await;
    assert_eq!(jobs.assert_ok()["data"][0]["subject"], "n8n", "{:?}", jobs.json);

    let decisions: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT actor, subject FROM decisions")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert!(!decisions.is_empty());
    for decision in decisions {
        assert_eq!(decision, (Some("api".to_string()), Some("n8n".to_string())));
    }
}

/// A key not given a guardrail sees the question with `answerable: false`,
/// and sending the name back does not lift it. The owner sees the same
/// question marked answerable.
#[tokio::test]
async fn a_guardrail_a_key_was_not_given_is_referred_to_a_person() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let simulation = app.simulate().await;
    let bare = mint(&app, None, json!({ "name": "cron", "scopes": ["operate"] })).await;
    let trusted = json!({ "name": "trusted", "scopes": ["operate"], "may_confirm": ["batch"] });
    let trusted = mint(&app, None, trusted).await;
    let body = json!({ "simulation_id": simulation, "confirm": ["batch"] });

    let referred =
        send(&app, "POST", "/api/v1/decisions/apply-all", Some(&bare), Some(body.clone())).await;
    let referred = referred.assert_status(StatusCode::CONFLICT);
    assert_eq!(referred["error"], "confirmation_required");
    assert_eq!(referred["confirm"], "batch");
    assert_eq!(referred["answerable"], false);
    assert!(arr.recorded().writes.is_empty(), "a refused key moved something");

    let asked = json!({ "simulation_id": simulation });
    let asked = send(&app, "POST", "/api/v1/decisions/apply-all", None, Some(asked)).await;
    assert_eq!(asked.assert_status(StatusCode::CONFLICT)["answerable"], true);

    let applied =
        send(&app, "POST", "/api/v1/decisions/apply-all", Some(&trusted), Some(body)).await;
    assert_eq!(applied.assert_ok()["applied"], 2);
}

#[tokio::test]
async fn a_key_not_allowed_to_move_files_is_refused_before_anything_moves() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let simulation = app.simulate().await;
    let token = json!({ "name": "cron", "scopes": ["operate"], "may_confirm": ["batch"] });
    let token = mint(&app, None, token).await;

    let body = json!({ "simulation_id": simulation, "move_files": true, "confirm": ["batch"] });
    let refused = send(&app, "POST", "/api/v1/decisions/apply-all", Some(&token), Some(body));
    let refused = refused.await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.json);
    assert_eq!(refused.json["error"], "forbidden");
    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn a_pin_names_the_application_that_set_it() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let token = mint(&app, None, json!({ "name": "request-bot", "scopes": ["write"] })).await;

    let pin = json!({ "media_id": "m-1", "target_category": "anime" });
    let set = send(&app, "POST", "/api/v1/overrides", Some(&token), Some(pin)).await;
    assert_eq!(set.assert_ok()["subject"], "request-bot");
    let listed = send(&app, "GET", "/api/v1/overrides", Some(&token), None).await;
    assert_eq!(listed.assert_ok()[0]["subject"], "request-bot");
}

#[tokio::test]
async fn the_last_use_of_a_key_is_recorded() {
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "homepage" })).await;
    let before = send(&app, "GET", "/api/v1/applications", None, None).await;
    assert_eq!(before.assert_ok()[0]["last_used_at"], Value::Null);

    send(&app, "GET", "/api/v1/status", Some(&token), None).await.assert_ok();

    // Written off the request, so it lands a moment later.
    for _ in 0..100 {
        let used: Option<String> = sqlx::query_scalar("SELECT last_used_at FROM api_keys")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        if used.is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the last use was never recorded");
}

/// The owner routes are the owner's: a key cannot list, make or revoke keys,
/// however many scopes it holds.
#[tokio::test]
async fn no_key_can_mint_another() {
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "all", "scopes": ["operate", "write"] })).await;
    let body = Some(json!({ "name": "escalated", "scopes": ["operate"] }));
    let refused = send(&app, "POST", "/api/v1/applications", Some(&token), body).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.message().contains("owner"), "{}", refused.message());
}
