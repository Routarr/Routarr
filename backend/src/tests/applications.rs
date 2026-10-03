//! Application keys: what each one reaches, what it may answer, and what it
//! leaves its name on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

use super::fake_arr::FakeArr;
use super::security::{OPEN, declared_routes, route_template};
use super::{TestApp, TestResponse};
use crate::api::applications::{GRANTS, scope_for};
use crate::config::{AuthMode, Config, normalise_base_path};
use crate::services::applications::Scope;
use crate::state::AppState;

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
        let granted = scope_for(&method_name, &route_template(&path))
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

/// What a key holding no scope beyond `read` is refused, written by hand
/// rather than read from `GRANTS`: a grant lowered to `read` in the table
/// leaves this list behind, and the walk above, which reads the table, would
/// follow it.
const BEYOND_READ: &[(&str, &str)] = &[
    ("GET", "/api/v1/health"),
    ("POST", "/api/v1/instances/sync"),
    ("POST", "/api/v1/instances/probe/sync"),
    ("POST", "/api/v1/simulate"),
    ("POST", "/api/v1/decisions/apply"),
    ("POST", "/api/v1/decisions/apply-all"),
    ("POST", "/api/v1/decisions/revert"),
    ("POST", "/api/v1/overrides"),
    ("DELETE", "/api/v1/overrides/probe"),
    ("PUT", "/api/v1/overrides/external"),
    ("DELETE", "/api/v1/overrides/external"),
];

#[tokio::test]
async fn a_read_key_changes_nothing_whatever_the_table_says() {
    let app = TestApp::with_api_key(MASTER).await;
    let token = mint(&app, Some(MASTER), json!({ "name": "homepage" })).await;

    let writes: Vec<_> = declared_routes()
        .into_iter()
        .filter(|(method, path)| *method != "GET" && !OPEN.contains(&(*method, path.as_str())))
        .collect();
    assert!(writes.len() > 10, "only {} write route(s) parsed out of main.rs", writes.len());
    let named = BEYOND_READ.iter().map(|(method, path)| (*method, path.to_string()));
    for (method, path) in writes.into_iter().chain(named) {
        let status = send(&app, method, &path, Some(&token), Some(json!({}))).await.status;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} answered a read key {status}");
    }
}

/// An operator chooses the master key, and one that starts like an
/// application key is still the owner's.
#[tokio::test]
async fn a_master_key_shaped_like_an_application_key_keeps_the_owners_reach() {
    let master = "rtr_the_owner_chose_this";
    let app = TestApp::with_api_key(master).await;
    let token = mint(&app, Some(master), json!({ "name": "homepage" })).await;

    send(&app, "GET", "/api/v1/applications", Some(master), None).await.assert_ok();
    let refused = send(&app, "GET", "/api/v1/applications", Some(&token), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.json);
}

#[tokio::test]
async fn an_application_key_sent_as_a_bearer_token_is_held_to_its_scopes() {
    let app = TestApp::with_api_key(MASTER).await;
    let token = mint(&app, Some(MASTER), json!({ "name": "homepage" })).await;
    let bearer = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap()
    };

    assert_eq!(app.send(bearer("GET", "/api/v1/status")).await.status, StatusCode::OK);
    for (method, path) in [("POST", "/api/v1/decisions/apply"), ("GET", "/api/v1/applications")] {
        let status = app.send(bearer(method, path)).await.status;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} answered a bearer read key");
    }
}

/// A key already stored is admitted by its token: the row below is built by
/// hand, its hash written out from SHA-256 itself, so a change to the label,
/// the encoding or the hex case of the digest refuses every key in service.
#[tokio::test]
async fn a_key_stored_by_hand_is_admitted_by_its_token() {
    let app = TestApp::with_api_key(MASTER).await;
    app.execute(&["INSERT INTO api_keys (id, name, secret_hash, scopes, may_confirm,
                                         may_move_files, created_by)
           VALUES ('k-1', 'homepage', 'a5c025dbc98bb2ecde41b35bcc38bf4a1229caba397033a9aa76a47ec604ce7b', '[\"read\"]', '[]', 0, 'apikey')"])
        .await;

    let admitted = send(&app, "GET", "/api/v1/status", Some("rtr_k-1_already-stored"), None).await;
    assert_eq!(admitted.status, StatusCode::OK, "{}", admitted.json);
    let refused = send(&app, "GET", "/api/v1/status", Some("rtr_k-1_already-storeD"), None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "the control: another secret");
}

/// Under a mount point the matched route carries the prefix, and the grant
/// is still found by the part after `/api/v1`.
#[tokio::test]
async fn an_application_key_is_held_to_its_scopes_under_a_mount_point() {
    let mut config = Config::for_tests();
    config.api_key = Some(MASTER.to_string());
    config.auth_mode = AuthMode::ApiKey;
    config.base_path = normalise_base_path("/routarr");
    let app = TestApp::around(AppState::for_tests().await.with_config(config));
    let body = json!({ "name": "homepage" });
    let minted = send(&app, "POST", "/routarr/api/v1/applications", Some(MASTER), Some(body)).await;
    let token = minted.assert_ok()["token"].as_str().expect("a token").to_string();

    send(&app, "GET", "/routarr/api/v1/status", Some(&token), None).await.assert_ok();
    let one = send(&app, "GET", "/routarr/api/v1/media/probe", Some(&token), None).await;
    assert_eq!(one.status, StatusCode::NOT_FOUND, "{:?}", one.json);
    for (method, path) in
        [("POST", "/routarr/api/v1/instances/probe/sync"), ("GET", "/routarr/api/v1/applications")]
    {
        let status = send(&app, method, path, Some(&token), Some(json!({}))).await.status;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} answered a read key {status}");
    }
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
        .map(|(method, path)| (method.to_string(), route_template(&path)))
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
    // Gone from the list, and revoked once only.
    let listed = send(&app, "GET", "/api/v1/applications", None, None).await;
    assert_eq!(listed.assert_ok().as_array().unwrap().len(), 0, "a revoked key is still listed");
    let again = send(&app, "DELETE", &format!("/api/v1/applications/{id}"), None, None).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND, "a key was revoked twice");

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

/// The three routes that move files, each over two films a simulation
/// proposes, with the body a caller sends and the guardrail it is asked first.
/// The revert's films are applied by the owner beforehand. The threshold is
/// set at one, so moving both asks it.
const MOVING: [(&str, &str); 3] =
    [("apply", "threshold"), ("apply-all", "batch"), ("revert", "threshold")];

async fn ready_to_move(arr: &FakeArr, route: &str) -> (TestApp, Value) {
    let app = TestApp::films_to_move(arr, 2).await;
    let simulation = app.simulate().await;
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM decisions WHERE simulation_id = ? ORDER BY id")
            .bind(&simulation)
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let body = match route {
        "apply-all" => json!({ "simulation_id": simulation }),
        "apply" => json!({ "decision_ids": ids }),
        _ => {
            let all = json!({ "simulation_id": simulation, "confirm": ["batch"] });
            let applied = send(&app, "POST", "/api/v1/decisions/apply-all", None, Some(all)).await;
            assert_eq!(applied.assert_ok()["applied"], 2);
            json!({ "decision_ids": ids })
        }
    };
    app.store_setting("confirmation_threshold", "1").await;
    (app, body)
}

/// A key not given a guardrail sees the question with `answerable: false` on
/// every route that moves files, and sending the name back does not lift it.
/// The owner sees the same question marked answerable, and a key given the
/// name moves.
#[tokio::test]
async fn every_route_that_moves_refers_a_guardrail_the_key_was_not_given() {
    for (route, asked) in MOVING {
        let arr = FakeArr::start().await;
        let (app, body) = ready_to_move(&arr, route).await;
        let path = format!("/api/v1/decisions/{route}");
        let before = arr.recorded().writes.len();
        let bare = mint(&app, None, json!({ "name": "cron", "scopes": ["operate"] })).await;
        let mut answered = body.clone();
        answered["confirm"] = json!([asked]);

        let referred = send(&app, "POST", &path, Some(&bare), Some(answered.clone())).await;
        let referred = referred.assert_status(StatusCode::CONFLICT);
        assert_eq!(referred["confirm"], asked, "{route}");
        assert_eq!(referred["answerable"], false, "{route}");
        assert_eq!(arr.recorded().writes.len(), before, "{route}: a refused key moved something");

        let asked_owner = send(&app, "POST", &path, None, Some(body)).await;
        let asked_owner = asked_owner.assert_status(StatusCode::CONFLICT);
        assert_eq!(asked_owner["answerable"], true, "{route}");

        let given = json!({ "name": "trusted", "scopes": ["operate"], "may_confirm": [asked] });
        let given = mint(&app, None, given).await;
        let moved = send(&app, "POST", &path, Some(&given), Some(answered)).await;
        assert_eq!(moved.status, StatusCode::OK, "{route}: {:?}", moved.json);
        assert!(
            arr.recorded().writes.len() > before,
            "{route}: the key given the name moved nothing"
        );
    }
}

/// A key that may not move files is refused before anything runs, on every
/// route that moves them, however many questions it may answer. The same key
/// leaving the files where they are moves the titles.
#[tokio::test]
async fn every_route_that_moves_refuses_files_to_a_key_not_allowed_them() {
    for (route, _) in MOVING {
        let arr = FakeArr::start().await;
        let (app, body) = ready_to_move(&arr, route).await;
        let path = format!("/api/v1/decisions/{route}");
        let before = arr.recorded().writes.len();
        let every = ["batch", "threshold", "capacity", "unreachable"];
        let key = json!({ "name": "cron", "scopes": ["operate"], "may_confirm": every });
        let key = mint(&app, None, key).await;
        let mut sent = body.clone();
        sent["confirm"] = json!(every);
        sent["move_files"] = json!(true);

        let refused = send(&app, "POST", &path, Some(&key), Some(sent.clone())).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{route}: {:?}", refused.json);
        assert_eq!(refused.json["error"], "forbidden");
        assert_eq!(arr.recorded().writes.len(), before, "{route}: files moved");

        sent["move_files"] = json!(false);
        let moved = send(&app, "POST", &path, Some(&key), Some(sent)).await;
        assert_eq!(moved.status, StatusCode::OK, "{route}: {:?}", moved.json);
        assert!(arr.recorded().writes.len() > before, "{route}: nothing moved");
    }
}

/// A key's moves are logged under its name, its apply and its revert both:
/// the Logs screen tells them from the owner's.
#[tokio::test]
async fn a_keys_apply_and_revert_are_logged_under_its_name() {
    let arr = FakeArr::start().await;
    let (app, body) = ready_to_move(&arr, "apply").await;
    let every = ["batch", "threshold", "capacity", "unreachable"];
    let key = json!({ "name": "cron", "scopes": ["operate"], "may_confirm": every });
    let key = mint(&app, None, key).await;
    let mut sent = body.clone();
    sent["confirm"] = json!(every);

    send(&app, "POST", "/api/v1/decisions/apply", Some(&key), Some(sent.clone())).await.assert_ok();
    send(&app, "POST", "/api/v1/decisions/revert", Some(&key), Some(sent)).await.assert_ok();

    let logged: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT action, actor, subject FROM execution_logs WHERE media_id = 'm-0'
          ORDER BY executed_at, rowid",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    let as_cron =
        |action: &str| (action.to_string(), Some("api".to_string()), Some("cron".to_string()));
    assert_eq!(logged, [as_cron("move"), as_cron("revert")]);

    // A whole simulation applied through the key, the third route that moves.
    let arr = FakeArr::start().await;
    let (app, mut body) = ready_to_move(&arr, "apply-all").await;
    let key =
        mint(&app, None, json!({ "name": "cron", "scopes": ["operate"], "may_confirm": every }))
            .await;
    body["confirm"] = json!(every);
    send(&app, "POST", "/api/v1/decisions/apply-all", Some(&key), Some(body)).await.assert_ok();
    let logged: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT DISTINCT actor, subject FROM execution_logs")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(logged, [(Some("api".to_string()), Some("cron".to_string()))]);
}

/// A title pinned by the id another service knows it by names the
/// application that pinned it, as a pin by the library's id does.
#[tokio::test]
async fn a_pin_by_external_id_names_the_application_that_set_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let token = mint(&app, None, json!({ "name": "request-bot", "scopes": ["write"] })).await;

    let path = "/api/v1/overrides/external?type=movie&tmdb=8392";
    let pinned =
        send(&app, "PUT", path, Some(&token), Some(json!({ "target_category": "anime" }))).await;

    assert_eq!(pinned.assert_ok()[0]["subject"], "request-bot");
    let stored: Option<String> = sqlx::query_scalar("SELECT subject FROM overrides")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored.as_deref(), Some("request-bot"));
}

/// Mark a folder as not answering, as a sync that found the NAS asleep does.
async fn asleep(app: &TestApp, path: &str) {
    sqlx::query(
        "UPDATE root_folders SET accessible = 0, last_accessible_at = '2026-09-05 03:00:00'
         WHERE rtrim(path, '/') = ?",
    )
    .bind(path)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

/// The batch question states a sleeping destination rather than asking it
/// apart. Answering it answers that fact too, so a key given the batch
/// question alone meets it marked for a person, and a key given both answers
/// the whole question.
#[tokio::test]
async fn the_batch_question_does_not_answer_a_guardrail_it_states() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let simulation = app.simulate().await;
    asleep(&app, "/movies/anime").await;
    let batch_only = json!({ "name": "cron", "scopes": ["operate"], "may_confirm": ["batch"] });
    let batch_only = mint(&app, None, batch_only).await;
    let apply_all = |confirm: Value| json!({ "simulation_id": simulation, "confirm": confirm });

    let referred = send(
        &app,
        "POST",
        "/api/v1/decisions/apply-all",
        Some(&batch_only),
        Some(apply_all(json!(["batch"]))),
    )
    .await;
    let referred = referred.assert_status(StatusCode::CONFLICT);
    assert_eq!(referred["confirm"], "batch");
    assert_eq!(referred["includes"], json!(["unreachable"]));
    assert_eq!(referred["answerable"], false);
    assert!(arr.recorded().writes.is_empty(), "a key moved titles into a sleeping folder");

    // The owner is asked the one question, and the names it states go back
    // with it.
    let asked = apply_all(json!([]));
    let asked = send(&app, "POST", "/api/v1/decisions/apply-all", None, Some(asked)).await;
    let asked = asked.assert_status(StatusCode::CONFLICT);
    assert_eq!(asked["includes"], json!(["unreachable"]));
    assert_eq!(asked["answerable"], true);
    let half = apply_all(json!(["batch"]));
    let half = send(&app, "POST", "/api/v1/decisions/apply-all", None, Some(half)).await;
    assert_eq!(half.assert_status(StatusCode::CONFLICT)["confirm"], "batch");

    let both = json!({ "name": "trusted", "scopes": ["operate"], "may_confirm": ["batch", "unreachable"] });
    let both = mint(&app, None, both).await;
    let whole = apply_all(json!(["batch", "unreachable"]));
    let applied = send(&app, "POST", "/api/v1/decisions/apply-all", Some(&both), Some(whole)).await;
    assert_eq!(applied.assert_ok()["candidates"], 2, "{:?}", applied.json);
}

/// `subject` holds a person's user name in `forms` and often an e-mail
/// address in `oidc`. An application reads its own name and no one else's,
/// on every list that carries it, while the owner reads them all.
#[tokio::test]
async fn an_application_reads_its_own_name_and_no_one_elses() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.simulate().await;
    let person = "alice@example.com";
    for (id, trigger, subject) in [("j-person", "manual", person), ("j-app", "api", "dashboard")] {
        sqlx::query(
            "INSERT INTO jobs (id, kind, status, trigger, subject) VALUES (?, 'sync', 'success', ?, ?)",
        )
        .bind(id)
        .bind(trigger)
        .bind(subject)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE decisions SET actor = 'manual', subject = ?")
        .bind(person)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO overrides (id, media_id, target_category, subject) VALUES ('o-1', 'm-1', 'anime', ?)")
        .bind(person)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let token = mint(&app, None, json!({ "name": "dashboard" })).await;

    let jobs = send(&app, "GET", "/api/v1/jobs", Some(&token), None).await;
    let jobs = jobs.assert_ok()["data"].as_array().unwrap().clone();
    let named = |id: &str| jobs.iter().find(|job| job["id"] == id).unwrap()["subject"].clone();
    assert_eq!(named("j-person"), Value::Null, "a key read who runs Routarr");
    assert_eq!(named("j-app"), "dashboard");
    let one = send(&app, "GET", "/api/v1/jobs/j-person", Some(&token), None).await;
    assert_eq!(one.assert_ok()["subject"], Value::Null);
    let decisions = send(&app, "GET", "/api/v1/decisions", Some(&token), None).await;
    assert_eq!(decisions.assert_ok()["data"][0]["subject"], Value::Null);
    let pins = send(&app, "GET", "/api/v1/overrides", Some(&token), None).await;
    assert_eq!(pins.assert_ok()[0]["subject"], Value::Null);

    let owner = send(&app, "GET", "/api/v1/jobs/j-person", None, None).await;
    assert_eq!(owner.assert_ok()["subject"], person, "the owner lost sight of who asked");
    let owner = send(&app, "GET", "/api/v1/overrides", None, None).await;
    assert_eq!(owner.assert_ok()[0]["subject"], person);
}

/// `enrich=true` asks every metadata source now, on the owner's quotas, as
/// the `operate` probes of `/health` do. A read key places from what is
/// cached, and asking the sources takes `operate`.
#[tokio::test]
async fn asking_the_sources_now_takes_the_operate_scope() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let reader = mint(&app, None, json!({ "name": "dashboard" })).await;
    let operator = mint(&app, None, json!({ "name": "cron", "scopes": ["operate"] })).await;
    let path = "/api/v1/route?type=movie&tmdb=1";
    let enriched = "/api/v1/route?type=movie&tmdb=1&enrich=true";

    let refused = send(&app, "GET", enriched, Some(&reader), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.json);
    assert_ne!(send(&app, "GET", path, Some(&reader), None).await.status, StatusCode::FORBIDDEN);
    assert_ne!(
        send(&app, "GET", enriched, Some(&operator), None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_ne!(send(&app, "GET", enriched, None, None).await.status, StatusCode::FORBIDDEN);
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

/// A key names who made it: the master key is `apikey`, and an open mode,
/// where nobody signed in, names nobody.
#[tokio::test]
async fn a_key_names_who_made_it() {
    let keyed = TestApp::with_api_key(MASTER).await;
    mint(&keyed, Some(MASTER), json!({ "name": "homepage" })).await;
    let listed = send(&keyed, "GET", "/api/v1/applications", Some(MASTER), None).await;
    assert_eq!(listed.assert_ok()[0]["created_by"], "apikey");

    let open = TestApp::new().await;
    mint(&open, None, json!({ "name": "homepage" })).await;
    let listed = send(&open, "GET", "/api/v1/applications", None, None).await;
    assert_eq!(listed.assert_ok()[0]["created_by"], Value::Null);
}

/// The last use is written at most once a minute: a use within the minute
/// leaves it, one past it moves it.
#[tokio::test]
async fn the_last_use_is_written_at_most_once_a_minute() {
    let app = TestApp::new().await;
    let token = mint(&app, None, json!({ "name": "homepage" })).await;
    let last_use = || async {
        sqlx::query_scalar::<_, String>("SELECT last_used_at FROM api_keys")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    };

    app.execute(&["UPDATE api_keys SET last_used_at = datetime('now', '-30 seconds')"]).await;
    let recent = last_use().await;
    send(&app, "GET", "/api/v1/status", Some(&token), None).await.assert_ok();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(last_use().await, recent, "a use within the minute was written");

    app.execute(&["UPDATE api_keys SET last_used_at = datetime('now', '-2 minutes')"]).await;
    let old = last_use().await;
    send(&app, "GET", "/api/v1/status", Some(&token), None).await.assert_ok();
    for _ in 0..100 {
        if last_use().await != old {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("a use past the minute was not written");
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
