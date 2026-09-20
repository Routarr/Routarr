//! Serving Routarr under a sub-path.
//!
//! The Servarr "URL base" convention: `https://host/routarr` behind the family
//! reverse proxy. Everything here is about one failure mode — a mount point that
//! works on the developer's `/` and breaks behind the proxy, which is the one
//! place it cannot be debugged comfortably.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::config::{Config, normalise_base_path};
use crate::state::AppState;

use super::TestApp;

/// A router mounted under `base`.
async fn mounted_at(base: &str) -> (axum::Router, AppState) {
    let mut config = Config::for_tests();
    config.base_path = normalise_base_path(base);

    let mut state = AppState::for_tests().await;
    state.config = std::sync::Arc::new(config);

    (crate::build_router(state.clone()), state)
}

/// A typo in an API path answered `index.html` with a 200: a script then
/// parsed HTML as JSON, and the message it got named neither the route nor the
/// mistake. Under the API prefix every miss is a JSON 404, whatever the method
/// and wherever the application is mounted.
#[tokio::test]
async fn an_unknown_api_path_is_a_json_404_not_the_index() {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    for (base, prefix) in [("", ""), ("/routarr", "/routarr")] {
        let (router, _) = mounted_at(base).await;
        for method in ["GET", "POST", "DELETE"] {
            let request = Request::builder()
                .method(method)
                .uri(format!("{prefix}/api/v1/nope"))
                .body(Body::empty())
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {prefix}/api/v1/nope");
            let content_type = response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(content_type.starts_with("application/json"), "{method}: {content_type}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["error"], "not_found", "{json}");
        }
    }
}

async fn status_of(router: &axum::Router, path: &str) -> StatusCode {
    router
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .expect("router call")
        .status()
}

#[test]
fn every_way_a_person_writes_a_sub_path_means_the_same_thing() {
    // People write all four and expect all four to work. Getting this wrong
    // yields a double slash or a missing one in every generated URL.
    for written in ["/routarr", "routarr", "/routarr/", "routarr/", "  /routarr/  "] {
        assert_eq!(normalise_base_path(written), "/routarr", "for {written:?}");
    }

    // Nested mount points survive; empty stays empty.
    assert_eq!(normalise_base_path("/media/routarr/"), "/media/routarr");
    for nothing in ["", "/", "   ", "//"] {
        assert_eq!(normalise_base_path(nothing), "", "for {nothing:?}");
    }
}

#[tokio::test]
async fn the_api_answers_under_the_mount_point() {
    let (router, _state) = mounted_at("/routarr").await;

    assert_eq!(status_of(&router, "/routarr/api/v1/ping").await, StatusCode::OK);
}

#[tokio::test]
async fn nothing_answers_outside_the_mount_point() {
    let (router, _state) = mounted_at("/routarr").await;

    // A proxy strips its own prefix or it does not; if it does not, answering at
    // the root anyway would hide the misconfiguration until something subtler
    // broke.
    assert_eq!(status_of(&router, "/api/v1/ping").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_default_deployment_is_untouched() {
    let (router, _state) = mounted_at("").await;

    assert_eq!(status_of(&router, "/api/v1/ping").await, StatusCode::OK);
    assert_eq!(status_of(&router, "/routarr/api/v1/ping").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_webhook_url_is_advertised_relative_to_the_mount_point() {
    // Radarr is handed this URL to call back. Missing the prefix, every webhook
    // would hit the proxy's 404 and the user would see nothing at all.
    let (router, _state) = mounted_at("/routarr").await;

    let response = router
        .oneshot(
            Request::post("/routarr/api/v1/instances")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "Radarr",
                        "instance_type": "radarr",
                        "base_url": "http://radarr:7878",
                        "api_key": "k",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("router call");

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let url = body["webhook_url"].as_str().expect("a webhook url");

    assert!(url.starts_with("/routarr/api/v1/webhook/"), "got {url}");
}

/// The index must carry a `<base href>` pointing at the mount point.
///
/// Without it a deep link like `/routarr/rules` resolves the relative asset URLs
/// against `/routarr/rules/` and the page comes up blank — behind the proxy, and
/// only there.
#[tokio::test]
async fn the_index_pins_asset_resolution_to_the_mount_point() {
    let dir = tempdir();
    std::fs::write(
        dir.join("index.html"),
        "<!DOCTYPE html>\n<html>\n  <head>\n    <title>x</title>\n  </head>\n  <body></body>\n</html>",
    )
    .unwrap();

    for (base, expected) in [("/routarr", "<base href=\"/routarr/\">"), ("", "<base href=\"/\">")] {
        let mut config = Config::for_tests();
        config.base_path = normalise_base_path(base);
        config.frontend_dir = dir.clone();

        let html = crate::index_html(&config);
        assert!(html.contains(expected), "for base {base:?}, got:\n{html}");
        // Injected inside the head, not before the doctype.
        assert!(html.find("<head>").unwrap() < html.find("<base").unwrap());
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_deep_link_and_the_root_both_serve_the_application() {
    let dir = tempdir();
    std::fs::write(dir.join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();

    let mut config = Config::for_tests();
    config.base_path = normalise_base_path("/routarr");
    config.frontend_dir = dir.clone();

    let mut state = AppState::for_tests().await;
    state.config = std::sync::Arc::new(config);
    let router = crate::build_router(state);

    // The root, a client-side route, and a real asset.
    assert_eq!(status_of(&router, "/routarr/").await, StatusCode::OK);
    assert_eq!(status_of(&router, "/routarr/rules").await, StatusCode::OK);
    assert_eq!(status_of(&router, "/routarr/assets/app.js").await, StatusCode::OK);
    // And nothing outside.
    assert_eq!(status_of(&router, "/rules").await, StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&dir).ok();
}

/// A scratch directory. `tempfile` is not a dependency and one test does not
/// justify adding it.
fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("routarr-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn the_default_test_harness_still_has_no_prefix() {
    // Guards the other 300-odd tests: they all address `/api/v1/...`.
    let app = TestApp::new().await;
    assert!(app.state.config.base_path.is_empty());
}
