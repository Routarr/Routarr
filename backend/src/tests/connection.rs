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

/// A server answering every request alike, as a wrong port or a proxy does.
async fn answering(
    status: u16,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
) -> super::Served {
    let status = StatusCode::from_u16(status).unwrap();
    super::serve(Router::new().fallback(move || async move {
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
async fn a_radarr_whose_library(movies: MethodRouter) -> super::Served {
    super::serve(
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

/// An address nothing listens at. A port bound and let go may be taken by
/// another test in between, and port 1 needs a privilege nothing here has.
const NOTHING_LISTENING: &str = "http://127.0.0.1:1";

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

/// Try values typed in the form for a Radarr, before anything is saved.
async fn probe(app: &TestApp, base_url: &str) -> TestResponse {
    probe_as(app, "radarr", base_url).await
}

async fn probe_as(app: &TestApp, instance_type: &str, base_url: &str) -> TestResponse {
    app.post(
        "/api/v1/instances/test",
        json!({ "instance_type": instance_type, "base_url": base_url, "api_key": "typed-key" }),
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

/// `address` with the `user:pass@` a proxy in front of the Arr asks for.
fn behind_a_proxy(address: &str) -> String {
    address.replacen("http://", "http://proxy-user:proxy-pass@", 1)
}

/// `address` as the instance form shows it, its credentials masked.
fn as_shown(address: &str) -> String {
    address.replacen("http://", "http://***@", 1)
}

/// The password of a proxy in front of the Arr is a password: the list and the
/// form show the address with it masked.
#[tokio::test]
async fn an_address_is_shown_with_its_credentials_masked() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &behind_a_proxy(&arr.base_url)).await;

    let listed = app.get("/api/v1/instances").await.assert_ok().clone();

    assert_eq!(listed[0]["base_url"], as_shown(&arr.base_url));
    let one = app.get("/api/v1/instances/inst-1").await.assert_ok().clone();
    assert_eq!(one["base_url"], as_shown(&arr.base_url));
    assert!(!format!("{listed}{one}").contains("proxy-pass"), "the password was answered");
}

/// A sentence explaining a failure quotes the address, and a 400 or a 502 body
/// is read, logged and pasted into tickets.
#[tokio::test]
async fn an_explanation_quotes_the_address_with_its_credentials_masked() {
    let app = TestApp::new().await;
    let address = NOTHING_LISTENING.to_string();

    let message = outage(&probe(&app, &behind_a_proxy(&address)).await);

    let expected = said(
        &app,
        "ArrUnreachableLoopback",
        &[("service", "Radarr"), ("address", &as_shown(&address))],
    )
    .await;
    assert_eq!(message, expected);
}

/// The form sends back the address it was shown. Saved as it came, the mask
/// would replace the credentials and the proxy would refuse every sync.
#[tokio::test]
async fn saving_the_address_as_shown_keeps_its_credentials() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &behind_a_proxy(&arr.base_url)).await;

    app.put("/api/v1/instances/inst-1", edit(&as_shown(&arr.base_url), "")).await.assert_ok();

    let kept: String = sqlx::query_scalar("SELECT base_url FROM instances WHERE id = 'inst-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(kept, behind_a_proxy(&arr.base_url));
}

/// Tried from the form, the address as shown reaches the Arr with the saved
/// credentials, the way the next sync will.
#[tokio::test]
async fn a_test_from_the_form_sends_the_saved_credentials() {
    use base64::Engine;

    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &behind_a_proxy(&arr.base_url)).await;

    app.post(
        "/api/v1/instances/test",
        json!({ "instance_type": "radarr", "base_url": as_shown(&arr.base_url), "api_key": "",
                "id": "inst-1" }),
    )
    .await
    .assert_ok();

    let basic = base64::engine::general_purpose::STANDARD.encode("proxy-user:proxy-pass");
    let seen = arr.recorded().authorizations.clone();
    assert!(!seen.is_empty() && seen.iter().all(|a| *a == format!("Basic {basic}")), "{seen:?}");
}

/// The saved credentials go to the address they were saved with, never to one
/// typed since, like the API key.
#[tokio::test]
async fn saved_credentials_do_not_follow_a_new_address() {
    let saved = FakeArr::start().await;
    let elsewhere = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &behind_a_proxy(&saved.base_url)).await;

    let moved =
        app.put("/api/v1/instances/inst-1", edit(&as_shown(&elsewhere.base_url), "new-key")).await;

    assert_eq!(
        refusal(&moved),
        said(
            &app,
            "InstanceCredentialsForNewAddress",
            &[("address", &as_shown(&elsewhere.base_url))]
        )
        .await
    );
    let tried = app
        .post(
            "/api/v1/instances/test",
            json!({ "instance_type": "radarr", "base_url": as_shown(&elsewhere.base_url),
                    "api_key": "new-key", "id": "inst-1" }),
        )
        .await;
    refusal(&tried);
    assert!(elsewhere.recorded().authorizations.is_empty(), "the credentials travelled");
}

/// In Docker, localhost is Routarr's own container, the likeliest mistake of all.
#[tokio::test]
async fn nothing_listening_at_localhost_is_explained_with_the_container_trap() {
    let app = TestApp::new().await;
    let address = NOTHING_LISTENING.to_string();

    let message = outage(&probe(&app, &address).await);

    let expected =
        said(&app, "ArrUnreachableLoopback", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(message, expected);
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_named() {
    let app = TestApp::resolving_nothing().await;
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

/// A link-local address is where a cloud host's metadata service answers,
/// with the machine's credentials, and never where an Arr runs. Routarr does
/// not connect there, written as a literal address or behind a name, and says
/// why rather than waiting out a timeout.
#[tokio::test]
async fn a_link_local_address_is_refused_before_any_connection() {
    let app = TestApp::new().await;
    for address in ["http://169.254.169.254", "http://[fe80::1]:7878"] {
        let message = refusal(&probe(&app, address).await);

        let expected =
            said(&app, "ArrLinkLocal", &[("service", "Radarr"), ("address", address)]).await;
        assert_eq!(message, expected, "{address}");
    }

    // The control: an Arr named rather than written as an address is still
    // reached through the resolver that keeps names off those ranges.
    let arr = FakeArr::start().await;
    let named = arr.base_url.replace("127.0.0.1", "localhost");
    probe(&app, &named).await.assert_ok();
}

/// Routarr appends the API's path to the address, so a query or a fragment in
/// it would carry that path into the query and send the key to whatever path
/// the address names. Both are refused, on the probe and on a save.
#[tokio::test]
async fn an_address_with_a_query_or_a_fragment_is_refused() {
    let app = TestApp::new().await;
    let arr = FakeArr::start().await;
    let expected = said(&app, "InstanceUrlPlain", &[]).await;
    for address in [format!("{}/x?y=", arr.base_url), format!("{}/#radarr", arr.base_url)] {
        assert_eq!(refusal(&probe(&app, &address).await), expected, "{address}");
        let created = app
            .post(
                "/api/v1/instances",
                json!({ "name": "Radarr", "instance_type": "radarr", "base_url": address,
                        "api_key": "k" }),
            )
            .await;
        assert_eq!(refusal(&created), expected, "{address}");
    }
    assert!(arr.recorded().api_keys.is_empty(), "the key went out");
    // The control: the same address, plain, is accepted.
    probe(&app, &arr.base_url).await.assert_ok();
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

/// The sentence names the application the operator declared, and the URL
/// base a Sonarr is reached under.
#[tokio::test]
async fn a_sonarr_is_explained_as_a_sonarr() {
    let app = TestApp::new().await;
    let radarr = answering(
        200,
        &[("content-type", "application/json")],
        r#"{"version":"5.2.6","appName":"Radarr"}"#,
    )
    .await;
    let welcome = answering(200, &[("content-type", "text/html")], "<html>Welcome</html>").await;

    let wrong_app = refusal(&probe_as(&app, "sonarr", &radarr).await);
    let not_the_api = refusal(&probe_as(&app, "sonarr", &welcome).await);

    let expected = said(
        &app,
        "ArrWrongApp",
        &[("service", "Sonarr"), ("address", &radarr), ("found", "Radarr")],
    )
    .await;
    assert_eq!(wrong_app, expected);
    let expected = said(
        &app,
        "ArrNotTheApi",
        &[("service", "Sonarr"), ("address", &welcome), ("base", "/sonarr")],
    )
    .await;
    assert_eq!(not_the_api, expected);
}

/// A script driving the sync retries a 502 and gives up on a 400. The test of
/// a saved instance explains as the probe does.
#[tokio::test]
async fn an_arr_that_is_down_answers_a_gateway_error() {
    let app = TestApp::new().await;
    let address = NOTHING_LISTENING.to_string();
    app.seed_instance_at("inst-1", "radarr", &address).await;

    let response = app.post("/api/v1/instances/inst-1/test", json!({})).await;

    let expected =
        said(&app, "ArrUnreachableLoopback", &[("service", "Radarr"), ("address", &address)]).await;
    assert_eq!(outage(&response), expected);
}

/// A sync started without waiting fails with the same explanation the
/// waited call answers, read from its task: a script polling the task gets
/// the sentence a person would have been shown.
#[tokio::test]
async fn a_sync_not_waited_for_keeps_the_explanation_on_its_task() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", NOTHING_LISTENING).await;
    let waited = app.post("/api/v1/instances/inst-1/sync", json!({})).await;
    let explained = outage(&waited);

    let request = axum::http::Request::post("/api/v1/instances/inst-1/sync")
        .header("prefer", "respond-async")
        .body(axum::body::Body::empty())
        .unwrap();
    let started = app.send(request).await;
    let task = started.assert_status(StatusCode::ACCEPTED)["job_id"].as_str().unwrap().to_string();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let job = app.get(&format!("/api/v1/jobs/{task}")).await.assert_ok().clone();
            if job["status"] != "running" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the task never ended");

    assert_eq!(ended["status"], "failed");
    assert_eq!(ended["error_message"], explained.as_str());
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

    for (id, address) in [("slow", slow.base_url.as_str()), ("stalled", &stalled)] {
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
    let address = NOTHING_LISTENING.to_string();
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

    let address = NOTHING_LISTENING.to_string();
    let params = [("service", "Radarr"), ("address", address.as_str())];
    let probed = probe(&app, &address).await;
    assert_eq!(
        in_french(outage(&probed), "ArrUnreachableLoopback", &params),
        said(&app, "ArrUnreachableLoopback", &params).await
    );
}

/// Sonarr 3 sends no `SeriesAdd` and knows no series language: Routarr works
/// with it, and says what is lost, in the test of the form and in the
/// diagnostics once a sync has read the version. A release at the minimum
/// is not warned of.
#[tokio::test]
async fn an_arr_older_than_the_minimum_is_named_with_what_it_lacks() {
    let arr = FakeArr::start().await;
    arr.report_version("3.0.10.1567", "Sonarr");
    let app = TestApp::new().await;

    let tested = probe_as(&app, "sonarr", &arr.base_url).await;
    let warning = tested.assert_ok()["warning"].as_str().unwrap_or_default().to_string();
    assert!(warning.contains("3.0.10.1567") && warning.contains("next sync"), "{warning}");

    app.seed_instance_at("inst-1", "sonarr", &arr.base_url).await;
    crate::services::sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::manual(None),
    )
    .await
    .unwrap();
    let below = |body: &serde_json::Value| {
        body["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "arr_below_version")
    };
    let status = app.get("/api/v1/status").await;
    assert!(below(status.assert_ok()), "{}", status.json);

    arr.report_version("4.0.0.738", "Sonarr");
    crate::services::sync::sync_instance(
        &app.state,
        "inst-1",
        &crate::jobs::Attribution::manual(None),
    )
    .await
    .unwrap();
    let status = app.get("/api/v1/status").await;
    assert!(!below(status.assert_ok()), "{}", status.json);
    assert_eq!(
        probe_as(&app, "sonarr", &arr.base_url).await.json["warning"],
        serde_json::Value::Null
    );
}
