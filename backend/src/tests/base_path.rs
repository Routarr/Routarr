//! Serving Routarr under a sub-path, the Servarr "URL base" (`https://host/routarr` behind a
//! reverse proxy), and what the server answers for the page and its static files.

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

/// A client parsing every failure as one shape must not fail on a 405.
#[tokio::test]
async fn a_method_a_route_does_not_take_answers_the_envelope_and_its_allow_header() {
    use axum::body::Body;
    use axum::http::Request;

    for (base, prefix) in [("", ""), ("/routarr", "/routarr")] {
        let app = mounted_at(base).await;
        let request = Request::patch(format!("{prefix}/api/v1/rules")).body(Body::empty()).unwrap();
        let response = app.send(request).await;

        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED, "{prefix}");
        assert_eq!(response.json["error"], "method_not_allowed", "{}", response.json);
        let allow = response.header("allow").unwrap_or_default();
        assert!(allow.contains("GET") && allow.contains("POST"), "allow: {allow}");
    }
}

/// Answered with `index.html` and a 200, a typo in an API path leaves a script parsing HTML as
/// JSON.
#[tokio::test]
async fn an_unknown_api_path_is_a_json_404_not_the_index() {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;

    // With the interface served, whose page answers every other path.
    let dir = super::TempDir::new("api-misses");
    std::fs::write(dir.join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    for (base, prefix) in [("", ""), ("/routarr", "/routarr")] {
        let mut config = Config::for_tests();
        config.base_path = normalise_base_path(base);
        config.frontend_dir = dir.to_path_buf();
        let app = serving(config).await;
        for (method, path) in [
            ("GET", "/api/v1/nope"),
            ("POST", "/api/v1/nope"),
            ("DELETE", "/api/v1/nope"),
            ("GET", "/api/v2/status"),
            ("GET", "/api/status"),
            ("POST", "/api/v1x/status"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(format!("{prefix}{path}"))
                .body(Body::empty())
                .unwrap();
            let response = app.send_raw(request).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {prefix}{path}");
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

/// A `Location` without it sends a client polling a path nothing answers.
#[tokio::test]
async fn a_task_not_waited_for_is_located_under_the_mount_point() {
    let app = mounted_at("/routarr").await;
    let request = axum::http::Request::post("/routarr/api/v1/simulate")
        .header("prefer", "respond-async")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let started = app.send(request).await;

    let job = started.assert_status(StatusCode::ACCEPTED)["task_id"].as_str().unwrap().to_string();
    let location = started.location().expect("no Location came back");
    assert_eq!(location, format!("/routarr/api/v1/tasks/{job}"));
    assert_eq!(status_of(&app, &location).await, StatusCode::OK);
}

#[tokio::test]
async fn nothing_answers_outside_the_mount_point() {
    let app = mounted_at("/routarr").await;

    // Answering at the root as well would hide a proxy misconfigured for the mount point.
    assert_eq!(status_of(&app, "/api/v1/ping").await, StatusCode::NOT_FOUND);
}

/// Typed by someone who forgot the mount point, the bare address would show an empty page.
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

/// Without a `<base href>`, a deep link like `/routarr/rules` resolves the relative asset URLs
/// against `/routarr/rules/`, and the page comes up blank.
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

    assert_eq!(status_of(&app, "/routarr/").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/routarr/rules").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/routarr/assets/app.js").await, StatusCode::OK);
    assert_eq!(status_of(&app, "/rules").await, StatusCode::NOT_FOUND);
}

/// A file under `assets/` is named after its content, so a browser keeps it
/// for a year rather than asking for every chunk on every load. The page that
/// names them is asked for every time, and so is a miss answered with it.
#[tokio::test]
async fn a_hashed_asset_is_kept_and_the_page_is_not() {
    let dir = super::TempDir::new("kept-assets");
    std::fs::write(dir.join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/app-3f2a.js"), "console.log(1)").unwrap();

    for base in ["", "/routarr"] {
        let mut config = Config::for_tests();
        config.base_path = normalise_base_path(base);
        config.frontend_dir = dir.to_path_buf();
        let app = serving(config).await;
        let kept = |path: String| {
            let app = &app;
            async move {
                let response = app.raw(&path).await;
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                response.headers()["cache-control"].to_str().unwrap().to_string()
            }
        };

        assert_eq!(
            kept(format!("{base}/assets/app-3f2a.js")).await,
            "public, max-age=31536000, immutable"
        );
        assert_eq!(kept(format!("{base}/")).await, "no-cache");
        assert_eq!(kept(format!("{base}/library")).await, "no-cache");
        // A chunk a tab opened before an upgrade asks for is gone, not the page.
        let gone = app.raw(&format!("{base}/assets/gone-1b2c.js")).await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND, "{base}");
    }
}

/// The third-party notices name authors with letters outside ASCII, which a browser handed
/// `text/plain` without a charset may garble.
#[tokio::test]
async fn a_text_file_the_build_wrote_is_read_as_utf8() {
    let dir = super::TempDir::new("utf8-files");
    std::fs::write(dir.join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    std::fs::write(dir.join("third-party-notices.txt"), "Copyright © 2015 Bartłomiej").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/app-3f2a.css"), "body{}").unwrap();
    std::fs::write(dir.join("favicon.svg"), "<svg/>").unwrap();

    for base in ["", "/routarr"] {
        let mut config = Config::for_tests();
        config.base_path = normalise_base_path(base);
        config.frontend_dir = dir.to_path_buf();
        let app = serving(config).await;
        let type_of = |path: String| {
            let app = &app;
            async move {
                let response = app.raw(&path).await;
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                response.headers()["content-type"].to_str().unwrap().to_string()
            }
        };

        assert_eq!(
            type_of(format!("{base}/third-party-notices.txt")).await,
            "text/plain; charset=utf-8"
        );
        assert_eq!(type_of(format!("{base}/assets/app-3f2a.css")).await, "text/css; charset=utf-8");
        assert_eq!(type_of(format!("{base}/")).await, "text/html; charset=utf-8");
        assert_eq!(type_of(format!("{base}/favicon.svg")).await, "image/svg+xml");
    }
}

/// Both change only with a release or the language, and are asked for again and again.
#[tokio::test]
async fn the_contract_and_the_dictionary_answer_a_client_holding_them_with_304() {
    use axum::body::Body;
    use axum::http::Request;

    let app = mounted_at("").await;
    for path in ["/api/v1/openapi.json", "/api/v1/localization"] {
        let first = app.raw(path).await;
        let tag = first.headers()["etag"].clone();
        assert_eq!(first.headers()["cache-control"], "no-cache", "{path}");
        // Weak: the compression layer answers the same tag for two encodings of one body.
        assert!(tag.to_str().unwrap().starts_with("W/\""), "{path}: {tag:?}");

        let again = Request::get(path).header("if-none-match", &tag).body(Body::empty()).unwrap();
        let again = app.send_raw(again).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "{path}");
    }

    let english = app.raw("/api/v1/localization").await.headers()["etag"].clone();
    app.save_setting("ui_language", "fr").await.assert_ok();
    let french = app.raw("/api/v1/localization").await.headers()["etag"].clone();
    assert_ne!(english, french, "a new language kept the tag of the old one");
}

/// The dictionary the page carries, between its script tags.
fn embedded_dictionary(html: &str) -> serde_json::Value {
    let open = r#"<script type="application/json" id="dictionary">"#;
    let start = html.find(open).expect("the page carries no dictionary") + open.len();
    let end = start + html[start..].find("</script>").expect("an unclosed dictionary");
    serde_json::from_str(&html[start..end]).expect("a dictionary that is not JSON")
}

/// The first paint then asks for nothing more, and a failed fetch never shows raw keys.
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
