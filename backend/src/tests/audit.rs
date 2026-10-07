//! The security log: each decision about who may do what, and each change to
//! a credential or a setting, leaves a line and a row of the security log
//! screen naming who and from where, and never the credential itself.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::layer::SubscriberExt;

use super::{LogCapture, TestApp, TestResponse};

const MASTER: &str = "the-master-key-of-this-installation";

/// What `request` answers, and what it logged.
async fn logged(app: &TestApp, request: Request<Body>) -> (TestResponse, String) {
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let response = app.send(request).with_subscriber(tracing::Dispatch::new(subscriber)).await;
    (response, capture.contents())
}

/// The security log as the owner reads it, once every event so far is stored.
async fn security_log(app: &TestApp, key: Option<&str>, query: &str) -> serde_json::Value {
    app.state.audit.flush().await;
    let mut request = Request::get(format!("/api/v1/security-log{query}"));
    if let Some(key) = key {
        request = request.header("x-api-key", key);
    }
    app.send(request.body(Body::empty()).unwrap()).await.assert_ok().clone()
}

/// A request from `address`, as the listener hands it on.
fn from(address: [u8; 4], request: axum::http::request::Builder, body: Body) -> Request<Body> {
    let mut request = request.body(body).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((address, 41000))));
    request
}

/// A guessing run against the master key leaves a line per address, which a
/// fail2ban filter reads, and the key tried is in none of them.
#[tokio::test]
async fn a_wrong_key_is_logged_with_its_address_and_without_the_key() {
    let app = TestApp::with_api_key(MASTER).await;
    let tried = "a-key-somebody-guessed";
    let request = from(
        [198, 51, 100, 7],
        Request::get("/api/v1/status").header("x-api-key", tried),
        Body::empty(),
    );

    let (response, log) = logged(&app, request).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(log.contains(" WARN ") && log.contains("routarr::audit:"), "{log}");
    assert!(log.contains("client=198.51.100.7") && log.contains("event=\"api_key\""), "{log}");
    assert!(!log.contains(tried), "the key tried is in the log:\n{log}");

    let kept = security_log(&app, Some(MASTER), "").await;
    let event = &kept["data"][0];
    assert_eq!(event["kind"], "api_key", "{kept}");
    assert_eq!(event["outcome"], "refused", "{kept}");
    assert_eq!(event["client"], "198.51.100.7", "{kept}");
    assert_eq!(event["message"], "AuditApiKeyRefused", "{kept}");
    assert!(!kept.to_string().contains(tried), "the key tried is in the table: {kept}");
}

/// A revoked key retried is told apart from noise by its id, which names the
/// row, and never by its secret.
#[tokio::test]
async fn a_revoked_application_key_is_logged_by_its_id() {
    let app = TestApp::new().await;
    let minted = app.post("/api/v1/applications", json!({ "name": "old-bot" })).await;
    let token = minted.assert_ok()["token"].as_str().unwrap().to_string();
    let (id, secret) = token.strip_prefix("rtr_").unwrap().split_once('_').unwrap();
    app.send(Request::delete(format!("/api/v1/applications/{id}")).body(Body::empty()).unwrap())
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let request = from(
        [203, 0, 113, 9],
        Request::get("/api/v1/status").header("x-api-key", &token),
        Body::empty(),
    );
    let (response, log) = logged(&app, request).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(log.contains("event=\"application_key\"") && log.contains(id), "{log}");
    assert!(!log.contains(secret), "the key's secret is in the log:\n{log}");
}

/// Making a credential is the change worth knowing who made: the line names
/// the caller and what the key may do, and not its token.
#[tokio::test]
async fn minting_a_key_is_logged_with_who_asked() {
    let app = TestApp::with_api_key(MASTER).await;
    let request = from(
        [192, 168, 1, 40],
        Request::post("/api/v1/applications")
            .header("x-api-key", MASTER)
            .header("content-type", "application/json"),
        Body::from(json!({ "name": "nightly", "scopes": ["operate"] }).to_string()),
    );

    let (response, log) = logged(&app, request).await;
    let token = response.assert_ok()["token"].as_str().unwrap().to_string();
    assert!(log.contains(" INFO ") && log.contains("routarr::audit:"), "{log}");
    assert!(log.contains("subject=\"apikey\"") && log.contains("client=192.168.1.40"), "{log}");
    assert!(log.contains("nightly") && log.contains("operate"), "{log}");
    assert!(!log.contains(&token) && !log.contains(MASTER), "a key is in the log:\n{log}");

    let kept = security_log(&app, Some(MASTER), "?kind=application_key").await;
    let event = &kept["data"][0];
    assert_eq!(event["subject"], "apikey", "{kept}");
    assert_eq!(event["message"], "AuditApplicationKeyMade", "{kept}");
    assert_eq!(event["params"]["name"], "nightly", "{kept}");
    assert_eq!(event["params"]["scopes"], "operate", "{kept}");
    let kept = kept.to_string();
    assert!(!kept.contains(&token) && !kept.contains(MASTER), "a key is in the table: {kept}");
}

/// An archive carries the master key, so taking one away is a line: who and
/// from where.
#[tokio::test]
async fn downloading_a_backup_is_logged() {
    let (app, _dir) = super::backup::app_with_files("audit-download").await;
    let taken = app.post("/api/v1/backups", json!({})).await;
    let name = taken.assert_ok()["name"].as_str().unwrap().to_string();

    let request =
        from([192, 168, 1, 41], Request::get(format!("/api/v1/backups/{name}")), Body::empty());
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let response = app.send_raw(request).with_subscriber(tracing::Dispatch::new(subscriber)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let log = capture.contents();
    assert!(log.contains("event=\"backup\"") && log.contains(&name), "{log}");
    assert!(log.contains("client=192.168.1.41"), "{log}");
}

/// A refusal anyone can send as fast as they like is summed per address in
/// the table as in the log: a guessing run is one row a minute, not one a
/// request.
#[tokio::test]
async fn a_refusal_sent_again_within_the_minute_is_one_row() {
    let app = TestApp::with_api_key(MASTER).await;
    for _ in 0..5 {
        let request = from(
            [198, 51, 100, 8],
            Request::get("/api/v1/status").header("x-api-key", "a-guess"),
            Body::empty(),
        );
        app.send(request).await.assert_status(StatusCode::UNAUTHORIZED);
    }

    let kept = security_log(&app, Some(MASTER), "").await;
    assert_eq!(kept["pagination"]["total"], 1, "{kept}");
}

/// The screen narrows the log to one kind of event, one outcome, or a name or
/// an address.
#[tokio::test]
async fn the_security_log_narrows_to_a_kind_an_outcome_or_an_address() {
    let app = TestApp::with_api_key(MASTER).await;
    let guess = from(
        [203, 0, 113, 20],
        Request::get("/api/v1/status").header("x-api-key", "a-guess"),
        Body::empty(),
    );
    app.send(guess).await.assert_status(StatusCode::UNAUTHORIZED);
    let saved = from(
        [192, 168, 1, 50],
        Request::put("/api/v1/settings")
            .header("x-api-key", MASTER)
            .header("content-type", "application/json"),
        Body::from(json!({ "settings": { "batch_limit": "20" } }).to_string()),
    );
    app.send(saved).await.assert_ok();

    let kinds = |page: &serde_json::Value| -> Vec<String> {
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap().into())
            .collect()
    };
    assert_eq!(kinds(&security_log(&app, Some(MASTER), "").await), ["settings", "api_key"]);
    assert_eq!(kinds(&security_log(&app, Some(MASTER), "?kind=settings").await), ["settings"]);
    assert_eq!(kinds(&security_log(&app, Some(MASTER), "?outcome=refused").await), ["api_key"]);
    assert_eq!(kinds(&security_log(&app, Some(MASTER), "?search=203.0.113").await), ["api_key"]);
    assert_eq!(kinds(&security_log(&app, Some(MASTER), "?search=apikey").await), ["settings"]);
    let saved = security_log(&app, Some(MASTER), "?kind=settings").await;
    assert_eq!(saved["data"][0]["params"]["names"], "batch_limit", "{saved}");
}

/// The export is read by a person, in the language the interface speaks.
#[tokio::test]
async fn the_export_says_each_event_in_the_configured_language() {
    let app = TestApp::with_api_key(MASTER).await;
    app.store_setting("ui_language", "fr").await;
    let guess = from(
        [203, 0, 113, 21],
        Request::get("/api/v1/status").header("x-api-key", "a-guess"),
        Body::empty(),
    );
    app.send(guess).await.assert_status(StatusCode::UNAUTHORIZED);
    app.state.audit.flush().await;

    let request = Request::get("/api/v1/security-log/export").header("x-api-key", MASTER);
    let response = app.send_raw(request.body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let csv = String::from_utf8(body.to_vec()).unwrap();
    let french = crate::localization::Localizer::new("fr").translate("AuditApiKeyRefused", &[]);
    let english = crate::localization::Localizer::new("en").translate("AuditApiKeyRefused", &[]);
    assert_ne!(french, english, "French falls back to English, which proves nothing here");
    assert!(csv.lines().nth(1).is_some_and(|line| line.contains(&french)), "{csv}");
    assert!(csv.contains("203.0.113.21"), "{csv}");
}
