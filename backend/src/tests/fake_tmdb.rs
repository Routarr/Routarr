//! An in-process TMDb stand-in.
//!
//! Answering slowly and out of order is what this fake is for: enrichment pairs
//! results with its input list, and a result that comes back out of order is
//! how a movie's metadata ends up filed against a series.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Recorded {
    /// Every path requested, in the order the fake saw them.
    pub paths: Vec<String>,
    /// When each of those arrived.
    pub arrivals: Vec<Instant>,
    /// The credential each request carried, as `(api_key query parameter,
    /// Authorization header)`: which way it travelled, and whether both did.
    pub credentials: Vec<(Option<String>, Option<String>)>,
}

#[derive(Clone)]
struct FakeState {
    recorded: Arc<Mutex<Recorded>>,
    /// Ids TMDb does not have: they answer 404.
    failing: Arc<Vec<i64>>,
    /// Ids TMDb fails on: they answer 500.
    erroring: Arc<Vec<i64>>,
    /// Ids that answer slowly, to force out-of-order completion.
    slow: Arc<Vec<i64>>,
    /// The `Retry-After` seconds the next item request is refused with, a 429
    /// answered once.
    throttle: Arc<Mutex<Option<u64>>>,
    /// The status every item request answers, as TMDb does while it is down.
    down: Option<u16>,
    /// Item requests wait until this turns true.
    open: tokio::sync::watch::Receiver<bool>,
}

/// A film TMDb says is in Cantonese, which it writes `cn`.
pub const CANTONESE: i64 = 1101;
/// A film TMDb says has no language, which it writes `xx`.
pub const NO_LANGUAGE: i64 = 1102;

pub struct FakeTmdb {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    opener: tokio::sync::watch::Sender<bool>,
    /// Serving until it is dropped.
    _server: super::Served,
}

impl FakeTmdb {
    pub async fn start() -> Self {
        Self::with(vec![], vec![]).await
    }

    /// `failing` answer 404 and `slow` answer after a delay.
    pub async fn with(failing: Vec<i64>, slow: Vec<i64>) -> Self {
        Self::build(failing, vec![], slow, None, None).await
    }

    /// A fake whose first item request is refused with a 429 asking for
    /// `seconds` of quiet, and which answers every request after it.
    pub async fn throttling_once(seconds: u64) -> Self {
        Self::build(vec![], vec![], vec![], Some(seconds), None).await
    }

    /// A fake whose every item request answers `status`.
    pub async fn down(status: u16) -> Self {
        Self::build(vec![], vec![], vec![], None, Some(status)).await
    }

    /// A fake that answers no item request until [`FakeTmdb::release`].
    pub async fn holding() -> Self {
        let fake = Self::start().await;
        fake.opener.send_replace(false);
        fake
    }

    pub fn release(&self) {
        self.opener.send_replace(true);
    }

    /// A fake that fails on `erroring` with a 500 and answers every other id.
    pub async fn erroring(erroring: Vec<i64>) -> Self {
        Self::build(vec![], erroring, vec![], None, None).await
    }

    async fn build(
        failing: Vec<i64>,
        erroring: Vec<i64>,
        slow: Vec<i64>,
        throttle: Option<u64>,
        down: Option<u16>,
    ) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let (opener, open) = tokio::sync::watch::channel(true);
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            failing: Arc::new(failing),
            erroring: Arc::new(erroring),
            slow: Arc::new(slow),
            throttle: Arc::new(Mutex::new(throttle)),
            down,
            open,
        };

        let app = Router::new()
            .route("/3/configuration", get(configuration))
            .route("/3/movie/{id}", get(movie))
            .route("/3/tv/{id}", get(tv))
            .with_state(state);

        let server = super::serve(app).await;

        Self { base_url: server.address.clone(), recorded, opener, _server: server }
    }

    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().expect("recorded lock")
    }
}

async fn configuration() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "images": {} }))
}

async fn movie(
    State(state): State<FakeState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(refusal) = record(&state, "movie", id, &query, &headers).await {
        return refusal;
    }
    if state.failing.contains(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if state.erroring.contains(&id) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Two ids answer TMDb's two codes outside ISO 639-1: `cn`, its Cantonese,
    // and `xx`, no language at all.
    let language = match id {
        CANTONESE => "cn",
        NO_LANGUAGE => "xx",
        _ => "ja",
    };
    let mut movie = serde_json::json!({
        "id": id,
        "title": format!("Movie {id}"),
        "genres": [{ "id": 16, "name": "Animation" }, { "id": 10751, "name": "Family" }],
        "original_language": language,
        // Leaves `origin_country` out, as some TMDb records do, so the client
        // falls back to `production_countries`.
        "production_countries": [{ "iso_3166_1": "JP", "name": "Japan" }],
        "status": "Released",
        "overview": "Un film.",
        "poster_path": "/p.jpg",
    });
    appended(
        &mut movie,
        &query,
        "keywords",
        serde_json::json!({ "keywords": [{ "id": 1, "name": "anime" }] }),
    );
    appended(
        &mut movie,
        &query,
        "release_dates",
        serde_json::json!({ "results": [
            { "iso_3166_1": "US", "release_dates": [{ "certification": "PG" }] },
            { "iso_3166_1": "FR", "release_dates": [{ "certification": "Tous publics" }] }
        ]}),
    );
    Json(movie).into_response()
}

/// Add `block` under `name` when the request's `append_to_response` names it,
/// as TMDb answers only the blocks it was asked for.
fn appended(
    body: &mut serde_json::Value,
    query: &HashMap<String, String>,
    name: &str,
    block: serde_json::Value,
) {
    let asked = query.get("append_to_response").map(String::as_str).unwrap_or_default();
    if asked.split(',').any(|appended| appended.trim() == name) {
        body[name] = block;
    }
}

async fn tv(
    State(state): State<FakeState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(refusal) = record(&state, "tv", id, &query, &headers).await {
        return refusal;
    }
    if state.failing.contains(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if state.erroring.contains(&id) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let mut series = serde_json::json!({
        "id": id,
        "name": format!("Series {id}"),
        "genres": [{ "id": 18, "name": "Drama" }],
        "original_language": "en",
        "origin_country": ["US"],
        "status": "Ended",
        "overview": "Une série.",
        "poster_path": "/s.jpg",
    });
    // TV keywords come back under `results`, not `keywords`.
    appended(
        &mut series,
        &query,
        "keywords",
        serde_json::json!({ "results": [{ "id": 2, "name": "documentary" }] }),
    );
    appended(
        &mut series,
        &query,
        "content_ratings",
        serde_json::json!({ "results": [{ "iso_3166_1": "US", "rating": "TV-14" }] }),
    );
    Json(series).into_response()
}

/// Write the request down, and hand back the 429 a throttling fake owes.
async fn record(
    state: &FakeState,
    kind: &str,
    id: i64,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
) -> Option<Response> {
    let appended = query.get("append_to_response").cloned().unwrap_or_default();
    {
        let mut recorded = state.recorded.lock().expect("lock");
        recorded.paths.push(format!("/{kind}/{id}?append_to_response={appended}"));
        recorded.arrivals.push(Instant::now());
        recorded.credentials.push((
            query.get("api_key").cloned(),
            headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string),
        ));
    }

    if let Some(status) = state.down {
        return Some(StatusCode::from_u16(status).unwrap().into_response());
    }
    let throttled = state.throttle.lock().expect("lock").take();
    if let Some(seconds) = throttled {
        let wait = [(header::RETRY_AFTER, seconds.to_string())];
        return Some((StatusCode::TOO_MANY_REQUESTS, wait).into_response());
    }
    if state.slow.contains(&id) {
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    let mut open = state.open.clone();
    let _ = open.wait_for(|open| *open).await;
    None
}
