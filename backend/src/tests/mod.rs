//! Integration tests driving the real router against an in-memory database.

mod api;
mod applications;
mod arr_clients;
mod arr_signals;
mod audit;
mod auto_apply;
mod automation;
mod backup;
mod base_path;
mod batch_apply;
mod categories;
mod config_bundle;
mod connection;
mod contract;
mod crypto_format;
mod enrichment;
mod executor;
mod extra_sources;
pub mod fake_arr;
pub mod fake_oidc;
pub mod fake_sources;
pub mod fake_tmdb;
mod limits;
mod live_sources;
mod localization;
mod metadata_sources;
mod metrics;
mod move_confirmation;
mod notify;
mod oidc;
mod onboarding;
mod outbound_http;
mod placement;
mod provider_keys;
pub(crate) mod races;
mod ratings;
mod routing;
mod rule_health;
mod rule_tests;
mod scale;
mod scheduler;
mod security;
mod sessions;
mod stopped_runs;
mod sync;
mod webhook_fuzz;
mod windows;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::services::routing::{SimulationOptions, run_simulation};
use crate::state::AppState;

/// A stand-in served on a port of the loopback the OS picks, so tests run side
/// by side, until the handle is dropped. Every stand-in that answers HTTP
/// starts here.
pub struct Served {
    /// `http://127.0.0.1:<port>`, with no trailing slash.
    pub address: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Read as its address wherever one is expected.
impl std::ops::Deref for Served {
    type Target = str;

    fn deref(&self) -> &str {
        &self.address
    }
}

impl std::fmt::Display for Served {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.address)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// A port bound and its address, for a stand-in that must know its own
/// address before it builds its routes, as an OIDC provider names itself.
pub async fn listen() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let address = format!("http://{}", listener.local_addr().expect("a local address"));
    (listener, address)
}

/// Serve `router` on `listener` until the returned handle is dropped.
pub fn serve_on(listener: tokio::net::TcpListener, router: axum::Router) -> Served {
    let address = format!("http://{}", listener.local_addr().expect("a local address"));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    Served { address, stop: Some(stop) }
}

/// Serve `router` on a fresh port until the returned handle is dropped.
pub async fn serve(router: axum::Router) -> Served {
    let (listener, _) = listen().await;
    serve_on(listener, router)
}

/// The HTTP client the application builds, under the test configuration.
pub fn http_client() -> reqwest::Client {
    crate::http::build_client(&crate::config::Config::for_tests()).expect("test http client")
}

/// Removed when dropped, so a test that fails halfway leaves nothing behind in
/// a temporary directory that, in the dev container, is a tmpfs.
pub(crate) struct TempDir(pub std::path::PathBuf);

impl TempDir {
    /// A fresh, empty directory. The label must be unique to the test, since
    /// the tests of one run share a process id and run in parallel, and a
    /// directory a killed run left behind is cleared first.
    pub fn new(label: &str) -> Self {
        let dir =
            Self(std::env::temp_dir().join(format!("routarr-{label}-{}", std::process::id())));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

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

/// A test harness holding the app state and a router built the same way `main`
/// builds it, so middleware and routing are exercised, not bypassed.
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
    /// configuration, and a test would then drive one set of credentials and
    /// assert on another.
    pub fn around(state: AppState) -> Self {
        Self { router: crate::build_router(state.clone()), state }
    }

    /// Same harness, but with an API key configured.
    pub async fn with_api_key(key: &str) -> Self {
        let mut config = crate::config::Config::for_tests();
        config.api_key = Some(key.to_string());
        // `for_tests` leaves the mode at `None` so the unauthenticated cases
        // stay testable, and a test that sets a key means to have it demanded.
        config.auth_mode = crate::config::AuthMode::ApiKey;

        Self::around(AppState::for_tests().await.with_config(config))
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
    /// `send` parses the body and discards it. A metrics scrape or a CSV export
    /// needs the bytes and the headers.
    pub async fn raw(&self, path: &str) -> axum::response::Response {
        self.send_raw(Request::get(path).body(Body::empty()).unwrap()).await
    }

    /// The raw response to any request, headers and body untouched.
    pub async fn send_raw(&self, request: Request<Body>) -> axum::response::Response {
        self.router.clone().oneshot(request).await.expect("router call")
    }

    /// The response body as text.
    pub async fn text(&self, path: &str) -> String {
        let bytes = self.raw(path).await.into_body().collect().await.expect("body").to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self.send_raw(request).await;
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
        sqlx::query(AN_INSTANCE).execute(&self.state.pool).await.unwrap();

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

        self.list_tmdb().await;
    }

    /// List TMDb after the Arr, for a library TMDb describes.
    ///
    /// Under the shipped order, the Arr alone, a TMDb answer in the cache counts
    /// for nothing. An order a test set first is its own.
    pub async fn list_tmdb(&self) {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('metadata_providers', 'arr,tmdb')
             ON CONFLICT(key) DO NOTHING",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// Point an instance at a running fake Arr, so sync and webhook paths can be
    /// exercised end to end. The first of a kind is `Fake radarr` or `Fake
    /// sonarr`, and each after it carries its id, names being unique.
    pub async fn seed_instance_at(&self, id: &str, kind: &str, base_url: &str) {
        let first = format!("Fake {kind}");
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM instances WHERE name = ?)")
                .bind(&first)
                .fetch_one(&self.state.pool)
                .await
                .unwrap();
        let name = if taken { format!("{first} {id}") } else { first };
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
             VALUES (?, ?, ?, ?, ?, 1, 'tok')",
        )
        .bind(id)
        .bind(name)
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

    /// Everything that routes TMDb's film 8392 to `/movies/anime` on `inst-1`
    /// but the film itself: the anime rule, the `anime` category mapped onto
    /// `rf-2`, and TMDb's answer for 8392, listed. Each test seeds the item
    /// with the path, the files and the id its case needs.
    pub async fn seed_route_to_anime(&self) {
        self.seed_anime_rule().await;
        sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime')")
            .execute(&self.state.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
             VALUES ('rf-2', 'inst-1', 2, '/movies/anime', 1, 'anime')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
             original_language, origin_countries, expires_at)
             VALUES ('tmdb', '8392', 'movie', '[\"Animation\"]', '[]', 'ja', '[]', '2099-01-01')",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
        self.list_tmdb().await;
    }

    /// `/movies/standard` on `inst-1`, where the films `seed_route_to_anime`
    /// moves sit, reported by the Arr as every root folder is. A revert writes
    /// back into it, and skips a folder its instance no longer has.
    pub async fn seed_standard_folder(&self) {
        sqlx::query(
            "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible)
             VALUES ('rf-standard', 'inst-1', 1, '/movies/standard', 1)",
        )
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// `arr` as the Radarr `inst-1`, the anime rule and the folder its films
    /// leave.
    async fn seed_routing_off(&self, arr: &fake_arr::FakeArr) {
        self.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
        self.seed_route_to_anime().await;
        self.seed_standard_folder().await;
    }

    /// The film `arr` holds as id 10, `m-1`, which the anime rule wants to move.
    /// `has_files` is what the last sync recorded, and the fake is what Radarr
    /// answers now: auto-apply writes only when both say there is no file, so
    /// `false` goes with [`fake_arr::FakeArr::with_unimported_movie`].
    pub async fn one_film_to_move(arr: &fake_arr::FakeArr, has_files: bool) -> Self {
        let app = Self::new().await;
        app.seed_one_film_to_move(arr, has_files).await;
        app
    }

    /// [`Self::one_film_to_move`], seeded into this harness.
    pub async fn seed_one_film_to_move(&self, arr: &fake_arr::FakeArr, has_files: bool) {
        self.seed_routing_off(arr).await;
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
             current_root_folder, monitored, has_files)
             VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro', 8392,
                     '/movies/standard/My Neighbor Totoro (1988)', '/movies/standard', 1, ?)",
        )
        .bind(has_files)
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// A library of `count` films the anime rule wants to move off `arr`,
    /// with the global dry run off, ready to apply. Film `n` is `m-n`, Arr id
    /// `100 + n`, in the folder `Film <its Arr id>`, as `arr` holds it.
    pub async fn films_to_move(arr: &fake_arr::FakeArr, count: usize) -> Self {
        let app = Self::new().await;
        app.seed_routing_off(arr).await;

        for index in 0..count {
            arr.hold_film(index as i64 + 100);
            sqlx::query(
                "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id,
                 current_path, current_root_folder, monitored, has_files)
                 VALUES (?, 'inst-1', ?, 'movie', ?, 8392, ?, '/movies/standard', 1, 1)",
            )
            .bind(format!("m-{index}"))
            .bind(index as i64 + 100)
            .bind(format!("Film {index:03}"))
            .bind(format!("/movies/standard/Film {}", index + 100))
            .execute(&app.state.pool)
            .await
            .unwrap();
        }

        app.store_setting("global_dry_run", "false").await;
        app
    }

    /// A harness whose library was synced from `arr`, as instance `inst-1`.
    pub async fn synced_from(kind: &str, arr: &fake_arr::FakeArr) -> Self {
        let app = Self::new().await;
        app.seed_instance_at("inst-1", kind, &arr.base_url).await;
        crate::services::sync::sync_instance(
            &app.state,
            "inst-1",
            &crate::jobs::Attribution::manual(None),
        )
        .await
        .unwrap();
        app
    }

    /// A rule whose only condition is `condition`, routing to `anime`, a
    /// category it creates when the library has none.
    pub async fn seed_rule_on(&self, condition: serde_json::Value) {
        sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-anime', 'anime')")
            .execute(&self.state.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
             target_category, match_mode)
             VALUES ('r-1', 'Rule under test', 10, 1, 'both', ?, 'anime', 'all')",
        )
        .bind(serde_json::json!([condition]).to_string())
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// The category the rules settle on for the first item, by a simulation
    /// that stores nothing.
    pub async fn decided_category(&self) -> String {
        let result = run_simulation(
            &self.state.pool,
            SimulationOptions { persist: false, ..Default::default() },
        )
        .await
        .unwrap();
        result.decisions[0].target_category.clone()
    }

    /// Run a persisting simulation and hand back its id.
    pub async fn simulate(&self) -> String {
        run_simulation(&self.state.pool, SimulationOptions { persist: true, ..Default::default() })
            .await
            .unwrap()
            .simulation_id
    }

    /// Write a setting straight into the table, as a hand edit or an older
    /// release leaves it: nothing validates or seals the value.
    /// Run each statement on the test database, in order.
    pub async fn execute(&self, statements: &[&'static str]) {
        for statement in statements {
            sqlx::query(*statement).execute(&self.state.pool).await.unwrap();
        }
    }

    /// The single number `sql` selects.
    pub async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.state.pool).await.unwrap()
    }

    /// The status of the latest job of `kind`, as the Tasks screen reads it.
    pub async fn last_job_status(&self, kind: &str) -> String {
        sqlx::query_scalar(
            "SELECT status FROM jobs WHERE kind = ? ORDER BY started_at DESC LIMIT 1",
        )
        .bind(kind)
        .fetch_one(&self.state.pool)
        .await
        .unwrap()
    }

    pub async fn store_setting(&self, key: &str, value: &str) {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.state.pool)
        .await
        .unwrap();
    }

    /// Save a setting as the Settings screen saves it, through `PUT /settings`,
    /// which validates the value and seals a credential.
    pub async fn save_setting(&self, key: &str, value: &str) -> TestResponse {
        self.put("/api/v1/settings", serde_json::json!({ "settings": { key: value } })).await
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

/// A library wired to a fake Radarr, with dry-run off and one pending move.
pub async fn one_move_ready(arr: &fake_arr::FakeArr) -> (TestApp, String) {
    let app = TestApp::one_film_to_move(arr, true).await;
    app.store_setting("global_dry_run", "false").await;

    let result = crate::services::routing::run_simulation(
        &app.state.pool,
        crate::services::routing::SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(result.moves_required, 1);
    let decision_id = result.decisions[0].id.clone();

    (app, decision_id)
}

/// Once every webhook delivery to `instance` has ended: none waiting, none
/// holding the instance.
pub async fn webhook_settled(app: &TestApp, instance: &str) {
    let key = format!("webhook:{instance}");
    let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while app.state.jobs.waiting_for(&key) > 0 || app.state.jobs.try_lock(&key).is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    settled.await.expect("a webhook delivery never ended");
}

/// A database as the release that shipped migration `last` hands it to an
/// upgrade. A test seeds it, then runs `db::run_migrations` as a start does.
pub async fn database_through(last: &str) -> sqlx::SqlitePool {
    let memory =
        <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str("sqlite::memory:")
            .unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(crate::db::with_paths(memory))
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=ON").execute(&pool).await.unwrap();
    crate::db::run_migrations_through(&pool, last).await.unwrap();
    pool
}

/// An application whose notification webhook accepts the connection and never
/// answers, with an outbound timeout long enough that a caller awaiting a
/// notification is seen waiting.
pub async fn waiting_on_a_silent_webhook() -> TestApp {
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = silent.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = silent.accept().await {
            held.push(socket);
        }
    });
    let app = TestApp::new().await.with_http_budget(std::time::Duration::from_secs(30));
    let hook = format!("http://{address}/hook");
    app.save_setting("notification_webhook_url", &hook).await.assert_ok();
    app
}

/// One enabled instance, written in the initial schema's terms, which every
/// later schema reads.
///
/// At port 1 on the loopback, where nothing listens: a request is refused at
/// once and never leaves the machine. A host name such as `radarr` goes through
/// the host's resolver, and on a homelab it may well answer.
pub const AN_INSTANCE: &str = "INSERT INTO instances
    (id, name, instance_type, base_url, api_key, enabled, webhook_token)
    VALUES ('inst-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'secret', 1, 'tok')";

/// A resolver that knows no name. A test about a name that does not resolve
/// asks it rather than the host's resolver, which may be slow, absent or, on a
/// homelab, answer.
pub struct NoNames;

impl reqwest::dns::Resolve for NoNames {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async { Err("no name resolves here".into()) })
    }
}

impl TestApp {
    /// The same harness, its outbound calls allowed `budget`, for a test
    /// holding one open longer than the 300 ms of `Config::for_tests`.
    pub fn with_http_budget(self, budget: std::time::Duration) -> Self {
        let mut config = (*self.state.config).clone();
        config.http_timeout = budget;
        let mut state = self.state.with_config(config.clone());
        state.http = crate::http::build_client(&config).unwrap();
        Self::around(state)
    }

    /// A harness whose applies follow the Arr's moves of files for `wait`.
    pub fn with_move_wait(self, wait: std::time::Duration) -> Self {
        let mut config = (*self.state.config).clone();
        config.move_wait = wait;
        Self::around(self.state.with_config(config))
    }

    /// A harness answering a webhook delivery within `wait`.
    pub fn with_webhook_answer_wait(self, wait: std::time::Duration) -> Self {
        let mut config = (*self.state.config).clone();
        config.webhook_answer_wait = wait;
        Self::around(self.state.with_config(config))
    }

    /// A harness whose client resolves no name.
    pub async fn resolving_nothing() -> Self {
        let mut state = AppState::for_tests().await;
        state.http =
            reqwest::Client::builder().dns_resolver(std::sync::Arc::new(NoNames)).build().unwrap();
        Self::around(state)
    }
}

/// Collects what a `tracing` subscriber writes, so a test can read the log a
/// request produced rather than trust that nothing sensitive is in it.
#[derive(Clone, Default)]
pub struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogCapture {
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A POST asking not to wait (`Prefer: respond-async`): it answers 202 once its
/// task has started.
pub fn preferring_async(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header("prefer", "respond-async")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// The task, once it has finished.
pub async fn finished(app: &TestApp, id: &str) -> serde_json::Value {
    for _ in 0..200 {
        let task = app.get(&format!("/api/v1/jobs/{id}")).await;
        if task.assert_ok()["status"] != "running" {
            return task.json;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the task {id} never finished");
}

/// The warnings of a `/status` or `/health` answer, as the reader sees them.
#[track_caller]
pub fn warning_messages(body: &serde_json::Value) -> Vec<String> {
    body["warnings"]
        .as_array()
        .expect("a list of warnings")
        .iter()
        .map(|warning| warning["message"].as_str().expect("a warning's message").to_string())
        .collect()
}
