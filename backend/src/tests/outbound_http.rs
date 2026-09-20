//! What the shared outbound client does with a redirect.
//!
//! Every call to a Radarr, a Sonarr or TMDb carries a credential in a *custom*
//! header (`X-Api-Key`). reqwest strips `Authorization` and `Cookie` when a
//! redirect crosses to another host, but it has no way to know a custom header
//! is sensitive — so a redirect would forward the key verbatim.

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// A server that records the `X-Api-Key` of everything it receives.
async fn recorder() -> (String, Arc<Mutex<Vec<String>>>, tokio::sync::oneshot::Sender<()>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let state = seen.clone();
    let app = Router::new().route(
        "/{*rest}",
        get(move |headers: HeaderMap| {
            let state = state.clone();
            async move {
                let key = headers
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                state.lock().unwrap().push(key);
                (StatusCode::OK, "{}").into_response()
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .ok();
    });
    (format!("http://{addr}"), seen, tx)
}

/// A server that answers every request with a 302 to `target`.
async fn redirector(target: String) -> (String, tokio::sync::oneshot::Sender<()>) {
    let app =
        Router::new().route(
            "/{*rest}",
            get(move || {
                let target = target.clone();
                async move {
                    (StatusCode::FOUND, [(axum::http::header::LOCATION, target)]).into_response()
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .ok();
    });
    (format!("http://{addr}"), tx)
}

#[tokio::test]
async fn a_redirect_to_another_host_does_not_carry_the_api_key() {
    let (elsewhere, seen, _stop_a) = recorder().await;
    let (arr, _stop_b) = redirector(format!("{elsewhere}/api/v3/movie")).await;

    let client =
        crate::http::build_client(&crate::config::Config::for_tests()).expect("test http client");
    let _ = client.get(format!("{arr}/api/v3/movie")).header("X-Api-Key", "s3cret").send().await;

    let leaked = seen.lock().unwrap().clone();
    assert!(
        leaked.iter().all(|k| k != "s3cret"),
        "the Arr API key was forwarded to another host by a redirect: {leaked:?}"
    );
}

/// The counterpart: refusing *every* redirect would also pass the test above,
/// and would break an Arr behind a proxy that normalises a trailing slash.
#[tokio::test]
async fn a_redirect_within_the_same_service_is_still_followed() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let state = seen.clone();

    // One server that redirects `/api/v3/movie` to `/api/v3/movie/` on itself.
    let app = Router::new()
        .route(
            "/api/v3/movie",
            get(|| async {
                (StatusCode::FOUND, [(axum::http::header::LOCATION, "/api/v3/movie/")])
                    .into_response()
            }),
        )
        .route(
            "/api/v3/movie/",
            get(move |headers: HeaderMap| {
                let state = state.clone();
                async move {
                    let key = headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    state.lock().unwrap().push(key);
                    (StatusCode::OK, "{}").into_response()
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await
            .ok();
    });

    let client =
        crate::http::build_client(&crate::config::Config::for_tests()).expect("test http client");
    let response = client
        .get(format!("http://{addr}/api/v3/movie"))
        .header("X-Api-Key", "s3cret")
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "same-origin redirect was not followed");
    assert_eq!(seen.lock().unwrap().as_slice(), ["s3cret"], "the key must reach the real endpoint");
    let _ = tx.send(());
}

/// A blocked redirect must surface as a failure, not as an empty success: the
/// caller would otherwise parse a `302` body as JSON and report "no media".
#[tokio::test]
async fn a_blocked_redirect_is_reported_rather_than_read_as_an_empty_library() {
    let (elsewhere, _seen, _stop_a) = recorder().await;
    let (arr, _stop_b) = redirector(format!("{elsewhere}/api/v3/movie")).await;

    let client =
        crate::http::build_client(&crate::config::Config::for_tests()).expect("test http client");
    let response =
        client.get(format!("{arr}/api/v3/movie")).header("X-Api-Key", "s3cret").send().await;

    // reqwest hands back the redirect itself; the status is not a success, so
    // `error_for_status`-style handling in the adapters treats it as a failure.
    let status = response.expect("the request itself completes").status();
    assert!(status.is_redirection(), "expected the 302 to surface, got {status}");
}
