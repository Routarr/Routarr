//! Integration tests driving the real router against an in-memory database.

mod api;
mod arr_clients;
mod arr_signals;
mod auto_apply;
mod backup;
mod base_path;
mod batch_apply;
mod categories;
mod config_bundle;
mod crypto_format;
mod enrichment;
mod executor;
mod extra_sources;
pub mod fake_arr;
pub mod fake_oidc;
pub mod fake_sources;
pub mod fake_tmdb;
mod live_sources;
mod localization;
mod metadata_sources;
mod metrics;
mod notify;
mod outbound_http;
mod provider_keys;
mod routing;
mod rule_health;
mod rule_tests;
mod scale;
mod scheduler;
mod security;
mod sync;
mod webhook_fuzz;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::state::AppState;

/// A test harness holding the app state and a router built the same way `main`
/// builds it, so middleware and routing are exercised, not bypassed.
/// Removed when dropped, so a test that fails halfway leaves nothing behind in
/// a temporary directory that, in the dev container, is a tmpfs.
pub(crate) struct TempDir(pub std::path::PathBuf);

impl std::ops::Deref for TempDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for TempDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub struct TestApp {
    pub state: AppState,
    router: Router,
}

impl TestApp {
    pub async fn new() -> Self {
        let state = AppState::for_tests().await;
        Self::around(state)
    }

    /// Rebuild the harness around a modified state.
    ///
    /// The router captures the state it was built with, so replacing `state`
    /// on an existing harness leaves every HTTP route talking to the old
    /// configuration — a test would then drive one set of credentials and
    /// assert on another.
    pub fn around(state: AppState) -> Self {
        Self { router: crate::build_router(state.clone()), state }
    }

    /// Same harness, but with an API key configured.
    pub async fn with_api_key(key: &str) -> Self {
        let mut config = crate::config::Config::for_tests();
        config.api_key = Some(key.to_string());
        // `for_tests` leaves the mode at `None` so the unauthenticated cases
        // stay testable; a test that sets a key means to have it demanded.
        config.auth_mode = crate::config::AuthMode::ApiKey;

        let state = AppState::for_tests().await.with_config(config);

        Self { router: crate::build_router(state.clone()), state }
    }

    pub async fn get(&self, path: &str) -> TestResponse {
        self.send(Request::get(path).body(Body::empty()).unwrap()).await
    }

    pub async fn post(&self, path: &str, body: serde_json::Value) -> TestResponse {
        self.send(json_request("POST", path, body)).await
    }

    pub async fn put(&self, path: &str, body: serde_json::Value) -> TestResponse {
        self.send(json_request("PUT", path, body)).await
    }

    pub async fn delete(&self, path: &str) -> TestResponse {
        self.send(Request::delete(path).body(Body::empty()).unwrap()).await
    }

    /// The raw response, for anything that is not JSON.
    ///
    /// `send` parses the body and discards it; a metrics scrape or a CSV export
    /// needs the bytes and the headers.
    pub async fn raw(&self, path: &str) -> axum::response::Response {
        self.router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .expect("router call")
    }

    /// The response body as text.
    pub async fn text(&self, path: &str) -> String {
        let bytes = self.raw(path).await.into_body().collect().await.expect("body").to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self.router.clone().oneshot(request).await.expect("router call");
        let status = response.status();
        // Kept before the body is consumed: a redirect has no body worth
        // reading and everything it says is in this header.
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body().collect().await.expect("body").to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        TestResponse { status, json, location }
    }

    /// Seed a Radarr instance, a mapped root folder and one media item.
    pub async fn seed_library(&self) {
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
             VALUES ('inst-1', 'Radarr', 'radarr', 'http://radarr:7878', 'secret', 1, 'tok')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();

        sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime')")
            .execute(&self.state.pool)
            .await
            .unwrap();

        for (id, arr_id, path, category) in [
            ("rf-1", 1, "/movies/standard", Some("standard")),
            ("rf-2", 2, "/movies/anime", Some("anime")),
        ] {
            sqlx::query(
                "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
                 VALUES (?, 'inst-1', ?, ?, 1, ?)",
            )
            .bind(id)
            .bind(arr_id)
            .bind(path)
            .bind(category)
            .execute(&self.state.pool)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
             current_path, current_root_folder, monitored, has_files, status, added_at)
             VALUES ('m-1', 'inst-1', 10, 'movie', 'My Neighbor Totoro', 1988, 8392,
                     '/movies/standard/My Neighbor Totoro (1988)', '/movies/standard', 1, 1,
                     'released', '2026-08-20 10:00:00')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
             original_language, origin_countries, certification, expires_at)
             VALUES ('tmdb', '8392', 'movie', '[\"Animation\",\"Family\"]', '[\"anime\"]', 'ja',
                     '[\"JP\"]', 'G', '2099-01-01')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// Point an instance at a running fake Arr, so sync and webhook paths can be
    /// exercised end to end.
    pub async fn seed_instance_at(&self, id: &str, kind: &str, base_url: &str) {
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
             VALUES (?, ?, ?, ?, ?, 1, 'tok')",
        )
        .bind(id)
        .bind(format!("Fake {kind}"))
        .bind(kind)
        .bind(base_url)
        .bind(self.state.secrets.seal("arr-key").unwrap())
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// Seed the canonical anime rule.
    pub async fn seed_anime_rule(&self) {
        sqlx::query(
            "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
             target_category, match_mode)
             VALUES ('rule-anime', 'Anime', 10, 1, 'both',
                     '[{\"type\":\"original_language\",\"value\":[\"ja\"]},
                       {\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]',
                     'anime', 'all')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
    }
}

fn json_request(method: &str, path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub struct TestResponse {
    pub status: StatusCode,
    pub json: serde_json::Value,
    location: Option<String>,
}

impl TestResponse {
    /// Where a redirect points, which is the whole content of one.
    pub fn location(&self) -> Option<String> {
        self.location.clone()
    }
}

impl TestResponse {
    #[track_caller]
    pub fn assert_ok(&self) -> &serde_json::Value {
        assert!(self.status.is_success(), "expected success, got {}: {}", self.status, self.json);
        &self.json
    }

    #[track_caller]
    pub fn assert_status(&self, expected: StatusCode) -> &serde_json::Value {
        assert_eq!(self.status, expected, "body: {}", self.json);
        &self.json
    }

    pub fn message(&self) -> String {
        self.json.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string()
    }
}
