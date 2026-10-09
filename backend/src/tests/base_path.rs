//! Serving Routarr under a sub-path.
//!
//! The Servarr "URL base" convention: `https://host/routarr` behind the family
//! reverse proxy. Everything here is about one failure mode: a mount point that
//! works on the developer's `/` and breaks behind the proxy, which is the one
//! place it cannot be debugged comfortably.

use axum::http::StatusCode;

use crate::config::{Config, normalise_base_path};
use crate::state::AppState;

use super::TestApp;

/// The application under `config`, as `main` assembles it.
async fn serving(config: Config) -> TestApp {
    TestApp::around(AppState::for_tests().await.with_config(config))
}

/// The application mounted under `base`.
async fn mounted_at(base: &str) -> TestApp {
    let mut config = Config::for_tests();
    config.base_path = normalise_base_path(base);
    serving(config).await
}

/// Under the API prefix every miss is a JSON 404, whatever the method and
/// wherever the application is mounted. Answered with `index.html` and a 200,
/// a typo in an API path leaves a script parsing HTML as JSON, with a message
/// that names neither the route nor the mistake.
#[tokio::test]
async fn an_unknown_api_path_is_a_json_404_not_the_index() {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;

    for (base, prefix) in [("", ""), ("/routarr", "/routarr")] {
        let app = mounted_at(base).await;
        for method in ["GET", "POST", "DELETE"] {
            let request = Request::builder()
                .method(method)
                .uri(format!("{prefix}/api/v1/nope"))
                .body(Body::empty())
                .unwrap();
            let response = app.send_raw(request).await;
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

async fn status_of(app: &TestApp, path: &str) -> StatusCode {
    app.raw(path).await.status()
}

#[test]
fn every_way_a_person_writes_a_sub_path_means_the_same_thing() {
    // People write all four and expect all four to work. Getting this wrong
    // yields a double slash or a missing one in every generated URL.
    for written in ["/routarr", "routarr", "/routarr/", "routarr/", "  /routarr/  "] {
        assert_eq!(normalise_base_path(written), "/routarr", "for {written:?}");
    }

    // Nested mount points survive, and empty stays empty.
    assert_eq!(normalise_base_path("/media/routarr/"), "/media/routarr");
    for nothing in ["", "/", "   ", "//"] {
        assert_eq!(normalise_base_path(nothing), "", "for {nothing:?}");
    }
}

#[tokio::test]
async fn the_api_answers_under_the_mount_point() {
    let app = mounted_at("/routarr").await;

    assert_eq!(status_of(&app, "/routarr/api/v1/ping").await, StatusCode::OK);
}

/// A task started without waiting is found at the address its answer gives,
/// under the mount point as anywhere: a `Location` without it sends a client
/// polling to a path nothing answers.
#[tokio::test]
async fn a_task_not_waited_for_is_located_under_the_mount_point() {
    let app = mounted_at("/routarr").await;
    let request = axum::http::Request::post("/routarr/api/v1/simulate")
        .header("prefer", "respond-async")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let started = app.send(request).await;

    let job = started.assert_status(StatusCode::ACCEPTED)["job_id"].as_str().unwrap().to_string();
    let location = started.location().expect("no Location came back");
    assert_eq!(location, format!("/routarr/api/v1/jobs/{job}"));
    assert_eq!(status_of(&app, &location).await, StatusCode::OK);
}

#[tokio::test]
async fn nothing_answers_outside_the_mount_point() {
    let app = mounted_at("/routarr").await;

    // A proxy strips its own prefix or it does not. If it does not, answering at
    // the root anyway would hide the misconfiguration until something subtler
    // broke.
    assert_eq!(status_of(&app, "/api/v1/ping").await, StatusCode::NOT_FOUND);
}

/// The bare address, typed by someone who forgot the mount point, leads to
/// it rather than to an empty page.
#[tokio::test]
async fn the_root_leads_to_the_mount_point() {
    let app = mounted_at("/routarr").await;

    let answer = app.raw("/").await;

    assert_eq!(answer.status(), StatusCode::TEMPORARY_REDIRECT);
    let location = answer.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/routarr/");
}

#[tokio::test]
async fn the_default_deployment_is_untouched() {
    let app = mounted_at("").await;

    assert_eq!(status_of(&app, "/api/v1/ping").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/routarr/api/v1/ping").await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_webhook_url_is_advertised_relative_to_the_mount_point() {
    // Radarr is handed this URL to call back. Missing the prefix, every webhook
    // would hit the proxy's 404 and the user would see nothing at all.
    let app = mounted_at("/routarr").await;

    let response = app
        .post(
            "/routarr/api/v1/instances",
            serde_json::json!({
                "name": "Radarr",
                "instance_type": "radarr",
                "base_url": "http://radarr:7878",
                "api_key": "k",
            }),
        )
        .await;

    let url = response.assert_ok()["webhook_url"].as_str().expect("a webhook url");

    assert!(url.starts_with("/routarr/api/v1/webhook/"), "got {url}");
}

/// The index must carry a `<base href>` pointing at the mount point.
///
/// Without it a deep link like `/routarr/rules` resolves the relative asset URLs
/// against `/routarr/rules/` and the page comes up blank, behind the proxy and
/// only there.
#[tokio::test]
async fn the_index_pins_asset_resolution_to_the_mount_point() {
    let dir = super::TempDir::new("base-href");
    std::fs::write(
        dir.join("index.html"),
        "<!DOCTYPE html>\n<html>\n  <head>\n    <title>x</title>\n  </head>\n  <body></body>\n</html>",
    )
    .unwrap();

    for (base, expected) in [("/routarr", "<base href=\"/routarr/\">"), ("", "<base href=\"/\">")] {
        let mut config = Config::for_tests();
        config.base_path = normalise_base_path(base);
        config.frontend_dir = dir.to_path_buf();

        let html = crate::index_html(&config);
        assert!(html.contains(expected), "for base {base:?}, got:\n{html}");
        // Injected inside the head, not before the doctype.
        assert!(html.find("<head>").unwrap() < html.find("<base").unwrap());
    }
}

#[tokio::test]
async fn a_deep_link_and_the_root_both_serve_the_application() {
    let dir = super::TempDir::new("deep-link");
    std::fs::write(dir.join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();

    let mut config = Config::for_tests();
    config.base_path = normalise_base_path("/routarr");
    config.frontend_dir = dir.to_path_buf();

    let app = serving(config).await;

    // The root, a client-side route, and a real asset.
    assert_eq!(status_of(&app, "/routarr/").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/routarr/rules").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/routarr/assets/app.js").await, StatusCode::OK);
    // And nothing outside.
    assert_eq!(status_of(&app, "/rules").await, StatusCode::NOT_FOUND);
}

/// The dictionary the page carries, between its script tags.
fn embedded_dictionary(html: &str) -> serde_json::Value {
    let open = r#"<script type="application/json" id="dictionary">"#;
    let start = html.find(open).expect("the page carries no dictionary") + open.len();
    let end = start + html[start..].find("</script>").expect("an unclosed dictionary");
    serde_json::from_str(&html[start..end]).expect("a dictionary that is not JSON")
}

/// The page carries its strings, in the language set, so its first paint asks
/// for nothing more, and a dictionary that cannot be fetched is never a blank
/// page followed by raw keys.
#[tokio::test]
async fn the_page_carries_its_strings_in_the_language_set() {
    let dir = super::TempDir::new("dictionary");
    std::fs::write(
        dir.join("index.html"),
        "<html><head><title>x</title></head><body></body></html>",
    )
    .unwrap();
    let mut config = Config::for_tests();
    config.frontend_dir = dir.to_path_buf();
    let app = serving(config).await;

    let english = embedded_dictionary(&app.text("/").await);
    assert_eq!(english["language"], "en");
    assert_eq!(english["strings"]["Dashboard"], "Dashboard");
    assert!(english["counts"].as_array().is_some_and(|counts| !counts.is_empty()));

    app.save_setting("ui_language", "ar").await.assert_ok();
    let arabic = embedded_dictionary(&app.text("/").await);
    assert_eq!(arabic["language"], "ar");
    assert_eq!(arabic["direction"], "rtl");
}

/// A string holding `</script>` would close the element early and run what
/// follows it as a script.
#[test]
fn a_string_cannot_close_the_dictionary_early() {
    let payload = serde_json::json!({ "strings": { "X": "</script><script>alert(1)</script>" } });

    let html = crate::with_dictionary("<html><head></head><body></body></html>", &payload);

    assert_eq!(html.matches("</script>").count(), 1, "{html}");
    assert_eq!(embedded_dictionary(&html), payload);
}
