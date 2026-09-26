//! What the interface says when an Arr cannot be reached.
//!
//! A raw failure names the transport, "connection refused" or "HTTP 401", and
//! leaves the operator to work out which of the address, the port, the key or
//! the type they typed is wrong. Each case here is served from a real socket,
//! the way a wrong address, a wrong port or a proxy answers, so what gets
//! explained is the client's own reading of the failure. The expected sentence
//! is read from the dictionary by its key, never through the code that picks it.

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{MethodRouter, get};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::fake_arr::FakeArr;
use super::{TestApp, TestResponse};

/// Serve `app` on a port of its own, and answer with its address.
async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    format!("http://{address}")
}

/// A server answering every request alike, as a wrong port or a proxy does.
async fn answering(
    status: u16,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
) -> String {
    let status = StatusCode::from_u16(status).unwrap();
    serve(Router::new().fallback(move || async move {
        let mut response = axum::response::Response::new(axum::body::Body::from(body));
        *response.status_mut() = status;
        for (name, value) in headers {
            response.headers_mut().insert(*name, value.parse().unwrap());
        }
        response
    }))
    .await
}

/// A Radarr whose status and root folders answer at once, and whose library
/// answers as `movies` does: a sync that fails past the probe.
async fn a_radarr_whose_library(movies: MethodRouter) -> String {
    serve(
        Router::new()
            .route(
                "/api/v3/system/status",
                get(|| async {
                    axum::Json(json!({ "version": "5.2.6.8376", "appName": "Radarr" }))
                }),
            )
            .route("/api/v3/rootfolder", get(|| async { axum::Json(json!([])) }))
            .route("/api/v3/tag", get(|| async { axum::Json(json!([])) }))
            .route("/api/v3/movie", movies),
    )
    .await
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

/// A port serving TLS, which answers a plain request with a TLS alert.
async fn speaking_tls() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            // A fatal "protocol version" alert, as a TLS server sends to a
            // client that did not start a handshake.
            let _ = socket.write_all(&[0x15, 0x03, 0x01, 0x00, 0x02, 0x02, 0x46]).await;
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

/// What `key` says with `params`, in the language the application speaks.
async fn said(app: &TestApp, key: &str, params: &[(&str, &str)]) -> String {
    app.state.localizer().await.translate(key, params)
}

/// A refusal of what the operator typed, a 400: trying again changes nothing.
fn refusal(response: &TestResponse) -> String {
    response.assert_status(StatusCode::BAD_REQUEST)["message"].as_str().unwrap().to_string()
}

/// An Arr down or failing, a 502: the request was right, and a retry may pass.
fn outage(response: &TestResponse) -> String {
    let body = response.assert_status(StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"], "external_api_error");
    body["message"].as_str().unwrap().to_string()
}

fn edit(base_url: &str, api_key: &str) -> serde_json::Value {
    json!({ "name": "Radarr", "instance_type": "radarr", "base_url": base_url, "api_key": api_key })
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

/// The stored key is the Arr's write credential: it goes to the address it
/// was saved with, never to one typed since.
#[tokio::test]
async fn a_blank_key_is_not_sent_to_another_address() {
    let saved = FakeArr::start().await;
    let elsewhere = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &saved.base_url).await;

    let response = app
        .post(
            "/api/v1/instances/test",
            json!({ "instance_type": "radarr", "base_url": elsewhere.base_url, "api_key": "",
                    "id": "inst-1" }),
        )
        .await;

    assert_eq!(
        refusal(&response),
        said(&app, "InstanceKeyForNewAddress", &[("address", &elsewhere.base_url)]).await
    );
    assert!(elsewhere.recorded().api_keys.is_empty());
}

#[tokio::test]
async fn a_key_typed_on_an_edit_is_the_one_tried() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    app.post(
        "/api/v1/instances/test",
        json!({ "instance_type": "radarr", "base_url": arr.base_url, "api_key": "new-key",
                "id": "inst-1" }),
    )
    .await
    .assert_ok();

    let keys = arr.recorded().api_keys.clone();
    assert!(!keys.is_empty() && keys.iter().all(|k| k == "new-key"), "{keys:?}");
}

/// Saved with the key left blank, the next sync would carry the stored key
/// to the new address.
#[tokio::test]
async fn an_edit_moving_to_another_address_needs_the_key_again() {
    let saved = FakeArr::start().await;
    let elsewhere = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &saved.base_url).await;

    let moved = app.put("/api/v1/instances/inst-1", edit(&elsewhere.base_url, "")).await;

    assert_eq!(
        refusal(&moved),
        said(&app, "InstanceKeyForNewAddress", &[("address", &elsewhere.base_url)]).await
    );
    app.put("/api/v1/instances/inst-1", edit(&saved.base_url, "")).await.assert_ok();
    app.put("/api/v1/instances/inst-1", edit(&elsewhere.base_url, "new-key")).await.assert_ok();
}

/// In Docker, localhost is Routarr's own container, the likeliest mistake of all.
#[tokio::test]
async fn nothing_listening_at_localhost_is_explained_with_the_container_trap() {
    let app = TestApp::new().await;
    let address = nothing_listening().await;

    let message = outage(&probe(&app, &address).await);

    let expected =
        said(&app, "ArrUnreachableLoopback", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_named() {
    let app = TestApp::new().await;
    // Reserved never to resolve (RFC 6761).
    let address = "http://routarr-nowhere.invalid:7878";

    let message = outage(&probe(&app, address).await);

    let expected =
        said(&app, "ArrNameUnresolved", &[("service", "Radarr"), ("address", address)]).await;
    assert_eq!(message, expected);
}

#[tokio::test]
async fn an_address_that_never_answers_is_named() {
    let app = TestApp::new().await;
    let address = never_answering().await;

    let message = outage(&probe(&app, &address).await);

    let expected = said(&app, "ArrTimedOut", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
}

/// Radarr is running and the port is right: the scheme is what is wrong.
#[tokio::test]
async fn https_on_a_plain_http_port_is_not_explained_as_nothing_listening() {
    let app = TestApp::new().await;
    let plain = answering(200, &[("content-type", "application/json")], "{}").await;
    let address = plain.replacen("http://", "https://", 1);

    let message = refusal(&probe(&app, &address).await);

    let expected =
        said(&app, "ArrHandshakeFailed", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
}

#[tokio::test]
async fn plain_http_on_a_tls_port_is_named() {
    let app = TestApp::new().await;
    let address = speaking_tls().await;

    let message = refusal(&probe(&app, &address).await);

    let expected = said(&app, "ArrNotHttp", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
}

#[tokio::test]
async fn a_refused_key_is_named() {
    let app = TestApp::new().await;
    let address = answering(401, &[], "Unauthorized").await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, said(&app, "ArrKeyRefused", &[("service", "Radarr")]).await);
}

/// Radarr names itself in its own challenge, a proxy in front names itself.
#[tokio::test]
async fn a_sign_in_asked_for_in_front_of_the_arr_is_not_blamed_on_the_key() {
    let app = TestApp::new().await;
    let proxy = answering(401, &[("www-authenticate", "Basic realm=\"Authelia\"")], "").await;
    let radarr = answering(401, &[("www-authenticate", "Basic realm=\"Radarr\"")], "").await;

    let message = refusal(&probe(&app, &proxy).await);
    assert_eq!(message, said(&app, "ArrProxySignIn", &[("service", "Radarr")]).await);

    let message = refusal(&probe(&app, &radarr).await);
    assert_eq!(message, said(&app, "ArrKeyRefused", &[("service", "Radarr")]).await);
}

/// Radarr answers a wrong key with 401. A 403 is a firewall, an allow list or
/// a sign-in policy in front of it, and a 502 to 504 a proxy that cannot
/// reach it.
#[tokio::test]
async fn a_status_a_proxy_answers_is_not_blamed_on_the_arr() {
    let app = TestApp::new().await;

    let refused = answering(403, &[], "Forbidden").await;
    let message = refusal(&probe(&app, &refused).await);
    assert_eq!(message, said(&app, "ArrProxyRefused", &[("service", "Radarr")]).await);

    let unreachable = answering(502, &[], "Bad Gateway").await;
    let message = outage(&probe(&app, &unreachable).await);
    let expected =
        said(&app, "ArrProxyUnreachable", &[("service", "Radarr"), ("status", "502")]).await;
    assert_eq!(message, expected);
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

        let expected = said(
            &app,
            "ArrNotTheApi",
            &[("service", "Radarr"), ("address", &address), ("base", "/radarr")],
        )
        .await;
        assert_eq!(message, expected);
    }
}

#[tokio::test]
async fn an_arr_failing_on_its_own_side_is_named() {
    let app = TestApp::new().await;
    let address = answering(500, &[], "boom").await;

    let message = outage(&probe(&app, &address).await);

    let expected = said(&app, "ArrServerError", &[("service", "Radarr"), ("status", "500")]).await;
    assert_eq!(message, expected);
}

/// The key never follows a redirect to another origin, so the probe stops
/// there. On the same host, the operator is told to use the address it names.
#[tokio::test]
async fn a_redirect_to_another_port_is_named() {
    let app = TestApp::new().await;
    let address = answering(302, &[("location", "http://127.0.0.1:1/radarr")], "").await;

    let message = refusal(&probe(&app, &address).await);

    assert_eq!(message, said(&app, "ArrRedirected", &[("address", &address)]).await);
}

/// On another host, it is as likely a sign-in portal as the Arr.
#[tokio::test]
async fn a_redirect_to_another_host_names_that_host() {
    let app = TestApp::new().await;
    let address = answering(302, &[("location", "https://auth.example.org/?rd=radarr")], "").await;

    let message = refusal(&probe(&app, &address).await);

    let expected = said(
        &app,
        "ArrRedirectedElsewhere",
        &[("service", "Radarr"), ("address", &address), ("host", "auth.example.org")],
    )
    .await;
    assert_eq!(message, expected);
}

/// A proxy rule sending `/api` back to itself: nothing to follow elsewhere.
#[tokio::test]
async fn a_redirect_loop_is_named_as_one() {
    let app = TestApp::new().await;
    let address = answering(302, &[("location", "/loop")], "").await;

    let message = refusal(&probe(&app, &address).await);

    let expected =
        said(&app, "ArrRedirectLoop", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
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

    let expected = said(
        &app,
        "ArrWrongApp",
        &[("service", "Radarr"), ("address", &address), ("found", "Sonarr")],
    )
    .await;
    assert_eq!(message, expected);
}

/// A script driving the sync retries a 502 and gives up on a 400. The test of
/// a saved instance explains as the probe does.
#[tokio::test]
async fn an_arr_that_is_down_answers_a_gateway_error() {
    let app = TestApp::new().await;
    let address = nothing_listening().await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/test", json!({})).await;

    let expected =
        said(&app, "ArrUnreachableLoopback", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(outage(&response), expected);
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

    let expected = said(
        &app,
        "ArrWrongApp",
        &[("service", "Radarr"), ("address", &address), ("found", "Sonarr")],
    )
    .await;
    assert_eq!(refusal(&response), expected);
}

/// Past a probe that passed, the address is right, and the library is what
/// the sync could not read.
#[tokio::test]
async fn a_library_slower_than_the_timeout_is_not_blamed_on_the_address() {
    let app = TestApp::new().await;
    let slow = FakeArr::holding_edits(Duration::from_secs(1)).await;
    let stalled = a_radarr_whose_library(get(|| async {
        let body = futures::stream::once(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"[]"))
        });
        axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(axum::body::Body::from_stream(body))
            .unwrap()
    }))
    .await;

    for (id, address) in [("slow", &slow.base_url), ("stalled", &stalled)] {
        app.seed_instance_at(id, "radarr", address).await;

        let response = app.post(&format!("/api/v1/instances/{id}/sync"), json!({})).await;

        assert_eq!(
            outage(&response),
            said(&app, "InstanceSyncTimedOut", &[("service", "Radarr")]).await,
            "{id}"
        );
    }
}

#[tokio::test]
async fn a_library_that_cannot_be_read_is_named_as_such() {
    let app = TestApp::new().await;
    let address =
        a_radarr_whose_library(get(|| async { axum::Json(json!([{ "id": "not-a-number" }])) }))
            .await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/sync", json!({})).await;

    assert_eq!(
        outage(&response),
        said(&app, "InstanceSyncUnreadable", &[("service", "Radarr")]).await
    );
}

#[tokio::test]
async fn a_library_failing_on_the_arrs_side_is_named() {
    let app = TestApp::new().await;
    let address = a_radarr_whose_library(get(|| async {
        (StatusCode::INTERNAL_SERVER_ERROR, "Database is locked")
    }))
    .await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/sync", json!({})).await;

    let expected = said(&app, "ArrServerError", &[("service", "Radarr"), ("status", "500")]).await;
    assert_eq!(outage(&response), expected);
}

#[tokio::test]
async fn a_sync_with_nothing_listening_is_explained() {
    let app = TestApp::new().await;
    let address = nothing_listening().await;
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/sync", json!({})).await;

    let expected =
        said(&app, "ArrUnreachableLoopback", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(outage(&response), expected);
}

/// Read under the field it names, so in the reader's language, from each of
/// the three handlers the form calls.
#[tokio::test]
async fn the_form_is_answered_in_the_readers_language() {
    let app = TestApp::new().await;
    app.put("/api/v1/settings", json!({ "settings": { "ui_language": "fr" } })).await.assert_ok();
    let english = crate::localization::Localizer::new("en");
    let in_french = |message: String, key: &str, params: &[(&str, &str)]| {
        assert_ne!(message, english.translate(key, params), "{key} came back in English");
        message
    };

    let created = app
        .post(
            "/api/v1/instances",
            json!({ "name": "Radarr", "instance_type": "radarr", "base_url": "radarr:7878",
                    "api_key": "k" }),
        )
        .await;
    assert_eq!(
        in_french(refusal(&created), "InstanceUrlScheme", &[]),
        said(&app, "InstanceUrlScheme", &[]).await
    );

    app.seed_instance_at("inst-1", "radarr", "http://radarr:7878").await;
    let updated = app
        .put(
            "/api/v1/instances/inst-1",
            json!({ "name": " ", "instance_type": "radarr", "base_url": "http://radarr:7878",
                    "api_key": "" }),
        )
        .await;
    assert_eq!(
        in_french(refusal(&updated), "InstanceNameRequired", &[]),
        said(&app, "InstanceNameRequired", &[]).await
    );

    let address = nothing_listening().await;
    let params = [("service", "Radarr"), ("address", address.as_str())];
    let probed = probe(&app, &address).await;
    assert_eq!(
        in_french(outage(&probed), "ArrUnreachableLoopback", &params),
        said(&app, "ArrUnreachableLoopback", &params).await
    );
}
