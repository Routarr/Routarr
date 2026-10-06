//! The security log: each decision about who may do what, and each change to
//! a credential or a setting, leaves a line naming who and from where, and
//! never the credential itself.

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
