//! Outbound notifications.
//!
//! The property that matters most is not that a message is sent but that the
//! same failure is not sent over and over. An alert that fires every
//! fifteen minutes for a week is an alert the operator mutes, which leaves them
//! worse off than with no notifications at all.

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::services::auto_apply::AutoApplyOutcome;
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

/// The address of a Discord or Slack webhook is the channel's credential:
/// whoever reads it can post there. It is sealed like an API key, and what the
/// screen gets back says whether one is stored, never what it is.
#[tokio::test]
async fn a_saved_webhook_url_is_sealed_and_never_read_back() {
    let app = TestApp::new().await;
    let url = "https://discord.com/api/webhooks/123/hook-secret-8d2e";
    app.save_setting("notification_webhook_url", url).await.assert_ok();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'notification_webhook_url'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(stored.starts_with("enc:v1:"), "stored in the clear: {stored}");

    let body = app.get("/api/v1/settings").await.assert_ok().clone();
    assert_eq!(body["notification_webhook_url"], "", "the address came back out");
    assert_eq!(body["notification_webhook_url_configured"], true);
    assert!(!body.to_string().contains("hook-secret"), "the secret is in the payload");
}

/// A database from before the address was sealed holds it in the clear. The
/// pass that seals every secret at startup covers it, and the notifications
/// keep arriving.
#[tokio::test]
async fn a_webhook_url_stored_in_the_clear_is_sealed_at_startup() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('notification_webhook_url', ?)")
        .bind(&receiver.url)
        .execute(&app.state.pool)
        .await
        .unwrap();

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'notification_webhook_url'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(stored.starts_with("enc:v1:"), "left in the clear: {stored}");

    let event = crate::services::notify::Event::InstanceRecovered { instance: "Radarr".into() };
    crate::services::notify::send(&app.state, event).await;
    assert_eq!(receiver.messages().len(), 1, "the sealed address no longer delivers");
}

#[tokio::test]
async fn an_instance_going_down_notifies_once_not_on_every_tick() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    // Port 1 on the loopback refuses the connection at once: unreachable,
    // without the timeout a black-hole address costs on every test.
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

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
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

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
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

    for _ in 0..3 {
        sync::sync_instance(&app.state, "inst-1", "schedule").await.unwrap();
    }

    assert!(receiver.messages().is_empty(), "success is not an event");
}

/// Cleared, the webhook sends nothing: the failure that reached the receiver
/// while it was set is the proof that the one after it would have.
#[tokio::test]
async fn nothing_is_sent_once_the_webhook_is_cleared() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;
    assert_eq!(receiver.messages().len(), 1, "the unreachable instance was not notified");

    app.save_setting("notification_webhook_url", "").await.assert_ok();
    sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'inst-1'")
        .bind(&arr.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sync::sync_instance(&app.state, "inst-1", "schedule").await.unwrap();

    assert_eq!(receiver.messages().len(), 1, "the recovery went to a cleared webhook");
}

#[tokio::test]
async fn the_payload_carries_the_aliases_the_usual_receivers_read() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

    let _ = sync::sync_instance(&app.state, "inst-1", "schedule").await;

    let message = receiver.messages().remove(0);
    let text = message["message"].as_str().unwrap();
    // Discord reads `content`, Apprise reads `body`, Gotify reads `message`.
    // All three carry the same sentence so one webhook fits all of them.
    assert_eq!(message["content"].as_str().unwrap(), text);
    assert_eq!(message["body"].as_str().unwrap(), text);
    assert_eq!(message["source"], "routarr");
}

/// Delivering a notification never fails the work that produced it. The
/// recovery this sync announces is sent, fails, and is logged, and the sync
/// still succeeds.
#[tokio::test]
async fn an_unreachable_webhook_does_not_break_the_sync() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.save_setting("notification_webhook_url", "http://127.0.0.1:1/hook").await.assert_ok();
    sqlx::query(
        "UPDATE instances SET last_sync_status = 'error: previously down' WHERE id = 'inst-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let report = sync::sync_instance(&app.state, "inst-1", "schedule")
        .with_subscriber(tracing::Dispatch::new(subscriber))
        .await
        .unwrap();

    assert!(report.media > 0, "the sync must succeed regardless");
    let log = capture.contents();
    assert!(
        log.contains("Notification not delivered") && log.contains("instance_recovered"),
        "the recovery was never sent, so nothing here could have failed:\n{log}"
    );
}

/// An unattended apply the Arr refused is the failure nobody is watching for,
/// and the message says how many moves failed and the first reason.
#[tokio::test]
async fn an_unattended_apply_the_arr_refuses_is_notified() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro', 8392,
                 '/movies/standard/Totoro (1988)', '/movies/standard', 1, 0)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

    let simulation = app.simulate().await;
    let outcome =
        crate::services::auto_apply::apply_simulation(&app.state, &simulation, "webhook").await;

    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied(report)) if report.failed == 1),
        "{outcome:?}"
    );
    let messages = receiver.messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["event"], "auto_apply_failed");
    assert_eq!(messages[0]["severity"], "error");
    let text = messages[0]["message"].as_str().unwrap();
    assert!(text.contains("Failures: 1") && text.contains("Totoro"), "{text}");
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

    // A typo here fails silently in the background, where nobody sees it,
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
