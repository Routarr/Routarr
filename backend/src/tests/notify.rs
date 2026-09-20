//! Outbound notifications.
//!
//! The property that matters most is not that a message is sent — it is that
//! the same failure is not sent over and over. An alert that fires every
//! fifteen minutes for a week is an alert the operator mutes, which leaves them
//! worse off than with no notifications at all.

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// A webhook receiver that records what Routarr posted to it.
struct Receiver {
    url: String,
    received: Arc<Mutex<Vec<serde_json::Value>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Receiver {
    async fn start() -> Self {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);

        let app = Router::new().route(
            "/hook",
            post(move |Json(body): Json<serde_json::Value>| {
                let sink = Arc::clone(&sink);
                async move {
                    sink.lock().expect("lock").push(body);
                    Json(serde_json::json!({ "ok": true }))
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind receiver");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self { url: format!("http://{addr}/hook"), received, shutdown: Some(tx) }
    }

    fn messages(&self) -> Vec<serde_json::Value> {
        self.received.lock().expect("lock").clone()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn set(app: &TestApp, key: &str, value: &str) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn an_instance_going_down_notifies_once_not_on_every_tick() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    // Port 1 on the loopback refuses the connection at once — unreachable,
    // without the timeout a black-hole address costs on every test.
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    set(&app, "notification_webhook_url", &receiver.url).await;

    // Three consecutive failed syncs, as the scheduler would produce.
    for _ in 0..3 {
        let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;
    }

    let messages = receiver.messages();
    assert_eq!(messages.len(), 1, "only the transition is worth a message, got {messages:?}");
    assert_eq!(messages[0]["event"], "instance_unreachable");
    assert_eq!(messages[0]["severity"], "error");
    assert!(
        messages[0]["message"].as_str().unwrap().contains("Fake radarr"),
        "the message must name the instance: {:?}",
        messages[0]["message"]
    );
}

#[tokio::test]
async fn coming_back_closes_the_loop() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    set(&app, "notification_webhook_url", &receiver.url).await;

    let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;

    // Point it at a reachable Arr and sync again.
    sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'inst-1'")
        .bind(&arr.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sync::sync_instance(&app.state, "inst-1", "schedule").await.unwrap();

    let messages = receiver.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["event"], "instance_recovered");
    assert_eq!(messages[1]["severity"], "info", "recovery is news, not an alarm");
}

#[tokio::test]
async fn a_healthy_instance_is_never_worth_a_message() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    set(&app, "notification_webhook_url", &receiver.url).await;

    for _ in 0..3 {
        sync::sync_instance(&app.state, "inst-1", "schedule").await.unwrap();
    }

    assert!(receiver.messages().is_empty(), "success is not an event");
}

#[tokio::test]
async fn nothing_is_sent_when_no_webhook_is_configured() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    // Deliberately not configured — the default.

    let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;

    assert!(receiver.messages().is_empty());
}

#[tokio::test]
async fn the_payload_carries_the_aliases_the_usual_receivers_read() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    set(&app, "notification_webhook_url", &receiver.url).await;

    let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;

    let message = receiver.messages().remove(0);
    let text = message["message"].as_str().unwrap();
    // Discord reads `content`, Apprise reads `body`, Gotify reads `message`.
    // All three carry the same sentence so one webhook fits all of them.
    assert_eq!(message["content"].as_str().unwrap(), text);
    assert_eq!(message["body"].as_str().unwrap(), text);
    assert_eq!(message["source"], "routarr");
}

#[tokio::test]
async fn an_unreachable_webhook_does_not_break_the_sync() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    // Points nowhere: delivering a notification must never be able to fail the
    // work that produced it.
    set(&app, "notification_webhook_url", "http://127.0.0.1:1/hook").await;
    sqlx::query(
        "UPDATE instances SET last_sync_status = 'error: previously down' WHERE id = 'inst-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = sync::sync_instance(&app.state, "inst-1", "schedule").await.unwrap();

    assert!(report.media > 0, "the sync must succeed regardless");
}

#[tokio::test]
async fn a_url_that_is_not_a_url_is_refused_at_the_settings_boundary() {
    let app = TestApp::new().await;

    let refused = app
        .put(
            "/api/v1/settings",
            serde_json::json!({ "settings": { "notification_webhook_url": "discord.com/hook" } }),
        )
        .await;

    // A typo here fails silently in the background, where nobody sees it —
    // which is exactly what this feature exists to prevent.
    assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);

    let accepted = app
        .put(
            "/api/v1/settings",
            serde_json::json!({ "settings": { "notification_webhook_url": "" } }),
        )
        .await;
    assert!(accepted.status.is_success(), "empty means 'off', not 'invalid'");
}
