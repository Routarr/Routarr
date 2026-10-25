//! The last webhook delivery of each instance, which tells an Arr whose webhook never reached
//! Routarr from a quiet one.

use super::TestApp;

const DELIVERY: &str = "/api/v1/webhook/inst-1/tok";

async fn last_delivery(app: &TestApp) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT last_webhook_at, last_webhook_event FROM instances WHERE id = 'inst-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

/// A delivery with the valid token is stamped with its event, at most once a minute because a
/// season sends one per episode, and a refused token leaves nothing.
#[tokio::test]
async fn a_delivery_is_stamped_with_its_event_and_a_refused_one_is_not() {
    let app = TestApp::new().await;
    app.seed_library().await;

    app.post("/api/v1/webhook/inst-1/wrong", serde_json::json!({ "eventType": "Test" }))
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
    assert_eq!(last_delivery(&app).await, (None, None));

    app.post(DELIVERY, serde_json::json!({ "eventType": "Test" })).await.assert_ok();
    let (at, event) = last_delivery(&app).await;
    assert!(at.is_some());
    assert_eq!(event.as_deref(), Some("Test"));

    app.post(DELIVERY, serde_json::json!({ "eventType": "Grab" })).await.assert_ok();
    assert_eq!(last_delivery(&app).await.1.as_deref(), Some("Test"), "stamped within the minute");

    app.execute(&["UPDATE instances SET last_webhook_at = datetime('now', '-2 minutes')"]).await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "Grab" })).await.assert_ok();
    assert_eq!(last_delivery(&app).await.1.as_deref(), Some("Grab"));
}

/// A stored word would read in English whatever the interface's language.
#[tokio::test]
async fn a_delivery_naming_no_event_is_stamped_without_one() {
    let app = TestApp::new().await;
    app.seed_library().await;

    app.post(DELIVERY, serde_json::json!({})).await.assert_ok();
    let (at, event) = last_delivery(&app).await;
    assert!(at.is_some());
    assert_eq!(event, None);

    app.execute(&["UPDATE instances SET last_webhook_at = datetime('now', '-2 minutes')"]).await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "\u{7}" })).await.assert_ok();
    assert_eq!(last_delivery(&app).await.1, None, "an event of control characters alone");
}

/// A rotated token waits for the new address's first delivery, so the stamp goes with the old
/// one, and the hour before a warning counts from the rotation.
#[tokio::test]
async fn rotating_the_token_clears_the_last_delivery() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "Test" })).await.assert_ok();

    let rotated = app.post("/api/v1/instances/inst-1/webhook-token", serde_json::json!({})).await;
    assert!(rotated.assert_ok()["last_webhook_at"].is_null());
    assert_eq!(last_delivery(&app).await, (None, None));
    let issued: Option<String> =
        sqlx::query_scalar("SELECT webhook_token_at FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(issued.is_some());
}

/// An instance armed for automatic application and silent an hour after it was added is
/// warned of: a new title then waits for the next sync without a word. An instance not armed, or
/// added a moment ago, or heard from, is not.
#[tokio::test]
async fn an_armed_instance_never_heard_from_is_warned_of_an_hour_on() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["UPDATE instances SET created_at = datetime('now', '-2 hours')"]).await;
    let warned = || async {
        let status = app.get("/api/v1/status").await;
        let body = status.assert_ok().clone();
        body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == "webhook_never_delivered")
            .map(|w| w["message"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    assert!(warned().await.is_empty(), "warned with automatic application off");
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let messages = warned().await;
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("Radarr"), "{}", messages[0]);

    app.execute(&["UPDATE instances SET auto_apply = 0"]).await;
    assert!(warned().await.is_empty(), "warned for an instance kept to proposals");
    app.execute(&["UPDATE instances SET auto_apply = 1, created_at = datetime('now')"]).await;
    assert!(warned().await.is_empty(), "warned for an instance added a moment ago");
    app.execute(&["UPDATE instances SET created_at = datetime('now', '-2 hours')"]).await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "Test" })).await.assert_ok();
    assert!(warned().await.is_empty(), "warned once a delivery arrived");
}

/// The last delivery is read on the instance, where an application sees it too, with the switch
/// the Arr names it by, and counted.
#[tokio::test]
async fn the_last_delivery_is_read_and_counted() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "Download" })).await.assert_ok();

    let read = app.get("/api/v1/instances/inst-1").await;
    let instance = read.assert_ok();
    assert!(instance["last_webhook_at"].is_string(), "{instance}");
    assert_eq!(instance["last_webhook_event"], "Download");
    assert_eq!(instance["last_webhook_event_label"], "On File Import");
    let health = app.get("/api/v1/health?probe=false").await;
    let heard = &health.assert_ok()["instances"][0];
    assert_eq!(heard["last_webhook_event"], "Download");
    assert_eq!(heard["last_webhook_event_label"], "On File Import");

    // `Test` is sent by a button, no switch.
    app.execute(&["UPDATE instances SET last_webhook_at = datetime('now', '-2 minutes')"]).await;
    app.post(DELIVERY, serde_json::json!({ "eventType": "Test" })).await.assert_ok();
    let read = app.get("/api/v1/instances/inst-1").await;
    assert!(read.assert_ok()["last_webhook_event_label"].is_null());
    let metrics = app.text("/api/v1/metrics").await;
    assert!(
        metrics.lines().any(|line| line.starts_with(
            r#"routarr_instance_last_webhook_timestamp_seconds{arr_instance="Radarr",arr_instance_id="inst-1"} "#
        )),
        "{metrics}"
    );
}
