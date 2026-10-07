//! What the shared outbound client does with a redirect.
//!
//! Every call to a Radarr, a Sonarr or TMDB carries a credential in a *custom*
//! header (`X-Api-Key`). reqwest strips `Authorization` and `Cookie` when a
//! redirect crosses to another host, but it has no way to know a custom header
//! is sensitive, so a redirect would forward the key verbatim.

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use std::sync::{Arc, Mutex};

/// A server that records the `X-Api-Key` of everything it receives.
async fn recorder() -> (super::Served, Arc<Mutex<Vec<String>>>) {
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
    (super::serve(app).await, seen)
}

/// A server that answers every request with a 302 to `target`.
async fn redirector(target: String) -> super::Served {
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
    super::serve(app).await
}

#[tokio::test]
async fn a_redirect_to_another_host_does_not_carry_the_api_key() {
    let (elsewhere, seen) = recorder().await;
    let arr = redirector(format!("{elsewhere}/api/v3/movie")).await;

    let client = super::http_client();
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
    let server = super::serve(app).await;

    let client = super::http_client();
    let response = client
        .get(format!("{server}/api/v3/movie"))
        .header("X-Api-Key", "s3cret")
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "same-origin redirect was not followed");
    assert_eq!(seen.lock().unwrap().as_slice(), ["s3cret"], "the key must reach the real endpoint");
}

/// A blocked redirect comes back as the redirect itself, a status that is not
/// a success, so `check_status` in the Arr clients reports it rather than
/// parsing a `302` body as an empty library. The adapter's own wording is
/// pinned in `tests::connection`.
#[tokio::test]
async fn a_blocked_redirect_comes_back_as_the_redirect_itself() {
    let (elsewhere, _seen) = recorder().await;
    let arr = redirector(format!("{elsewhere}/api/v3/movie")).await;

    let client = super::http_client();
    let response =
        client.get(format!("{arr}/api/v3/movie")).header("X-Api-Key", "s3cret").send().await;

    let status = response.expect("the request itself completes").status();
    assert!(status.is_redirection(), "expected the 302 to surface, got {status}");
}
