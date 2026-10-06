//! Outbound notifications.
//!
//! The property that matters most is not that a message is sent but that the
//! same failure is not sent over and over. An alert that fires every
//! fifteen minutes for a week is an alert the operator mutes, which leaves them
//! worse off than with no notifications at all.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};

use crate::services::auto_apply::AutoApplyOutcome;
use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// One POST the receiver took: the Standard Webhooks headers and the body as
/// sent, byte for byte, which is what a signature covers.
#[derive(Debug, Clone)]
struct Delivery {
    id: String,
    timestamp: String,
    signature: Option<String>,
    body: String,
    headers: HeaderMap,
}

/// A webhook receiver that records what Routarr posted to it.
struct Receiver {
    url: String,
    received: Arc<Mutex<Vec<Delivery>>>,
    /// Serving until it is dropped.
    _server: super::Served,
}

impl Receiver {
    async fn start() -> Self {
        Self::answering(&[]).await
    }

    /// A receiver answering these statuses in turn, then 200. A 429 asks for
    /// a second's quiet in its `Retry-After`, as a rate-limited receiver does.
    async fn answering(statuses: &[u16]) -> Self {
        Self::build(statuses, std::time::Duration::ZERO).await
    }

    /// A receiver taking `delay` to answer its first delivery with `status`,
    /// then answering 200 at once.
    async fn slow_then(status: u16, delay: std::time::Duration) -> Self {
        Self::build(&[status], delay).await
    }

    async fn build(statuses: &[u16], first_delay: std::time::Duration) -> Self {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let answers = Arc::new(Mutex::new(statuses.iter().copied().collect::<VecDeque<u16>>()));

        let app = Router::new().route(
            "/hook",
            post(move |headers: HeaderMap, body: String| {
                let sink = Arc::clone(&sink);
                let answers = Arc::clone(&answers);
                async move {
                    let header = |name: &str| {
                        headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
                    };
                    sink.lock().expect("lock").push(Delivery {
                        id: header("webhook-id").unwrap_or_default(),
                        timestamp: header("webhook-timestamp").unwrap_or_default(),
                        signature: header("webhook-signature"),
                        body,
                        headers: headers.clone(),
                    });
                    let first = sink.lock().expect("lock").len() == 1;
                    if first && !first_delay.is_zero() {
                        tokio::time::sleep(first_delay).await;
                    }
                    let status = answers.lock().expect("lock").pop_front().unwrap_or(200);
                    let mut answer = (
                        StatusCode::from_u16(status).unwrap(),
                        Json(serde_json::json!({ "ok": true })),
                    )
                        .into_response();
                    if status == 429 {
                        answer.headers_mut().insert("retry-after", "1".parse().unwrap());
                    }
                    answer
                }
            }),
        );

        let server = super::serve(app).await;

        Self { url: format!("{}/hook", server.address), received, _server: server }
    }

    fn messages(&self) -> Vec<serde_json::Value> {
        self.deliveries()
            .iter()
            .map(|delivery| serde_json::from_str(&delivery.body).expect("a JSON body"))
            .collect()
    }

    fn deliveries(&self) -> Vec<Delivery> {
        self.received.lock().expect("lock").clone()
    }

    /// The deliveries once `count` have arrived, for what is sent from a task
    /// of its own.
    async fn awaiting(&self, count: usize) -> Vec<Delivery> {
        for _ in 0..300 {
            let deliveries = self.deliveries();
            if deliveries.len() >= count {
                return deliveries;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{count} deliveries never arrived, {} did", self.deliveries().len());
    }

    /// The messages once `count` have arrived.
    async fn arrived(&self, count: usize) -> Vec<serde_json::Value> {
        self.awaiting(count)
            .await
            .iter()
            .map(|delivery| serde_json::from_str(&delivery.body).expect("a JSON body"))
            .collect()
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
    assert!(body.get("notification_webhook_url").is_none(), "the address came back out");
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
    app.store_setting("notification_webhook_url", &receiver.url).await;

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

/// The signing secrets go through the same startup pass: one in the clear
/// comes back sealed and opens to the same text, and one no key can open is
/// the only copy, left byte for byte.
#[tokio::test]
async fn the_signing_secrets_are_sealed_at_startup_and_an_unreadable_one_is_kept() {
    let app = TestApp::new().await;
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
    )
    .unwrap()
    .seal("whsec_the-only-copy")
    .unwrap();
    for secret in ["whsec_left-in-the-clear", foreign.as_str()] {
        sqlx::query("INSERT INTO webhook_secrets (secret) VALUES (?)")
            .bind(secret)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: Vec<String> =
        sqlx::query_scalar("SELECT secret FROM webhook_secrets ORDER BY rowid")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert!(stored[0].starts_with("enc:v1:"), "left in the clear: {}", stored[0]);
    assert_eq!(app.state.secrets.open(&stored[0]).unwrap(), "whsec_left-in-the-clear");
    assert_eq!(stored[1], foreign, "the only copy of a signing secret was overwritten");
}

/// A sync's notifications go out on their own task: a receiver slow to answer
/// holds neither the sync's answer nor its lock, which a second sync then
/// finds held.
#[tokio::test]
async fn a_slow_receiver_holds_neither_the_sync_nor_its_lock() {
    let app = super::waiting_on_a_silent_webhook().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    let by = crate::jobs::Attribution::manual(None);

    let failed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::services::sync::sync_instance(&app.state, "inst-1", &by),
    )
    .await
    .expect("the sync waited for its notifications");

    assert!(failed.is_err(), "the instance at port 1 answered");
    assert!(app.state.jobs.try_lock("sync:inst-1").is_some(), "the lock outlived the sync");
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
        let _ = sync::sync_instance(
            &app.state,
            "inst-1",
            &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
        )
        .await;
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

    let _ = sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await;
    // Sent on a task of its own: waited for, so the recovery arrives second.
    receiver.arrived(1).await;

    // Point it at a reachable Arr and sync again.
    sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'inst-1'")
        .bind(&arr.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await
    .unwrap();

    let messages = receiver.arrived(2).await;
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
        sync::sync_instance(
            &app.state,
            "inst-1",
            &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
        )
        .await
        .unwrap();
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
    let _ = sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await;
    assert_eq!(receiver.arrived(1).await.len(), 1, "the unreachable instance was not notified");

    app.save_setting("notification_webhook_url", "").await.assert_ok();
    sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'inst-1'")
        .bind(&arr.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await
    .unwrap();

    assert_eq!(receiver.messages().len(), 1, "the recovery went to a cleared webhook");
}

#[tokio::test]
async fn the_payload_carries_the_aliases_the_usual_receivers_read() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();

    let _ = sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await;

    let message = receiver.arrived(1).await.remove(0);
    let text = message["message"].as_str().unwrap();
    // Discord reads `content`, Apprise reads `body`, Gotify reads `message`.
    // All three carry the same sentence so one webhook fits all of them.
    assert_eq!(message["content"].as_str().unwrap(), text);
    assert_eq!(message["body"].as_str().unwrap(), text);
    assert_eq!(message["source"], "routarr");
}

/// Delivering a notification never fails the work that produced it. The
/// recovery this sync announces reaches a webhook that fails every time, and
/// the sync still succeeds.
#[tokio::test]
async fn a_failing_webhook_does_not_break_the_sync() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let receiver = Receiver::answering(&[500, 500, 500, 500]).await;
    listening(&app, &receiver).await;
    sqlx::query(
        "UPDATE instances SET last_sync_status = 'error: previously down' WHERE id = 'inst-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE),
    )
    .await
    .unwrap();

    assert!(report.media > 0, "the sync must succeed regardless");
    // The positive control: the recovery was sent, and failed.
    assert_eq!(receiver.arrived(1).await[0]["event"], "instance_recovered");
}

/// Nothing answering at the address at all is a failure like any other.
#[tokio::test]
async fn an_unreachable_webhook_does_not_fail_the_send() {
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", "http://127.0.0.1:1/hook").await.assert_ok();
    tokio::time::timeout(std::time::Duration::from_secs(5), notify::send(&app.state, recovered()))
        .await
        .expect("an unreachable webhook held the caller");
}

/// A film not yet downloaded, routed to `anime`, on a Radarr that refuses the
/// move, with automatic application armed.
async fn an_unattended_move(app: &TestApp, arr: &FakeArr) -> String {
    app.seed_one_film_to_move(arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.simulate().await
}

/// An unattended apply the Arr refused is the failure nobody is watching for,
/// and the message says how many moves failed and the first reason.
#[tokio::test]
async fn an_unattended_apply_the_arr_refuses_is_notified() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    let simulation = an_unattended_move(&app, &arr).await;

    let outcome =
        crate::services::auto_apply::apply_simulation(&app.state, &simulation, "webhook").await;

    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied(report)) if report.failed == 1),
        "{outcome:?}"
    );
    let messages = receiver.arrived(1).await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["event"], "auto_apply_failed");
    assert_eq!(messages[0]["severity"], "error");
    let text = messages[0]["message"].as_str().unwrap();
    assert!(text.contains("Failures: 1") && text.contains("Totoro"), "{text}");
}

/// An unattended apply that moved everything raises no alarm. The instance
/// found unreachable after it is the barrier: its message, sent later, is the
/// first and only one to arrive.
#[tokio::test]
async fn an_unattended_apply_that_moved_everything_raises_no_alarm() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    let simulation = an_unattended_move(&app, &arr).await;

    let outcome =
        crate::services::auto_apply::apply_simulation(&app.state, &simulation, "webhook").await;
    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied(report)) if report.failed == 0),
        "{outcome:?}"
    );
    app.execute(&["UPDATE instances SET base_url = 'http://127.0.0.1:1'"]).await;
    let by = crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE);
    let _ = sync::sync_instance(&app.state, "inst-1", &by).await;

    let messages = receiver.arrived(1).await;
    let events: Vec<&str> = messages.iter().filter_map(|m| m["event"].as_str()).collect();
    assert_eq!(events, ["instance_unreachable"]);
}

/// And told on a task of its own: the webhook waits for the automatic apply,
/// and a receiver slow to answer would hold that delivery and the ones queued
/// behind it.
#[tokio::test]
async fn a_slow_receiver_does_not_hold_the_unattended_apply() {
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = super::waiting_on_a_silent_webhook().await;
    let simulation = an_unattended_move(&app, &arr).await;

    let applied = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::services::auto_apply::apply_simulation(&app.state, &simulation, "webhook"),
    )
    .await;

    assert!(applied.is_ok(), "the unattended apply waited for its notification");
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

// ------------------------------------------------------ signed deliveries

use crate::services::notify::{self, Event};

async fn listening(app: &TestApp, receiver: &Receiver) {
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
}

/// The case the Standard Webhooks libraries publish for their own signer:
/// secret, message id, timestamp, body, and the signature every receiver built
/// on them expects. The tests below check a delivery against `notify::sign`,
/// which only proves the signer agrees with itself.
const REFERENCE: [&str; 5] = [
    "whsec_C2FVsBQIhrscChlQIMV+b5sSYspob7oD",
    "msg_27UH4WbU6Z5A5EzD8u03UvzRbpk",
    "1649367553",
    r#"{"email":"test@example.com","username":"test_user"}"#,
    "v1,tZ1I4/hDygAJgO5TYxiSd6Sd0kDW6hPenDe+bTa3Kkw=",
];

#[test]
fn a_signature_is_the_one_standard_webhooks_receivers_expect() {
    let [secret, id, timestamp, body, expected] = REFERENCE;
    let signing = crate::crypto::signing_key(secret).expect("the reference secret is valid");
    assert_eq!(notify::sign(&[signing], id, timestamp, body), expected);
}

/// Whether `signature` holds a `v1` entry made with `secret` over the rest.
fn signed_with(secret: &str, delivery: &Delivery) -> bool {
    let key = crate::crypto::signing_key(secret).expect("a signing secret");
    let expected = notify::sign(&[key], &delivery.id, &delivery.timestamp, &delivery.body);
    delivery.signature.as_deref().unwrap_or_default().split(' ').any(|entry| entry == expected)
}

fn recovered() -> Event {
    Event::InstanceRecovered { instance: "Radarr".into() }
}

/// The secret is shown once, and what it signs checks against it: the id,
/// the timestamp and the body, as Standard Webhooks writes them.
#[tokio::test]
async fn a_notification_is_signed_with_the_secret_shown_once() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    assert_eq!(app.get("/api/v1/notifications/webhook-secret").await.json["signed"], false);

    let minted = app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await;
    let secret = minted.assert_ok()["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("whsec_"), "{secret}");
    let status = app.get("/api/v1/notifications/webhook-secret").await;
    assert_eq!(status.assert_ok()["signed"], true);
    assert!(!status.json.to_string().contains(&secret), "the secret was read back");
    let stored: String = sqlx::query_scalar("SELECT secret FROM webhook_secrets")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(!stored.contains(&secret), "the secret is stored in the clear");

    notify::send(&app.state, recovered()).await;
    let delivery = receiver.deliveries().pop().expect("a delivery");
    assert!(delivery.id.starts_with("msg_"), "{delivery:?}");
    assert!(signed_with(&secret, &delivery), "{delivery:?}");
    let body: serde_json::Value = serde_json::from_str(&delivery.body).unwrap();
    assert_eq!(body["event"], "instance_recovered");
    assert_eq!(body["data"]["instance"], "Radarr");

    let removed = app.delete("/api/v1/notifications/webhook-secret").await;
    assert_eq!(removed.status, axum::http::StatusCode::NO_CONTENT);
    notify::send(&app.state, recovered()).await;
    assert_eq!(receiver.deliveries().pop().unwrap().signature, None);
}

/// A new secret signs beside the one it replaces for a day, so the receiver
/// can be given it without missing a message, and alone after that.
#[tokio::test]
async fn a_replaced_secret_keeps_signing_for_a_day() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    let mint = || app.post("/api/v1/notifications/webhook-secret", serde_json::json!({}));
    let old = mint().await.json["secret"].as_str().unwrap().to_string();
    let new = mint().await.json["secret"].as_str().unwrap().to_string();

    notify::send(&app.state, recovered()).await;
    let delivery = receiver.deliveries().pop().unwrap();
    assert!(signed_with(&old, &delivery) && signed_with(&new, &delivery), "{delivery:?}");

    sqlx::query("UPDATE webhook_secrets SET created_at = datetime('now', '-2 days')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    notify::send(&app.state, recovered()).await;
    let delivery = receiver.deliveries().pop().unwrap();
    assert!(signed_with(&new, &delivery), "{delivery:?}");
    assert!(!signed_with(&old, &delivery), "a secret replaced two days ago still signs");
}

/// A receiver that fails is tried again under the same id. One that refuses
/// the message is not: it will refuse it again.
#[tokio::test]
async fn a_failed_delivery_is_tried_again_and_a_refused_one_is_not() {
    let app = TestApp::new().await;
    let failing = Receiver::answering(&[503, 500]).await;
    listening(&app, &failing).await;
    notify::send(&app.state, recovered()).await;
    let deliveries = failing.awaiting(3).await;
    assert!(deliveries.iter().all(|d| d.id == deliveries[0].id), "{deliveries:?}");

    let refusing = Receiver::answering(&[400]).await;
    listening(&app, &refusing).await;
    notify::send(&app.state, recovered()).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(refusing.deliveries().len(), 1, "a refused message was sent again");
}

/// A secret no key opens, as a database restored beside another master key
/// leaves it, signs nothing. Sent unsigned, a message would pass any receiver
/// that checks a signature only when one is there, so nothing is sent, and
/// the status says the secret cannot be read rather than that it signs.
#[tokio::test]
async fn a_secret_that_cannot_be_opened_sends_nothing_and_says_so() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
    )
    .unwrap()
    .seal("whsec_QUJDREVGR0hJSktMTU5PUFFSU1RVVldY")
    .unwrap();
    sqlx::query("INSERT INTO webhook_secrets (secret) VALUES (?)")
        .bind(&foreign)
        .execute(&app.state.pool)
        .await
        .unwrap();

    notify::send(&app.state, recovered()).await;
    assert!(receiver.deliveries().is_empty(), "an unsigned message went out");
    let status = app.get("/api/v1/notifications/webhook-secret").await;
    assert_eq!(status.assert_ok()["readable"], false, "{:?}", status.json);

    // A secret made afterwards replaces it, and the messages go out signed.
    let minted = app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await;
    let secret = minted.assert_ok()["secret"].as_str().unwrap().to_string();
    notify::send(&app.state, recovered()).await;
    assert!(signed_with(&secret, &receiver.awaiting(1).await[0]));
    assert_eq!(app.get("/api/v1/notifications/webhook-secret").await.json["readable"], true);
}

/// A retry is signed with the secrets of its own time and sent to the address
/// of its own time: a secret replaced because it leaked stops being the only
/// one, and a cleared address receives nothing more.
#[tokio::test]
async fn a_retry_follows_the_secret_and_the_address_of_its_own_time() {
    let app = TestApp::new().await;
    // A second's quiet asked before each retry, the time to change things.
    let receiver = Receiver::answering(&[429]).await;
    listening(&app, &receiver).await;
    let mint = || app.post("/api/v1/notifications/webhook-secret", serde_json::json!({}));
    mint().await.assert_ok();

    notify::send_later(&app.state, recovered());
    receiver.awaiting(1).await;
    let new = mint().await.json["secret"].as_str().unwrap().to_string();
    let retried = receiver.awaiting(2).await;
    assert!(signed_with(&new, &retried[1]), "the retry ignored the new secret: {retried:?}");

    let cleared = Receiver::answering(&[429]).await;
    listening(&app, &cleared).await;
    notify::send_later(&app.state, recovered());
    cleared.awaiting(1).await;
    app.save_setting("notification_webhook_url", "").await.assert_ok();
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert_eq!(cleared.deliveries().len(), 1, "a cleared address was tried again");
}

/// What finished well reaches the webhook only when it asked for it.
#[tokio::test]
async fn a_completion_is_sent_only_to_a_webhook_that_asked_for_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await.assert_ok();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(receiver.deliveries().is_empty(), "a completion nobody asked for was sent");

    app.save_setting("notify_simulation_completed", "true").await.assert_ok();
    let simulated = app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await;
    let simulation = simulated.assert_ok()["simulation_id"].clone();
    receiver.awaiting(1).await;
    let message = &receiver.messages()[0];
    assert_eq!(message["event"], "simulation_completed");
    assert_eq!(message["data"]["simulation_id"], simulation);
    assert_eq!(message["data"]["moves"], 1);
}

/// An apply and a revert each announce themselves under their own name, with
/// their counts, and only to a webhook that asked for completions.
#[tokio::test]
async fn an_apply_and_a_revert_are_announced_apart_once_asked_for() {
    use crate::services::executor::{self, Confirmed};

    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    let by = crate::jobs::Attribution::manual(None);
    let pending = || async {
        app.simulate().await;
        let id: String = sqlx::query_scalar("SELECT id FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        vec![id]
    };

    let quiet = pending().await;
    executor::apply_decisions(&app.state, &quiet, false, &Confirmed::all(), &by).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(receiver.deliveries().is_empty(), "a completion nobody asked for was sent");

    app.save_setting("notify_moves_completed", "true").await.assert_ok();
    executor::revert_decisions(&app.state, &quiet, false, &Confirmed::all(), &by).await.unwrap();
    let reverted = &receiver.arrived(1).await[0];
    assert_eq!(reverted["event"], "revert_completed");
    let done = serde_json::json!({ "applied": 1, "failed": 0, "skipped": 0, "moving": 0 });
    assert_eq!(reverted["data"], done);

    let again = pending().await;
    executor::apply_decisions(&app.state, &again, false, &Confirmed::all(), &by).await.unwrap();
    let applied = &receiver.arrived(2).await[1];
    assert_eq!(applied["event"], "apply_completed");
    assert_eq!(applied["data"], done);
}

/// A failed sync is told every time once asked for, beside the one message
/// an instance going unreachable always earns.
#[tokio::test]
async fn a_failed_sync_is_told_every_time_once_asked_for() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    listening(&app, &receiver).await;
    let by = crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE);

    let _ = sync::sync_instance(&app.state, "inst-1", &by).await;
    assert_eq!(receiver.arrived(1).await[0]["event"], "instance_unreachable");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(receiver.deliveries().len(), 1, "a failed sync nobody asked for was sent");

    app.save_setting("notify_sync_failed", "true").await.assert_ok();
    let _ = sync::sync_instance(&app.state, "inst-1", &by).await;
    let failed = &receiver.arrived(2).await[1];
    assert_eq!(failed["event"], "sync_failed");
    let name = app.state.instance("inst-1").await.unwrap().name;
    assert_eq!(failed["data"], serde_json::json!({ "instance_id": "inst-1", "instance": name }));
}

/// A Standard Webhooks receiver refuses a `webhook-timestamp` more than a few
/// minutes from its own clock, in seconds. Each attempt is stamped when it is
/// sent: a retry carrying the first attempt's stamp is refused once the
/// retries reach minutes. The first answer takes over a second, so the retry
/// falls in another second.
#[tokio::test]
async fn each_attempt_is_stamped_in_seconds_when_it_is_sent() {
    let app = TestApp::new().await.with_http_budget(std::time::Duration::from_secs(5));
    let receiver = Receiver::slow_then(503, std::time::Duration::from_millis(1100)).await;
    listening(&app, &receiver).await;

    notify::send(&app.state, recovered()).await;

    let deliveries = receiver.awaiting(2).await;
    let stamps: Vec<i64> =
        deliveries.iter().map(|d| d.timestamp.parse().expect(&d.timestamp)).collect();
    let now = chrono::Utc::now().timestamp();
    assert!(stamps.iter().all(|stamp| (now - stamp).abs() <= 5), "{stamps:?} against {now}");
    assert!(stamps[1] > stamps[0], "the retry carried the first attempt's stamp: {stamps:?}");
}

/// A refused delivery is retried before anything sent after it: otherwise an
/// `instance_unreachable` retried ten seconds on lands after the
/// `instance_recovered` that followed it, and the channel ends on the wrong
/// state.
#[tokio::test]
async fn a_retried_notification_never_lands_after_a_newer_one() {
    let receiver = Receiver::answering(&[503]).await;
    let app = TestApp::new().await;
    listening(&app, &receiver).await;
    let unreachable =
        Event::InstanceUnreachable { instance: "Radarr".into(), error: "refused".into() };

    notify::send_later(&app.state, unreachable);
    notify::send_later(&app.state, recovered());

    let events: Vec<String> = receiver
        .arrived(3)
        .await
        .iter()
        .map(|message| message["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["instance_unreachable", "instance_unreachable", "instance_recovered"]);
}

/// A delivery refused for good is tried four times, once and then after each
/// of the three waits, and no more. A 429 is retried after the quiet its
/// `Retry-After` asks for. Every attempt is signed over its own timestamp.
#[tokio::test]
async fn a_refused_delivery_is_tried_four_times_each_signed_for_itself() {
    let app = TestApp::new().await;
    let refusing = Receiver::answering(&[500, 500, 500, 500, 500]).await;
    listening(&app, &refusing).await;
    let minted = app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await;
    let secret = minted.assert_ok()["secret"].as_str().unwrap().to_string();

    notify::send(&app.state, recovered()).await;

    let tried = refusing.deliveries();
    assert_eq!(tried.len(), 4, "{tried:?}");
    assert!(tried.iter().all(|delivery| signed_with(&secret, delivery)), "{tried:?}");

    let throttling = Receiver::answering(&[429]).await;
    listening(&app, &throttling).await;
    let started = std::time::Instant::now();
    notify::send(&app.state, recovered()).await;
    assert_eq!(throttling.deliveries().len(), 2, "a 429 was not retried");
    assert!(started.elapsed() >= std::time::Duration::from_secs(1), "{:?}", started.elapsed());
}

/// Deliveries wait their turn up to a bound: past thirty-two pending, an event
/// is dropped rather than queued without end, and once the queue has drained
/// the places are given back.
#[tokio::test]
async fn deliveries_queue_up_to_a_bound_and_give_their_places_back() {
    let app = TestApp::new().await.with_http_budget(std::time::Duration::from_secs(5));
    // The first answer takes a while, so the ones after it queue behind it.
    let receiver = Receiver::slow_then(200, std::time::Duration::from_millis(500)).await;
    listening(&app, &receiver).await;

    notify::send_later(&app.state, recovered());
    receiver.awaiting(1).await;
    for _ in 0..33 {
        notify::send_later(&app.state, recovered());
    }
    receiver.awaiting(32).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(receiver.deliveries().len(), 32, "the queue was not bounded at 32");

    notify::send(&app.state, recovered()).await;
    assert_eq!(receiver.deliveries().len(), 33, "a place was never given back");
}

/// The simulation that follows a scheduled sync is announced like one asked
/// for, and a preview, which stores nothing, is not announced at all.
#[tokio::test]
async fn a_scheduled_simulation_is_announced_and_a_preview_is_not() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.store_setting("backup_enabled", "false").await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    app.save_setting("notify_simulation_completed", "true").await.assert_ok();

    app.post("/api/v1/simulate", serde_json::json!({ "persist": false })).await.assert_ok();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(receiver.deliveries().is_empty(), "a preview was announced");

    crate::jobs::scheduler::tick(
        &app.state,
        &mut std::collections::HashMap::new(),
        &mut None,
        &mut None,
    )
    .await
    .unwrap();
    let chain = app.state.post_sync.lock().await.take().expect("the sync was not followed");
    chain.await.unwrap();

    let announced = &receiver.arrived(1).await[0];
    assert_eq!(announced["event"], "simulation_completed", "{announced}");
}

/// ntfy shows a JSON body posted to a topic as it came, unless the address
/// turns on its templates, which read `title` and `message` from the body.
/// The address the help gives for ntfy is saved and posted to as written.
#[tokio::test]
async fn an_ntfy_address_with_its_templates_is_saved_and_posted_to() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    let ntfy =
        format!("{}?template=yes&title={{{{.title}}}}&message={{{{.message}}}}", receiver.url);
    app.save_setting("notification_webhook_url", &ntfy).await.assert_ok();

    notify::send(&app.state, recovered()).await;

    let delivered = &receiver.arrived(1).await[0];
    assert_eq!(delivered["title"], "Routarr");
    assert!(delivered["message"].as_str().is_some_and(|m| !m.is_empty()), "{delivered}");
}

// ------------------------------------------------------ formats and tests

/// The format is read at each delivery, and the signature covers the body
/// as sent, whatever its shape.
#[tokio::test]
async fn the_format_chosen_in_settings_shapes_what_is_posted_and_is_signed() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    let minted = app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await;
    let secret = minted.assert_ok()["secret"].as_str().unwrap().to_string();

    app.save_setting("notification_format", "ntfy").await.assert_ok();
    notify::send(&app.state, recovered()).await;
    let ntfy = receiver.deliveries().pop().expect("a delivery");
    let header = |name: &str| ntfy.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(header("content-type").starts_with("text/plain"), "{ntfy:?}");
    assert_eq!(header("title"), "Instance reachable again");
    assert!(ntfy.body.contains("'Radarr'"), "{ntfy:?}");
    assert!(signed_with(&secret, &ntfy), "{ntfy:?}");

    app.save_setting("notification_format", "gotify").await.assert_ok();
    notify::send(&app.state, recovered()).await;
    let gotify: serde_json::Value =
        serde_json::from_str(&receiver.deliveries().pop().unwrap().body).unwrap();
    assert_eq!(gotify["priority"], 2, "{gotify}");
}

#[tokio::test]
async fn a_format_routarr_does_not_write_is_refused() {
    let app = TestApp::new().await;
    let refused = app.save_setting("notification_format", "telegram").await;
    assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
    app.save_setting("notification_format", "auto").await.assert_ok();
}

/// The answer comes once the receiver has taken the message.
#[tokio::test]
async fn a_test_notification_has_arrived_when_the_answer_comes() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;

    let sent = app.post("/api/v1/notifications/test", serde_json::json!({})).await;
    assert_eq!(sent.status, axum::http::StatusCode::NO_CONTENT, "{:?}", sent.json);
    assert_eq!(receiver.messages().pop().expect("a delivery")["event"], "test");
}

/// The screen shows why the test did not arrive, in the reader's language:
/// no address, the receiver's refusal, or no answer at all.
#[tokio::test]
async fn a_test_notification_says_why_it_did_not_arrive() {
    let app = TestApp::new().await;
    app.save_setting("ui_language", "fr").await.assert_ok();
    let test = || app.post("/api/v1/notifications/test", serde_json::json!({}));

    let nowhere = test().await;
    assert_eq!(nowhere.status, axum::http::StatusCode::BAD_REQUEST);
    assert!(nowhere.json["message"].as_str().unwrap().contains("adresse"), "{:?}", nowhere.json);

    let receiver = Receiver::answering(&[404]).await;
    listening(&app, &receiver).await;
    let refused = test().await;
    assert_eq!(refused.status, axum::http::StatusCode::BAD_GATEWAY);
    assert!(refused.json["message"].as_str().unwrap().contains("404"), "{:?}", refused.json);
    assert_eq!(receiver.deliveries().len(), 1, "a test is never tried again");

    app.save_setting("notification_webhook_url", "http://127.0.0.1:1/hook").await.assert_ok();
    let unanswered = test().await;
    assert_eq!(unanswered.status, axum::http::StatusCode::BAD_GATEWAY);
    assert!(!unanswered.json["message"].as_str().unwrap().contains("127.0.0.1"));
}
