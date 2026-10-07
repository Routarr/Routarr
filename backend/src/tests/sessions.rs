//! Sessions and what a session proves: their lifetime, the screen that lists
//! and ends them, the key a browser exchanges for one, and the proof a session
//! gives before it makes or withdraws a key.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use super::TestApp;
use super::security::{forms_app, generated_password, sign_in, with_session};

const KEY: &str = "the-api-key-of-this-installation-and-long";

/// The cookie a `Set-Cookie` answer hands the browser, as it sends it back.
fn handed(response: &axum::response::Response) -> String {
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

/// A session left open on a shared machine would otherwise make keys that
/// outlive it: making or withdrawing one takes the current password, a key
/// for another application too.
#[tokio::test]
async fn a_session_cannot_mint_a_key_without_the_password() {
    let (app, _dir) = forms_app("proof").await;
    let password = generated_password(&app);
    let session = sign_in(&app, &password).await;

    for refused in [json!({}), json!({ "current_password": "not the password at all" })] {
        let (status, body) =
            with_session(&app, "POST", "/api/v1/auth/api-key", &session, refused.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}: {body}");
        let mut new = json!({ "name": "nightly" });
        new.as_object_mut().unwrap().extend(refused.as_object().unwrap().clone());
        let (status, _) = with_session(&app, "POST", "/api/v1/applications", &session, new).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    }
    assert!(crate::crypto::read_api_key(&app.state.config.api_key_path()).is_none());

    let proof = json!({ "current_password": password });
    let (status, _) = with_session(&app, "POST", "/api/v1/auth/api-key", &session, proof).await;
    assert_eq!(status, StatusCode::OK);
    let new = json!({ "name": "nightly", "current_password": generated_password(&app) });
    let (status, body) = with_session(&app, "POST", "/api/v1/applications", &session, new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A password changed because it leaked closes what it opened, and the keys
/// made with it when asked: the API key is replaced, shown once, and every
/// application key is revoked.
#[tokio::test]
async fn a_password_change_can_revoke_every_key() {
    let (app, _dir) = forms_app("revoke").await;
    let password = generated_password(&app);
    let session = sign_in(&app, &password).await;
    let proof = json!({ "current_password": password });
    let (_, minted) = with_session(&app, "POST", "/api/v1/auth/api-key", &session, proof).await;
    let master = minted["api_key"].as_str().unwrap().to_string();
    let new = json!({ "name": "nightly", "current_password": password });
    let (_, made) = with_session(&app, "POST", "/api/v1/applications", &session, new).await;
    let token = made["token"].as_str().unwrap().to_string();

    let change = json!({
        "current": password, "new_password": "a new password of some length", "revoke_keys": true
    });
    let (status, body) = with_session(&app, "PUT", "/api/v1/auth/password", &session, change).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let replaced = body["api_key"].as_str().expect("the new API key, shown once");

    let status_with = |key: String| {
        let app = &app;
        async move {
            let request = Request::get("/api/v1/status").header("x-api-key", key);
            app.send(request.body(Body::empty()).unwrap()).await.status
        }
    };
    assert_eq!(status_with(master).await, StatusCode::UNAUTHORIZED, "the old API key opens");
    assert_eq!(status_with(token).await, StatusCode::UNAUTHORIZED, "an application key opens");
    assert_eq!(status_with(replaced.to_string()).await, StatusCode::OK);
}

/// A change refused because two sign-ins are already being checked says to
/// come back in a moment: neither a wrong password nor a credential gone.
#[tokio::test]
async fn a_busy_password_check_answers_503() {
    let (app, _dir) = forms_app("busy").await;
    let session = sign_in(&app, &generated_password(&app)).await;
    let change = json!({ "current": "not the password", "new_password": "a new one of length" });

    let attempts = (0..24).map(|_| {
        let request = Request::put("/api/v1/auth/password")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &session)
            .body(Body::from(change.to_string()))
            .unwrap();
        app.send_raw(request)
    });
    let answers = futures::future::join_all(attempts).await;
    let busy: Vec<_> =
        answers.iter().filter(|r| r.status() == StatusCode::SERVICE_UNAVAILABLE).collect();
    assert!(!busy.is_empty(), "twenty-four hashes ran at once");
    for response in busy {
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }
    assert!(
        answers
            .iter()
            .all(|r| matches!(r.status(), StatusCode::FORBIDDEN | StatusCode::SERVICE_UNAVAILABLE))
    );
}

/// The browser sends the key once and keeps a session cookie instead, which
/// no script on the page can read. A wrong key opens nothing, and counts
/// toward the address's wait as a wrong password does.
#[tokio::test]
async fn the_key_opens_a_session_and_the_browser_keeps_no_key() {
    let app = TestApp::with_api_key(KEY).await;
    let exchange = |key: &str| {
        Request::post("/api/v1/auth/key-session")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "key": key }).to_string()))
            .unwrap()
    };

    let refused = app.send(exchange("not the key")).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);

    let opened = app.send_raw(exchange(KEY)).await;
    assert_eq!(opened.status(), StatusCode::OK);
    let set = opened.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(set.contains("HttpOnly") && set.contains("SameSite"), "{set}");
    let cookie = handed(&opened);

    let request = Request::get("/api/v1/status").header(header::COOKIE, &cookie);
    app.send(request.body(Body::empty()).unwrap()).await.assert_ok();
    let (status, _) = with_session(&app, "POST", "/api/v1/simulate", &cookie, json!({})).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED, "a write with the session was refused");
}

/// A cookie travels on a cross-site request whether the page meant it or not:
/// a write whose origin is another site is refused, as in the sign-in modes.
#[tokio::test]
async fn a_write_from_another_site_is_refused_with_a_key_session() {
    let app = TestApp::with_api_key(KEY).await;
    let opened = app
        .send_raw(
            Request::post("/api/v1/auth/key-session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "key": KEY }).to_string()))
                .unwrap(),
        )
        .await;
    let cookie = handed(&opened);

    let request = Request::post("/api/v1/simulate")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "https://elsewhere.example")
        .header(header::HOST, "routarr.lan")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(app.send(request).await.status, StatusCode::FORBIDDEN);
}

/// A session the key opened replaces the key only with the key itself, and a
/// key replaced ends every other session it opened.
#[tokio::test]
async fn a_key_session_proves_itself_with_the_key_and_a_new_key_ends_the_others() {
    let (app, _dir) = super::security::stored_key_app("key-session", KEY).await;
    let open = || {
        let app = &app;
        async move {
            let response = app
                .send_raw(
                    Request::post("/api/v1/auth/key-session")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(json!({ "key": KEY }).to_string()))
                        .unwrap(),
                )
                .await;
            handed(&response)
        }
    };
    let (mine, other) = (open().await, open().await);

    let (status, _) = with_session(&app, "POST", "/api/v1/auth/api-key", &mine, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a key session replaced the key without it");
    let proof = json!({ "current_key": KEY });
    let (status, _) = with_session(&app, "POST", "/api/v1/auth/api-key", &mine, proof).await;
    assert_eq!(status, StatusCode::OK);

    let still = |cookie: String| {
        let app = &app;
        async move {
            let request = Request::get("/api/v1/status").header(header::COOKIE, cookie);
            app.send(request.body(Body::empty()).unwrap()).await.status
        }
    };
    assert_eq!(still(mine).await, StatusCode::OK, "the session that replaced the key ended");
    assert_eq!(still(other).await, StatusCode::UNAUTHORIZED, "the old key's session lives on");
}

/// The sessions screen lists every live session with the mode that opened it,
/// marks the one asking, ends one by its handle, and signs out everywhere.
#[tokio::test]
async fn the_sessions_are_listed_ended_one_by_one_and_all_at_once() {
    let (app, _dir) = forms_app("sessions").await;
    let password = generated_password(&app);
    let (mine, second, third) = (
        sign_in(&app, &password).await,
        sign_in(&app, &password).await,
        sign_in(&app, &password).await,
    );

    let (status, listed) =
        with_session(&app, "GET", "/api/v1/auth/sessions", &mine, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let listed = listed.as_array().unwrap().clone();
    assert_eq!(listed.len(), 3, "{listed:?}");
    assert_eq!(listed.iter().filter(|s| s["current"] == true).count(), 1, "{listed:?}");
    assert!(listed.iter().all(|s| s["source"] == "forms" && s["subject"] == "admin"));
    assert!(
        !listed.iter().any(|s| s.to_string().contains(&mine["routarr_session=".len()..])),
        "a session's id is on the screen"
    );

    let other = listed.iter().find(|s| s["current"] == false).unwrap();
    let handle = other["handle"].as_str().unwrap();
    let ended = format!("/api/v1/auth/sessions/{handle}");
    let (status, _) = with_session(&app, "DELETE", &ended, &mine, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = with_session(&app, "DELETE", &ended, &mine, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a session ended twice");

    let (status, _) =
        with_session(&app, "DELETE", "/api/v1/auth/sessions", &mine, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    for cookie in [mine, second, third] {
        let (status, _) = with_session(&app, "GET", "/api/v1/auth/me", &cookie, Value::Null).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a session outlived signing out everywhere");
    }
}
