//! Outbound notifications. What matters most is that one failure is not sent over and over:
//! an alert firing every fifteen minutes for a week is one the operator mutes.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};

use crate::localization::Localizer;
use crate::services::auto_apply::AutoApplyOutcome;
use crate::services::notify::{self, Event};
use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// One POST the receiver took: the Standard Webhooks headers and the body as
/// sent, byte for byte, which is what a signature covers.
#[derive(Debug, Clone)]
pub(crate) struct Delivery {
    id: String,
    timestamp: String,
    signature: Option<String>,
    pub(crate) body: String,
    headers: HeaderMap,
}

pub(crate) struct Receiver {
    pub(crate) url: String,
    received: Arc<Mutex<Vec<Delivery>>>,
    /// Serving until it is dropped.
    _server: super::Served,
}

impl Receiver {
    pub(crate) async fn start() -> Self {
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

    pub(crate) fn deliveries(&self) -> Vec<Delivery> {
        self.received.lock().expect("lock").clone()
    }

    /// The deliveries once `count` have arrived, for what is sent from a task
    /// of its own.
    pub(crate) async fn awaiting(&self, count: usize) -> Vec<Delivery> {
        for _ in 0..300 {
            let deliveries = self.deliveries();
            if deliveries.len() >= count {
                return deliveries;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{count} deliveries never arrived, {} did", self.deliveries().len());
    }

    async fn arrived(&self, count: usize) -> Vec<serde_json::Value> {
        self.awaiting(count)
            .await
            .iter()
            .map(|delivery| serde_json::from_str(&delivery.body).expect("a JSON body"))
            .collect()
    }
}

#[tokio::test]
async fn a_webhook_url_sealed_at_startup_still_delivers() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.store_setting("notification_webhook_url", &receiver.url).await;

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let event = crate::services::notify::Event::InstanceRecovered {
        instance_id: "inst-1".into(),
        instance: "Radarr".into(),
    };
    crate::services::notify::send(&app.state, event).await;
    assert_eq!(receiver.messages().len(), 1, "the sealed address no longer delivers");
}

/// One no key can open is the only copy, left byte for byte.
#[tokio::test]
async fn the_signing_secrets_are_sealed_at_startup_and_an_unreadable_one_is_kept() {
    let app = TestApp::new().await;
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
        None,
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
    assert!(stored[0].starts_with("enc:v2:"), "left in the clear: {}", stored[0]);
    assert_eq!(app.state.secrets.open(&stored[0]).unwrap(), "whsec_left-in-the-clear");
    assert_eq!(stored[1], foreign, "the only copy of a signing secret was overwritten");
}

/// A slow receiver would otherwise hold the sync's answer, and the second sync find its lock held.
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
    // Port 1 on the loopback refuses at once, without a black-hole address's timeout.
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

    settled(&app).await;
    let messages = receiver.messages();
    assert_eq!(messages.len(), 1, "only the transition is worth a message, got {messages:?}");
    assert_eq!(messages[0]["event"], "instance_unreachable");
    assert_eq!(messages[0]["severity"], "error");
    let text = messages[0]["message"].as_str().unwrap();
    assert!(text.contains("Fake radarr"), "the message must name the instance: {text}");
    assert!(!text.contains("\"key\""), "the cause is said, not stored: {text}");
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

    settled(&app).await;
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

    settled(&app).await;
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
    // Discord reads `content`, Apprise `body`, Gotify `message`: one sentence fits all three.
    assert_eq!(message["content"].as_str().unwrap(), text);
    assert_eq!(message["body"].as_str().unwrap(), text);
    assert_eq!(message["source"], "routarr");
}

/// The recovery this sync announces reaches a webhook that fails every time.
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

/// An address that closes every connection unanswered is a failure like any other: tried
/// four times, and the caller is not held past the retries.
#[tokio::test]
async fn a_webhook_answering_nothing_is_tried_as_a_failure_is() {
    let (closing, address) = super::listen().await;
    let tried = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&tried);
    tokio::spawn(async move {
        while let Ok((connection, _)) = closing.accept().await {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(connection);
        }
    });
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", &format!("{address}/hook")).await.assert_ok();
    tokio::time::timeout(std::time::Duration::from_secs(5), notify::send(&app.state, recovered()))
        .await
        .expect("a webhook answering nothing held the caller");
    assert_eq!(tried.load(std::sync::atomic::Ordering::SeqCst), 4, "tried as often as a failure");
}

/// A film not yet downloaded on `arr`, routed to `anime`, with automatic application armed.
async fn an_unattended_move(app: &TestApp, arr: &FakeArr) -> String {
    app.seed_one_film_to_move(arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.simulate().await
}

/// The failure nobody is watching for: the message gives how many moves failed and the first
/// reason, in the interface's language.
#[tokio::test]
async fn an_unattended_apply_the_arr_refuses_is_notified() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    app.store_setting("ui_language", "fr").await;
    let simulation = an_unattended_move(&app, &arr).await;

    let outcome = crate::services::auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await;

    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied { report, .. }) if report.failed == 1),
        "{outcome:?}"
    );
    settled(&app).await;
    let messages = receiver.messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["event"], "auto_apply_failed");
    assert_eq!(messages[0]["severity"], "error");
    let refused: String =
        sqlx::query_scalar("SELECT error_message FROM decisions WHERE status = 'failed'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let error = crate::localization::said(&refused, &Localizer::new("fr"));
    let in_language = |language: &str| {
        let values = [("failed", "1"), ("applied", "0"), ("title", "Totoro"), ("error", &error)];
        Localizer::new(language).translate("NotificationMessageAutoApplyFailed", &values)
    };
    assert_eq!(messages[0]["message"], in_language("fr"));
    assert_ne!(in_language("fr"), in_language("en"), "said in English around a French error");
}

/// More moves than one unattended run may make leaves every one of them to a
/// person, which nobody sees unless told: said once as the holding starts,
/// warned of on `/status` while it lasts, and cleared by a pass under the cap.
#[tokio::test]
async fn a_sweep_held_over_the_cap_is_announced_once_and_warned_of() {
    let receiver = Receiver::start().await;
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    app.seed_one_film_to_move(&arr, false).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Akira', 8392, '/movies/standard/Akira',
                 '/movies/standard', 1, 0)",
    ])
    .await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.store_setting("batch_limit", "1").await;
    let simulation = app.simulate().await;
    let by = crate::jobs::Attribution::unattended("schedule");
    let warned = |body: &serde_json::Value| {
        body["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "auto_apply_held")
    };

    for _ in 0..2 {
        let outcome =
            crate::services::auto_apply::apply_simulation(&app.state, &simulation, &by).await;
        assert!(matches!(outcome, Ok(AutoApplyOutcome::OverCap(_))), "{outcome:?}");
    }
    let held = app.get("/api/v1/status").await;
    assert!(warned(held.assert_ok()), "{}", held.json);

    app.store_setting("batch_limit", "2").await;
    crate::services::auto_apply::apply_simulation(&app.state, &simulation, &by).await.unwrap();
    let cleared = app.get("/api/v1/status").await;
    assert!(!warned(cleared.assert_ok()), "{}", cleared.json);
    let messages = receiver.arrived(1).await;
    settled(&app).await;
    let held: Vec<_> = receiver
        .messages()
        .into_iter()
        .filter(|message| message["event"] == "auto_apply_held")
        .collect();
    assert_eq!(held.len(), 1, "{messages:?}");
    assert_eq!(
        held[0]["data"],
        serde_json::json!({
            "instance_id": "inst-1", "instance": "Fake radarr", "candidates": 2, "cap": 1,
        })
    );
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

    let outcome = crate::services::auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await;
    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied { report, .. }) if report.failed == 0),
        "{outcome:?}"
    );
    app.execute(&["UPDATE instances SET base_url = 'http://127.0.0.1:1'"]).await;
    let by = crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE);
    let _ = sync::sync_instance(&app.state, "inst-1", &by).await;

    settled(&app).await;
    let messages = receiver.messages();
    let events: Vec<&str> = messages.iter().filter_map(|m| m["event"].as_str()).collect();
    assert_eq!(events, ["instance_unreachable"]);
}

/// A sync that fails on Routarr's side, a key it cannot open, is no Arr out
/// of reach: announced as one, the operator would go and check an Arr that
/// answers. The failed task says what it is.
#[tokio::test]
async fn an_internal_sync_failure_is_not_announced_as_an_unreachable_instance() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
    app.save_setting("notify_sync_failed", "true").await.assert_ok();
    app.execute(&["UPDATE instances SET api_key = 'enc:v1:AAAA' WHERE id = 'inst-1'"]).await;

    let by = crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_SCHEDULE);
    assert!(sync::sync_instance(&app.state, "inst-1", &by).await.is_err());

    let messages = receiver.arrived(1).await;
    let events: Vec<&str> = messages.iter().filter_map(|m| m["event"].as_str()).collect();
    assert_eq!(events, ["sync_failed"], "{messages:?}");
}

/// An unattended apply's failure is told on a task of its own: the webhook waits for the
/// automatic apply, and a receiver slow to answer would hold that delivery and the ones queued
/// behind it.
#[tokio::test]
async fn a_slow_receiver_does_not_hold_the_unattended_apply() {
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = super::waiting_on_a_silent_webhook().await;
    let simulation = an_unattended_move(&app, &arr).await;

    let applied = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::services::auto_apply::apply_simulation(
            &app.state,
            &simulation,
            &crate::jobs::Attribution::unattended("webhook"),
        ),
    )
    .await;

    assert!(applied.is_ok(), "the unattended apply waited for its notification");
}

#[tokio::test]
async fn a_url_that_is_not_a_url_is_refused_at_the_settings_boundary() {
    let app = TestApp::new().await;

    // A typo fails unseen in the background. No host, or a space in it, is as bad as no scheme.
    for typed in
        ["discord.com/hook", "http://", "https://", "https://discord .com/api/webhooks/1/abc"]
    {
        let refused = app
            .put(
                "/api/v1/settings",
                serde_json::json!({ "settings": { "notification_webhook_url": typed } }),
            )
            .await;
        assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST, "{typed}");
    }

    let accepted = app
        .put(
            "/api/v1/settings",
            serde_json::json!({ "settings": { "notification_webhook_url": "" } }),
        )
        .await;
    assert!(accepted.status.is_success(), "empty means 'off', not 'invalid'");
}

async fn listening(app: &TestApp, receiver: &Receiver) {
    app.save_setting("notification_webhook_url", &receiver.url).await.assert_ok();
}

/// Wait for every delivery `notify::send_later` started to return, retries included, so what
/// the receiver holds is all that was sent.
async fn settled(app: &TestApp) {
    super::until(|| async { app.state.notifications.in_flight() == 0 }).await;
    assert_eq!(app.state.notifications.in_flight(), 0, "a delivery still running");
}

/// The Standard Webhooks libraries' own case: secret, message id, timestamp, body and the
/// signature their receivers expect. The tests below check against `notify::sign`, which only
/// proves the signer agrees with itself.
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

/// Whether the delivery's signature holds a `v1` entry made with `secret` over the rest.
fn signed_with(secret: &str, delivery: &Delivery) -> bool {
    let key = crate::crypto::signing_key(secret).expect("a signing secret");
    let expected = notify::sign(&[key], &delivery.id, &delivery.timestamp, &delivery.body);
    delivery.signature.as_deref().unwrap_or_default().split(' ').any(|entry| entry == expected)
}

fn recovered() -> Event {
    Event::InstanceRecovered { instance_id: "inst-1".into(), instance: "Radarr".into() }
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
    assert_eq!(removed.assert_ok()["deleted"], true);
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

/// A receiver that refuses the message is not tried again: it will refuse it again.
#[tokio::test]
async fn a_refused_delivery_is_not_tried_again() {
    let app = TestApp::new().await;
    let refusing = Receiver::answering(&[400]).await;
    listening(&app, &refusing).await;
    notify::send(&app.state, recovered()).await;
    assert_eq!(refusing.deliveries().len(), 1, "a refused message was sent again");
}

/// A secret no key opens, as a restore beside another master key leaves it, signs nothing.
/// Unsigned, a message would pass a receiver that checks a signature only when there is one,
/// so nothing is sent, and the status says the secret cannot be read.
#[tokio::test]
async fn a_secret_that_cannot_be_opened_sends_nothing_and_says_so() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
        None,
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

#[tokio::test]
async fn secrets_that_cannot_be_read_send_nothing() {
    let app = TestApp::new().await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await.assert_ok();
    app.execute(&["ALTER TABLE webhook_secrets RENAME TO webhook_secrets_unreadable"]).await;

    notify::send(&app.state, recovered()).await;

    assert!(receiver.deliveries().is_empty(), "an unsigned message went out");
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
    settled(&app).await;
    assert_eq!(cleared.deliveries().len(), 1, "a cleared address was tried again");
}

#[tokio::test]
async fn a_completion_is_sent_only_to_a_webhook_that_asked_for_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await.assert_ok();
    settled(&app).await;
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
    settled(&app).await;
    assert!(receiver.deliveries().is_empty(), "a completion nobody asked for was sent");

    app.save_setting("notify_moves_completed", "true").await.assert_ok();
    executor::revert_decisions(&app.state, &quiet, false, &Confirmed::all(), &by).await.unwrap();
    let reverted = &receiver.arrived(1).await[0];
    assert_eq!(reverted["event"], "revert_completed");
    let moved = |from: &str, to: &str| {
        serde_json::json!([{
            "title": "Film 000", "media_type": "movie", "instance": "Fake radarr",
            "instance_id": "inst-1", "tmdb_id": 8392, "tvdb_id": null, "imdb_id": null,
            "from_path": from, "to_path": to, "status": "applied",
        }])
    };
    let (standard, anime) = ("/movies/standard/Film 100", "/movies/anime/Film 100");
    let task = reverted["data"]["task_id"].as_str().expect("a task named").to_string();
    let done = |moves: serde_json::Value| {
        serde_json::json!({
            "applied": 1, "failed": 0, "skipped": 0, "moving": 0, "task_id": task,
            "moves_total": 1, "truncated": false, "moves": moves,
        })
    };
    assert_eq!(reverted["data"], done(moved(anime, standard)));
    // A receiver handed a task reads each title it moved from the log.
    let logged = app.get(&format!("/api/v1/move-log?task_id={task}")).await;
    let rows = logged.assert_ok()["data"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["media_title"], "Film 000");

    let again = pending().await;
    executor::apply_decisions(&app.state, &again, false, &Confirmed::all(), &by).await.unwrap();
    let applied = &receiver.arrived(2).await[1];
    assert_eq!(applied["event"], "apply_completed");
    let task = applied["data"]["task_id"].as_str().unwrap().to_string();
    let mut expected = done(moved(standard, anime));
    expected["task_id"] = serde_json::json!(task);
    assert_eq!(applied["data"], expected);
}

/// A long apply names its first fifty titles and how many it moved in all: a receiver reads the
/// rest from the task's log, and the message stays a size every receiver takes.
#[tokio::test]
async fn a_long_apply_names_fifty_titles_and_counts_them_all() {
    use crate::services::executor::{self, Confirmed};

    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 55).await;
    app.store_setting("batch_limit", "100").await;
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    app.save_setting("notify_moves_completed", "true").await.assert_ok();
    app.simulate().await;
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM decisions WHERE status = 'pending'")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    let by = crate::jobs::Attribution::manual(None);

    executor::apply_decisions(&app.state, &ids, false, &Confirmed::all(), &by).await.unwrap();

    let data = &receiver.arrived(1).await[0]["data"];
    assert_eq!(data["applied"], 55);
    assert_eq!((data["moves_total"].clone(), data["truncated"].clone()), (55.into(), true.into()));
    assert_eq!(data["moves"].as_array().unwrap().len(), 50);
}

/// A move whose files were still on their way is announced `moving`, and again once the sync
/// that finds them in place settles it, so a media server scans the folder when the files are
/// there.
#[tokio::test]
async fn a_move_still_moving_is_announced_again_once_settled() {
    let arr = FakeArr::start().await;
    arr.moving_for(std::time::Duration::from_millis(800));
    let (app, decision_id) = super::one_move_ready(&arr).await;
    // Past `CEILING_ROUNDS` times the wait the run stops following the move, and a sync settles it.
    let app = app.with_move_wait(std::time::Duration::from_millis(1));
    let receiver = Receiver::start().await;
    listening(&app, &receiver).await;
    app.save_setting("notify_moves_completed", "true").await.assert_ok();
    let by = crate::jobs::Attribution::manual(None);
    let confirmed = crate::services::executor::Confirmed::all();

    crate::services::executor::apply_decisions(&app.state, &[decision_id], true, &confirmed, &by)
        .await
        .unwrap();
    let sent = &receiver.arrived(1).await[0];
    assert_eq!(sent["data"]["moves"][0]["status"], "moving", "{sent}");

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    crate::services::sync::sync_instance(&app.state, "inst-1", &by).await.unwrap();
    let settled = &receiver.arrived(2).await[1];
    assert_eq!(settled["event"], "moves_settled");
    let moved = &settled["data"]["moves"][0];
    assert_eq!(moved["title"], "Totoro");
    assert_eq!(moved["status"], "applied");
    assert_eq!(moved["to_path"], "/movies/anime/My Neighbor Totoro (1988)");
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
    settled(&app).await;
    assert_eq!(receiver.deliveries().len(), 1, "a failed sync nobody asked for was sent");

    app.save_setting("notify_sync_failed", "true").await.assert_ok();
    let _ = sync::sync_instance(&app.state, "inst-1", &by).await;
    let failed = &receiver.arrived(2).await[1];
    assert_eq!(failed["event"], "sync_failed");
    let name = app.state.instance("inst-1").await.unwrap().name;
    assert_eq!(failed["data"], serde_json::json!({ "instance_id": "inst-1", "instance": name }));
}

/// A Standard Webhooks receiver refuses a `webhook-timestamp` minutes from its clock, so each
/// attempt is stamped, in seconds, when it is sent. The first answer takes over a second, so
/// the retry falls in another second.
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

/// A refused delivery is retried before anything sent after it: an `instance_unreachable`
/// retried ten seconds on would land after the `instance_recovered` that followed it.
#[tokio::test]
async fn a_retried_notification_never_lands_after_a_newer_one() {
    let receiver = Receiver::answering(&[503]).await;
    let app = TestApp::new().await;
    listening(&app, &receiver).await;
    let unreachable = Event::InstanceUnreachable {
        instance_id: "inst-1".into(),
        instance: "Radarr".into(),
        error: "refused".into(),
    };

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

/// A delivery failing every time is tried four times under one id, once and then after each
/// of the three waits, and no more. A 429 is retried after the quiet its `Retry-After` asks
/// for. Every attempt is signed over its own timestamp.
#[tokio::test]
async fn a_failing_delivery_is_tried_four_times_each_signed_for_itself() {
    let app = TestApp::new().await;
    let refusing = Receiver::answering(&[500, 500, 500, 500, 500]).await;
    listening(&app, &refusing).await;
    let minted = app.post("/api/v1/notifications/webhook-secret", serde_json::json!({})).await;
    let secret = minted.assert_ok()["secret"].as_str().unwrap().to_string();

    notify::send(&app.state, recovered()).await;

    let tried = refusing.deliveries();
    assert_eq!(tried.len(), 4, "{tried:?}");
    assert!(tried.iter().all(|delivery| delivery.id == tried[0].id), "{tried:?}");
    assert!(tried.iter().all(|delivery| signed_with(&secret, delivery)), "{tried:?}");

    let throttling = Receiver::answering(&[429]).await;
    listening(&app, &throttling).await;
    let started = std::time::Instant::now();
    notify::send(&app.state, recovered()).await;
    assert_eq!(throttling.deliveries().len(), 2, "a 429 was not retried");
    assert!(started.elapsed() >= std::time::Duration::from_secs(1), "{:?}", started.elapsed());
}

/// Past the bound an event is dropped rather than queued without end, and the places come back
/// once the queue drains.
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
    settled(&app).await;
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
    settled(&app).await;
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

/// A scheduled backup that fails is sent, every time, as a failed sync is:
/// nothing else says so until the day an archive is needed. The badge says it
/// too, until a backup works again.
#[tokio::test]
async fn a_failed_scheduled_backup_is_sent_and_shown() {
    let receiver = Receiver::start().await;
    let (app, dir) = super::backup::app_with_files("backup-failure-sent").await;
    app.store_setting("notification_webhook_url", &receiver.url).await;
    // A file where the backups folder has to be: every attempt fails.
    std::fs::write(dir.join("backups"), b"not a directory").unwrap();

    let (mut synced, mut maintained, mut backed_up) = (Default::default(), None, None);
    crate::jobs::scheduler::tick(&app.state, &mut synced, &mut maintained, &mut backed_up)
        .await
        .unwrap();

    let sent = receiver.awaiting(1).await;
    let body: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
    assert_eq!(body["event"], "backup_failed", "{body}");
    let failed = app.state.localizer().await.translate("WarnBackupFailed", &[]);
    let warned = super::warning_messages(app.get("/api/v1/status").await.assert_ok());
    assert!(warned.contains(&failed), "{warned:?}");
}

/// A copy into the second folder that fails is sent whoever took the backup, since the copy
/// runs unattended and nothing else says the archives sit on one disk.
#[tokio::test]
async fn a_failed_backup_copy_is_sent_with_its_folder() {
    let receiver = Receiver::start().await;
    let (app, _dir) = super::backup::app_with_files("backup-copy-failure-sent").await;
    let folder = std::env::temp_dir().join(format!("routarr-no-share-{}", std::process::id()));
    let mut config = (*app.state.config).clone();
    config.backup_copy_dir = Some(folder.clone());
    let app = TestApp::around(app.state.clone().with_config(config));
    app.store_setting("notification_webhook_url", &receiver.url).await;

    crate::services::backup::create(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let sent = receiver.awaiting(1).await;
    let body: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
    assert_eq!(body["event"], "backup_copy_failed", "{body}");
    assert_eq!(body["data"]["folder"], folder.display().to_string(), "{body}");
}

/// A key expiring within a week is announced once, so the application gets a new key before it
/// stops. A key further off, one that never expires and one already expired are not.
#[tokio::test]
async fn a_key_about_to_expire_is_announced_once_a_week_before() {
    let receiver = Receiver::start().await;
    let app = TestApp::new().await;
    listening(&app, &receiver).await;
    app.store_setting("backup_enabled", "false").await;
    for (name, days) in
        [("soon", Some(30)), ("later", Some(30)), ("gone", Some(30)), ("ever", None)]
    {
        let body = serde_json::json!({ "name": name, "expires_in_days": days });
        app.post("/api/v1/applications", body).await.assert_ok();
    }
    app.execute(&[
        "UPDATE api_keys SET expires_at = datetime('now', '+5 days') WHERE name = 'soon'",
        "UPDATE api_keys SET expires_at = datetime('now', '-1 day') WHERE name = 'gone'",
    ])
    .await;

    for _ in 0..2 {
        let (mut synced, mut maintained, mut backed_up) = (Default::default(), None, None);
        crate::jobs::scheduler::tick(&app.state, &mut synced, &mut maintained, &mut backed_up)
            .await
            .unwrap();
    }

    let sent = receiver.awaiting(1).await;
    let body: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
    assert_eq!(body["event"], "application_key_expiring", "{body}");
    assert_eq!(body["data"]["name"], "soon", "{body}");
    assert!(body["data"]["expires_at"].as_str().unwrap().ends_with('Z'), "{body}");
    assert!(body["message"].as_str().unwrap().contains("soon"), "{body}");
    let english = crate::localization::Localizer::new("en");
    let day =
        crate::localization::human_date(chrono::Utc::now() + chrono::Duration::days(5), &english);
    assert!(body["message"].as_str().unwrap().contains(&format!("on {day}")), "{body}");
    settled(&app).await;
    assert_eq!(receiver.deliveries().len(), 1, "announced twice, or another key announced");
}
