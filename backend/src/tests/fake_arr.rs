//! An in-process Radarr/Sonarr stand-in.
//!
//! Pure helpers cover none of what an integration client does over the wire —
//! the auth header, the error mapping, the timeouts, the read-patch-write dance
//! Sonarr needs. Pointing the clients at a real socket exercises all of it
//! without reaching the network, and lets webhook tests run against a reachable
//! instance instead of waiting for a connection to time out.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// How long [`FakeArr::observing_concurrency`] holds a listing open.
const HOLD: std::time::Duration = std::time::Duration::from_millis(50);

/// What the fake recorded, so a test can assert on what the client actually sent.
#[derive(Debug, Default)]
pub struct Recorded {
    pub api_keys: Vec<String>,
    /// Bodies of every mutating request, in order.
    pub writes: Vec<serde_json::Value>,
    /// Query strings of every mutating request.
    pub query_strings: Vec<String>,
    /// Paths of every read, so a test can tell one item from the whole library.
    pub reads: Vec<String>,
}

#[derive(Clone)]
struct FakeState {
    recorded: Arc<Mutex<Recorded>>,
    /// Status code to answer mutating calls with, for failure-path tests.
    fail_with: Option<u16>,
    series_path: String,
    /// A body to answer `GET /api/v3/series/{id}` with, instead of a series.
    ///
    /// A reverse proxy, or a base URL pointing at the wrong service, answers
    /// 2xx with something that is not a series object — and the move path
    /// patches two fields into whatever came back.
    series_body: Arc<Mutex<Option<serde_json::Value>>>,
    /// Whether the movie has been downloaded yet. `false` is what Radarr reports
    /// between `MovieAdded` and the first import — the window auto-apply exists
    /// for.
    movie_has_file: bool,
    /// How long the library listing is held open before answering.
    ///
    /// Zero for every ordinary test. A non-zero hold is what makes overlap
    /// *observable*: sequential callers can never raise `max_in_flight` above
    /// one however long the hold is, because the second request is not issued
    /// until the first has returned. So this is not a race the test might lose
    /// — it only has to outlast the microseconds between issuing two requests.
    hold: std::time::Duration,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

pub struct FakeArr {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    series_body: Arc<Mutex<Option<serde_json::Value>>>,
    max_in_flight: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeArr {
    /// Start a fake that answers everything successfully.
    pub async fn start() -> Self {
        Self::with(None, "/tv/standard/Cowboy Bebop (1998)", true).await
    }

    /// Start a fake that rejects mutating calls with `status`.
    pub async fn failing(status: u16) -> Self {
        Self::with(Some(status), "/tv/standard/Cowboy Bebop (1998)", true).await
    }

    /// Start a fake whose series already lives at `series_path`.
    pub async fn with_series_path(series_path: &str) -> Self {
        Self::with(None, series_path, true).await
    }

    /// Start a fake that answers a single series with `body` and a 200.
    pub async fn answering_series_with(body: serde_json::Value) -> Self {
        let fake =
            Self::build(None, "/tv/standard/Cowboy Bebop (1998)", true, std::time::Duration::ZERO)
                .await;
        fake.series_body.lock().expect("lock").replace(body);
        fake
    }

    /// Start a fake whose movie has been added but not yet downloaded.
    pub async fn with_unimported_movie() -> Self {
        Self::with(None, "/tv/standard/Cowboy Bebop (1998)", false).await
    }

    /// Start a fake that holds each library listing open long enough for
    /// overlapping callers to be counted. See [`FakeArr::max_concurrent`].
    pub async fn observing_concurrency() -> Self {
        Self::build(None, "/tv/standard/Cowboy Bebop (1998)", true, HOLD).await
    }

    /// Start a fake that takes `hold` to perform each edit, so a caller can be
    /// observed — or dropped — while the Arr is still at work.
    pub async fn holding_edits(hold: std::time::Duration) -> Self {
        Self::build(None, "/tv/standard/Cowboy Bebop (1998)", true, hold).await
    }

    /// The most requests this fake ever had open at the same moment.
    ///
    /// One means the caller was sequential — not slow, sequential.
    pub fn max_concurrent(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    async fn with(fail_with: Option<u16>, series_path: &str, movie_has_file: bool) -> Self {
        Self::build(fail_with, series_path, movie_has_file, std::time::Duration::ZERO).await
    }

    async fn build(
        fail_with: Option<u16>,
        series_path: &str,
        movie_has_file: bool,
        hold: std::time::Duration,
    ) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let series_body: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            fail_with,
            series_path: series_path.to_string(),
            series_body: Arc::clone(&series_body),
            movie_has_file,
            hold,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::clone(&max_in_flight),
        };

        let app = Router::new()
            .route("/api/v3/system/status", get(system_status))
            .route("/api/v3/rootfolder", get(root_folders))
            .route("/api/v3/filesystem", get(filesystem))
            .route("/api/v3/tag", get(tags))
            .route("/api/v3/movie", get(movies))
            .route("/api/v3/movie/{id}", get(movie_one))
            .route("/api/v3/movie/editor", put(movie_editor))
            .route("/api/v3/series", get(series_list))
            .route("/api/v3/series/{id}", get(series_one).put(series_update))
            .route("/api/v3/command", post(command))
            .with_state(state);

        // Port 0: the OS picks a free one, so tests can run in parallel.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake arr");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self {
            base_url: format!("http://{addr}"),
            recorded,
            series_body,
            max_in_flight,
            shutdown: Some(tx),
        }
    }

    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().expect("recorded lock")
    }
}

impl Drop for FakeArr {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

fn record_read(state: &FakeState, path: &str) {
    state.recorded.lock().expect("lock").reads.push(path.to_string());
}

fn record_key(state: &FakeState, headers: &HeaderMap) {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        state.recorded.lock().expect("lock").api_keys.push(key.to_string());
    }
}

async fn system_status(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    Json(serde_json::json!({ "version": "5.2.6.8376", "appName": "Radarr" }))
}

async fn root_folders(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    Json(serde_json::json!([
        { "id": 1, "path": "/movies/standard", "freeSpace": 1024, "accessible": true },
        { "id": 2, "path": "/movies/anime", "freeSpace": 2048, "accessible": false },
        // A second *usable* destination. Without one, no test can produce a
        // move at all — the library sits in the only folder it could go to —
        // and an assertion that "nothing was written" passes for the wrong
        // reason.
        { "id": 3, "path": "/movies/kids", "freeSpace": 4096, "accessible": true },
    ]))
}

/// The Arr's own view of its filesystem, which is the only one that counts:
/// Routarr and the Arr run in different containers as often as not.
///
/// Anything under `/movies` is visible; nothing else is. That is enough to tell
/// a path the instance can reach from one it cannot.
async fn filesystem(
    State(state): State<FakeState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    // The real endpoint lists the *contents* of the directory it is given, and
    // answers about the nearest one above when the path does not exist — which
    // is why a prefix match here would let a misspelt leaf read as verified.
    let path = query.get("path").cloned().unwrap_or_default();
    let known = [
        "/movies",
        "/movies/standard",
        "/movies/anime",
        "/movies/anime/films",
        "/movies/anime/kids",
        "/movies/kids",
        "/tv",
        "/tv/standard",
        "/tv/anime",
    ];
    let asked = path.trim_end_matches('/');
    let children: Vec<serde_json::Value> = known
        .iter()
        .filter(|candidate| {
            candidate.rsplit_once('/').map(|(parent, _)| parent) == Some(asked)
                || (asked == "/" && candidate.matches('/').count() == 1)
        })
        .map(|candidate| serde_json::json!({ "type": "folder", "path": candidate }))
        .collect();
    Json(serde_json::json!({ "parent": null, "directories": children, "files": [] }))
}

async fn tags(State(state): State<FakeState>, headers: HeaderMap) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    Json(serde_json::json!([
        { "id": 1, "label": "anime" },
        { "id": 2, "label": "kids" },
    ]))
}

/// Count this request as open, hold it, and report the high-water mark.
///
/// Only the library listings are instrumented: they are the one call every
/// sync makes and the one that carries the payload, so they are where an
/// overlap between two instance syncs shows up.
async fn hold_and_count(state: &FakeState) {
    let now = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    state.max_in_flight.fetch_max(now, Ordering::SeqCst);
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    state.in_flight.fetch_sub(1, Ordering::SeqCst);
}

async fn movies(State(state): State<FakeState>, headers: HeaderMap) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/movie");
    hold_and_count(&state).await;
    Json(serde_json::json!([totoro(&state)]))
}

/// The one movie by id: Totoro is 10, anything else is unknown to this Radarr.
async fn movie_one(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record_key(&state, &headers);
    record_read(&state, &format!("/api/v3/movie/{id}"));
    if id != 10 {
        return Err(StatusCode::NOT_FOUND);
    }
    // Held like an edit: a webhook's read can be in flight while an apply
    // lands, and that overlap is what the `moved_at` gate exists for.
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    Ok(Json(totoro(&state)))
}

fn totoro(state: &FakeState) -> serde_json::Value {
    serde_json::json!({
        "id": 10,
        "title": "My Neighbor Totoro",
        "sortTitle": "my neighbor totoro",
        "year": 1988,
        "tmdbId": 8392,
        "imdbId": "tt0096283",
        "path": "/movies/standard/My Neighbor Totoro (1988)",
        "rootFolderPath": "/movies/standard",
        "monitored": true,
        "hasFile": state.movie_has_file,
        "status": "released",
        "added": "2026-08-20T10:00:00Z",
        "sizeOnDisk": 8_589_934_592i64,
        "tags": [1],
        // Metadata Radarr carries itself. The language arrives as a *name*,
        // never a code — the sync is what turns it into the `ja` a rule is
        // written against.
        "genres": ["Animation", "Family"],
        "originalLanguage": { "id": 8, "name": "Japanese" },
        "certification": "G"
    })
}

async fn movie_editor(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    record_key(&state, &headers);
    state.recorded.lock().expect("lock").writes.push(body);

    if let Some(status) = state.fail_with {
        return Err((
            StatusCode::from_u16(status).unwrap(),
            "upstream rejected the edit".to_string(),
        ));
    }
    // Recorded first, then held: a test can see the edit arrive before the
    // Arr is done with it.
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    Ok(Json(serde_json::json!([])))
}

async fn series_list(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/series");
    hold_and_count(&state).await;
    Json(serde_json::json!([bebop(&state, 20)]))
}

fn bebop(state: &FakeState, id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "titleSlug": "cowboy-bebop",
        "seasonFolder": true,
        "qualityProfileId": 3,
        "title": "Cowboy Bebop",
        "year": 1998,
        "tvdbId": 76885,
        "tmdbId": 30991,
        "path": state.series_path,
        "rootFolderPath": "/tv/standard",
        "monitored": true,
        "status": "ended",
        "added": "2026-01-01T00:00:00Z",
        "statistics": { "episodeFileCount": 26, "sizeOnDisk": 214_748_364_800i64 },
        "seriesType": "anime",
        // Season 0 is bonus material and must not be counted.
        "seasons": [
            { "seasonNumber": 0 },
            { "seasonNumber": 1 },
            { "seasonNumber": 2 }
        ],
        "tags": [1],
        "genres": ["Animation", "Action"],
        "originalLanguage": { "id": 8, "name": "Japanese" },
        "certification": "TV-14"
    })
}

async fn series_one(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, &format!("/api/v3/series/{id}"));
    if let Some(body) = state.series_body.lock().expect("lock").clone() {
        return Json(body);
    }
    Json(bebop(&state, id))
}

async fn series_update(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    record_key(&state, &headers);
    {
        let mut recorded = state.recorded.lock().expect("lock");
        recorded.writes.push(body.clone());
        recorded
            .query_strings
            .push(params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
    }

    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), "nope".to_string()));
    }
    Ok(Json(body))
}

async fn command(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    state.recorded.lock().expect("lock").writes.push(body);
    Json(serde_json::json!({ "id": 1, "status": "queued" }))
}
