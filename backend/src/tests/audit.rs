//! Each decision about who may do what, and each change to a credential or a setting, leaves a
//! line and a security log row naming who and from where, never the credential itself.

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

/// A fail2ban filter reads the address off the line.
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

/// The id tells a revoked key retried from noise, since it names the key's row.
#[tokio::test]
async fn a_revoked_application_key_is_logged_by_its_id() {
    let app = TestApp::new().await;
    let minted = app.post("/api/v1/applications", json!({ "name": "old-bot" })).await;
    let token = minted.assert_ok()["token"].as_str().unwrap().to_string();
    let (id, secret) = token.strip_prefix("rtr_").unwrap().split_once('_').unwrap();
    app.send(Request::delete(format!("/api/v1/applications/{id}")).body(Body::empty()).unwrap())
        .await
        .assert_ok();

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

/// A new credential is the change worth knowing who made.
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

/// An archive carries the master key.
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

    // A name of the right shape that no archive holds downloads nothing, and says nothing was.
    let absent = Request::get("/api/v1/backups/routarr-backup-20990101-000000.zip");
    let absent = app.send(absent.body(Body::empty()).unwrap()).await;
    absent.assert_status(StatusCode::NOT_FOUND);
    app.state.audit.flush().await;
    let downloads: Vec<String> = sqlx::query_scalar(
        "SELECT params FROM security_events WHERE message = 'AuditBackupDownloaded'",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(downloads.len(), 1, "{downloads:?}");
    assert!(downloads[0].contains(&name), "{downloads:?}");
}

/// An Arr's credentials, its webhook token and the archives are credentials like the keys:
/// each change leaves a row, and a webhook sent with a wrong token one naming its address, the
/// one a fail2ban jail bans. None names the key or the token.
#[tokio::test]
async fn credential_changes_and_webhook_refusals_are_logged() {
    let (app, _dir) = super::backup::app_with_files("audit-credentials").await;
    let instance = json!({
        "name": "Radarr", "instance_type": "radarr", "base_url": "http://127.0.0.1:1",
        "api_key": "the-arr-key",
    });
    let created = app.post("/api/v1/instances", instance.clone()).await;
    let id = created.assert_ok()["id"].as_str().unwrap().to_string();
    let mut changed = instance;
    changed["api_key"] = json!("another-arr-key");
    app.put(&format!("/api/v1/instances/{id}"), changed).await.assert_ok();
    let rotated = app.post(&format!("/api/v1/instances/{id}/webhook-token"), json!({})).await;
    let token = rotated.assert_ok()["webhook_token"].as_str().unwrap().to_string();
    let guessed = from(
        [198, 51, 100, 7],
        Request::post(format!("/api/v1/webhook/{id}/a-guessed-token"))
            .header("content-type", "application/json"),
        Body::from(json!({ "eventType": "Test" }).to_string()),
    );
    app.send(guessed).await.assert_status(StatusCode::NOT_FOUND);
    let taken = app.post("/api/v1/backups", json!({})).await;
    let name = taken.assert_ok()["name"].as_str().unwrap().to_string();
    app.delete(&format!("/api/v1/backups/{name}")).await.assert_ok();
    app.delete(&format!("/api/v1/instances/{id}")).await.assert_ok();
    app.state.audit.flush().await;

    let rows: Vec<(String, String, Option<String>, String)> =
        sqlx::query_as("SELECT kind, message, client, params FROM security_events ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let said = |message: &str| rows.iter().filter(|row| row.1 == message).count();
    for message in [
        "AuditInstanceAdded",
        "AuditInstanceCredentialChanged",
        "AuditWebhookTokenRotated",
        "AuditWebhookTokenRefused",
        "AuditBackupDeleted",
        "AuditInstanceRemoved",
    ] {
        assert_eq!(said(message), 1, "{message}: {rows:?}");
    }
    let refused = rows.iter().find(|row| row.1 == "AuditWebhookTokenRefused").unwrap();
    assert_eq!((refused.0.as_str(), refused.2.as_deref()), ("webhook", Some("198.51.100.7")));
    let all = format!("{rows:?}");
    assert!(!all.contains("arr-key") && !all.contains(&token), "a credential is logged: {all}");
}

/// A value an event carries may come from a provider or a person, and a line break in it
/// would start a line a fail2ban filter reads as an audit line of its own.
#[test]
fn an_event_is_one_line_whatever_its_values_hold() {
    let capture = LogCapture::default();
    let event =
        crate::api::auth::refusal(crate::services::audit::Kind::Settings, "AuditSettingsSaved")
            .with("names", "a\nb\r\nc");
    tracing::dispatcher::with_default(&capture.as_the_process_logs(), || {
        crate::services::audit::Log::default().record(event.by(Some("x\ny"), None));
    });

    let log = capture.contents();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains(r"a\nb\r\nc"), "the value is no longer readable: {log}");
}

/// The refused sign-in `SECURITY.md` shows is the line the text log writes, its time and its
/// request id aside: `scripts/check-fail2ban.py` holds the documented filter to that line.
#[tokio::test]
async fn the_refusal_security_md_shows_is_the_line_a_refused_sign_in_writes() {
    let (app, _dir) = super::security::forms_app("documented-line").await;
    let body = json!({ "username": "admin", "password": "not the password at all" });
    let request = from(
        [203, 0, 113, 9],
        Request::post("/api/v1/auth/login").header("content-type", "application/json"),
        Body::from(body.to_string()),
    );
    let capture = LogCapture::default();
    app.send(request).with_subscriber(capture.as_the_process_logs()).await;
    let log = capture.contents();
    let written = log.lines().find(|line| line.contains("routarr::audit:")).expect("no audit line");

    let documented = include_str!("../../../SECURITY.md")
        .lines()
        .find(|line| line.contains("routarr::audit:") && line.contains("outcome=\"refused\""))
        .expect("SECURITY.md shows no refusal");
    let (stamp, _) = documented.split_once(' ').unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(stamp).is_ok() && !stamp.ends_with('Z'),
        "the log writes no time as {stamp}"
    );
    let shape = |line: &str| {
        let (_, rest) = line.split_once(' ').unwrap();
        let (before, after) = rest.split_once("id=\"").unwrap();
        format!("{before}id=\"{}", after.split_once('"').unwrap().1)
    };
    assert_eq!(shape(documented), shape(written));
}

/// Anyone can send a refusal as fast as they like, so a guessing run is one row a minute.
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
    assert!(!response.headers().contains_key("x-routarr-truncated"));
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let csv = String::from_utf8(body.to_vec()).unwrap();
    let french = crate::localization::Localizer::new("fr").translate("AuditApiKeyRefused", &[]);
    let english = crate::localization::Localizer::new("en").translate("AuditApiKeyRefused", &[]);
    assert_ne!(french, english, "French falls back to English, which proves nothing here");
    assert!(csv.lines().nth(1).is_some_and(|line| line.contains(&french)), "{csv}");
    assert!(csv.contains("203.0.113.21"), "{csv}");
    // Without the mark Excel reads the system code page, and without its zone a stamp as local.
    assert!(csv.starts_with('\u{feff}'), "no byte order mark");
    let at = csv.lines().nth(1).and_then(|line| line.split(',').next()).unwrap_or_default();
    assert!(at.starts_with('"') && at.ends_with("Z\""), "{at}");
}

/// An export past its row cap says so, as the move log's does, rather than read as the whole log.
#[tokio::test]
async fn a_security_log_export_says_when_it_leaves_rows_out() {
    let app = TestApp::new().await;
    app.execute(&["INSERT INTO security_events (at, kind, outcome, subject, message)
                   WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50001)
                   SELECT datetime('2026-01-01', '+' || i || ' seconds'), 'api_key', 'refused',
                          'guess-' || i, 'AuditApiKeyRefused'
                     FROM n"])
        .await;

    let response = app.raw("/api/v1/security-log/export").await;
    assert_eq!(response.headers()["x-routarr-truncated"], "true");
    let csv = app.text("/api/v1/security-log/export").await;
    assert_eq!(csv.lines().count(), 50_001, "the header and 50 000 rows");
    assert!(!csv.contains("\"guess-1\""), "the oldest is the one left out");

    let filtered = app.raw("/api/v1/security-log/export?search=guess-50001").await;
    assert!(!filtered.headers().contains_key("x-routarr-truncated"));
}

/// The name shows the owner which application still runs on the expired key.
#[tokio::test]
async fn an_expired_application_key_is_logged_by_its_name() {
    let app = TestApp::new().await;
    let minted =
        app.post("/api/v1/applications", json!({ "name": "nightly", "expires_in_days": 1 })).await;
    let token = minted.assert_ok()["token"].as_str().unwrap().to_string();
    let (id, secret) = token.strip_prefix("rtr_").unwrap().split_once('_').unwrap();
    app.execute(&["UPDATE api_keys SET expires_at = datetime('now', '-1 minute')"]).await;

    let request = from(
        [203, 0, 113, 9],
        Request::get("/api/v1/status").header("x-api-key", &token),
        Body::empty(),
    );
    let (response, log) = logged(&app, request).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(log.contains("nightly") && log.contains(id), "{log}");
    assert!(!log.contains(secret), "the key's secret is in the log:\n{log}");

    let kept = security_log(&app, None, "?kind=application_key").await;
    let event = &kept["data"][0];
    assert_eq!(event["message"], "AuditApplicationKeyExpired", "{kept}");
    assert_eq!(event["outcome"], "refused", "{kept}");
    assert_eq!(event["params"]["name"], "nightly", "{kept}");
}
