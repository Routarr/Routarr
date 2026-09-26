//! What the interface says when an Arr cannot be reached.
//!
//! A raw failure names the transport, "connection refused" or "HTTP 401", and
//! leaves the operator to work out which of the address, the port, the key or
//! the type they typed is wrong. Each case here is served from a real socket,
//! the way a wrong address, a wrong port or a proxy answers, so what gets
//! explained is the client's own reading of the failure.

use axum::Router;
use axum::http::StatusCode;
use serde_json::json;
use tokio::net::TcpListener;

use crate::services::connection::{self, Cause};

use super::fake_arr::FakeArr;
use super::{TestApp, TestResponse};

/// A server answering every request alike, as a wrong port or a proxy does.
async fn answering(
    status: u16,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
) -> String {
    let status = StatusCode::from_u16(status).unwrap();
    let app = Router::new().fallback(move || async move {
        let mut response = axum::response::Response::new(axum::body::Body::from(body));
        *response.status_mut() = status;
        for (name, value) in headers {
            response.headers_mut().insert(*name, value.parse().unwrap());
        }
        response
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    format!("http://{address}")
}

/// An address nothing listens at: a port bound, then let go.
async fn nothing_listening() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}")
}

/// An address that takes the connection and never answers, as a host behind
/// a firewall that drops rather than refuses.
async fn never_answering() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    format!("http://{address}")
}

/// Try values typed in the form, before anything is saved.
async fn probe(app: &TestApp, base_url: &str) -> TestResponse {
    app.post(
        "/api/v1/instances/test",
        json!({ "instance_type": "radarr", "base_url": base_url, "api_key": "typed-key" }),
    )
    .await
}

/// The explanation the operator should read for `cause` at `base_url`.
async fn explained(app: &TestApp, cause: Cause, base_url: &str) -> String {
    connection::explain(&cause, "radarr", base_url, &app.state.localizer().await)
}

fn refusal(response: &TestResponse) -> String {
    response.assert_status(StatusCode::BAD_REQUEST)["message"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn values_typed_in_the_form_are_tried_before_anything_is_saved() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;

    let response = probe(&app, &arr.base_url).await;

    let body = response.assert_ok();
    assert_eq!(body["version"], "5.2.6.8376");
    assert_eq!(body["app_name"], "Radarr");
    let keys = arr.recorded().api_keys.clone();
    assert!(!keys.is_empty() && keys.iter().all(|k| k == "typed-key"), "{keys:?}");
    let saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(saved, 0);
}

/// The edit form never shows the stored key, and leaving it blank keeps it.
#[tokio::test]
async fn a_blank_key_on_an_edit_tries_the_stored_one() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    app.post(
        "/api/v1/instances/test",
        json!({ "instance_type": "radarr", "base_url": arr.base_url, "api_key": "", "id": "inst-1" }),
    )
    .await
    .assert_ok();

    let keys = arr.recorded().api_keys.clone();
    assert!(!keys.is_empty() && keys.iter().all(|k| k == "arr-key"), "{keys:?}");
}

/// In Docker, localhost is Routarr's own container, the likeliest mistake of all.
#[tokio::test]
async fn nothing_listening_at_localhost_is_explained_with_the_container_trap() {
    let app = TestApp::new().await;
    let address = nothing_listening().await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::Unreachable, &address).await);
    assert!(connection::is_loopback(&address));
}

#[tokio::test]
async fn an_address_that_never_answers_is_named() {
    let app = TestApp::new().await;
    let address = never_answering().await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::TimedOut, &address).await);
}

#[tokio::test]
async fn a_refused_key_is_named() {
    let app = TestApp::new().await;
    let address = answering(401, &[], "Unauthorized").await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::KeyRefused, &address).await);
}

/// A wrong port, or a missing URL base, lands on something that is not the API.
#[tokio::test]
async fn an_answer_that_is_not_the_api_is_named() {
    let app = TestApp::new().await;
    for address in [
        answering(404, &[], "Not Found").await,
        answering(200, &[("content-type", "text/html")], "<html>Welcome</html>").await,
    ] {
        let message = refusal(&probe(&app, &address).await);

        assert_eq!(message, explained(&app, Cause::NotTheApi, &address).await);
    }
}

#[tokio::test]
async fn an_arr_failing_on_its_own_side_is_named() {
    let app = TestApp::new().await;
    let address = answering(500, &[], "boom").await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::ServerError(500), &address).await);
}

/// The key never follows a redirect to another origin, so the probe stops there.
#[tokio::test]
async fn a_redirect_elsewhere_is_named() {
    let app = TestApp::new().await;
    let address = answering(302, &[("location", "https://radarr.example.org/")], "").await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::Redirected, &address).await);
}

/// 7878 and 8989 are one digit apart in a person's memory.
#[tokio::test]
async fn a_sonarr_declared_as_a_radarr_is_named() {
    let app = TestApp::new().await;
    let address = answering(
        200,
        &[("content-type", "application/json")],
        r#"{"version":"4.0.0","appName":"Sonarr"}"#,
    )
    .await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, explained(&app, Cause::WrongApp("Sonarr".into()), &address).await);
}

#[tokio::test]
async fn the_test_of_a_saved_instance_names_the_same_causes() {
    let app = TestApp::new().await;
    let address = answering(401, &[], "Unauthorized").await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/test", json!({})).await;

    assert_eq!(refusal(&response), explained(&app, Cause::KeyRefused, &address).await);
}

/// The first sync runs as the form closes, so its failure is the one most
/// people read, and a probe tells the wrong type apart from a wrong port.
#[tokio::test]
async fn a_failed_sync_is_explained_by_what_answers_at_the_address() {
    let app = TestApp::new().await;
    let address = answering(
        200,
        &[("content-type", "application/json")],
        r#"{"version":"4.0.0","appName":"Sonarr"}"#,
    )
    .await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/sync", json!({})).await;

    assert_eq!(
        refusal(&response),
        explained(&app, Cause::WrongApp("Sonarr".into()), &address).await
    );
}

#[tokio::test]
async fn a_sync_with_nothing_listening_is_explained() {
    let app = TestApp::new().await;
    let address = nothing_listening().await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/sync", json!({})).await;

    assert_eq!(refusal(&response), explained(&app, Cause::Unreachable, &address).await);
}

/// Read under the field it names, so in the reader's language.
#[tokio::test]
async fn a_refusal_of_the_form_itself_is_translated() {
    let app = TestApp::new().await;
    let localizer = app.state.localizer().await;

    let response = app
        .post(
            "/api/v1/instances",
            json!({ "name": "Radarr", "instance_type": "radarr", "base_url": "radarr:7878",
                    "api_key": "k" }),
        )
        .await;
    assert_eq!(refusal(&response), localizer.translate("InstanceUrlScheme", &[]));

    let response = app
        .post(
            "/api/v1/instances",
            json!({ "name": " ", "instance_type": "radarr", "base_url": "http://radarr:7878",
                    "api_key": "k" }),
        )
        .await;
    assert_eq!(refusal(&response), localizer.translate("InstanceNameRequired", &[]));
}
