//! An in-process TMDb stand-in.
//!
//! Answering slowly and out of order is what this fake is for: enrichment pairs
//! results with its input list, and a result that comes back out of order is
//! how a movie's metadata ends up filed against a series.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

#[derive(Debug, Default)]
pub struct Recorded {
    /// Every path requested, in the order the fake saw them.
    pub paths: Vec<String>,
}

#[derive(Clone)]
struct FakeState {
    recorded: Arc<Mutex<Recorded>>,
    /// Ids that answer with an error instead of a payload.
    failing: Arc<Vec<i64>>,
    /// Ids that answer slowly, to force out-of-order completion.
    slow: Arc<Vec<i64>>,
}

pub struct FakeTmdb {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeTmdb {
    pub async fn start() -> Self {
        Self::with(vec![], vec![]).await
    }

    /// `failing` answer 404; `slow` answer after a delay.
    pub async fn with(failing: Vec<i64>, slow: Vec<i64>) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            failing: Arc::new(failing),
            slow: Arc::new(slow),
        };

        let app = Router::new()
            .route("/3/configuration", get(configuration))
            .route("/3/movie/{id}", get(movie))
            .route("/3/tv/{id}", get(tv))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake tmdb");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self { base_url: format!("http://{addr}"), recorded, shutdown: Some(tx) }
    }

    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().expect("recorded lock")
    }
}

impl Drop for FakeTmdb {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn configuration() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "images": {} }))
}

async fn movie(
    State(state): State<FakeState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    record(&state, "movie", id, &query).await;
    if state.failing.contains(&id) {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "title": format!("Movie {id}"),
        "genres": [{ "id": 16, "name": "Animation" }, { "id": 10751, "name": "Family" }],
        "original_language": "ja",
        // Deliberately absent from the movie payload: the client must fall back
        // to production_countries.
        "production_countries": [{ "iso_3166_1": "JP", "name": "Japan" }],
        "status": "Released",
        "overview": "Un film.",
        "poster_path": "/p.jpg",
        "keywords": { "keywords": [{ "id": 1, "name": "anime" }] },
        "release_dates": { "results": [
            { "iso_3166_1": "US", "release_dates": [{ "certification": "PG" }] },
            { "iso_3166_1": "FR", "release_dates": [{ "certification": "Tous publics" }] }
        ]}
    })))
}

async fn tv(
    State(state): State<FakeState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    record(&state, "tv", id, &query).await;
    if state.failing.contains(&id) {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "name": format!("Series {id}"),
        "genres": [{ "id": 18, "name": "Drama" }],
        "original_language": "en",
        "origin_country": ["US"],
        "status": "Ended",
        "overview": "Une série.",
        "poster_path": "/s.jpg",
        // TV keywords come back under `results`, not `keywords`.
        "keywords": { "results": [{ "id": 2, "name": "documentary" }] },
        "content_ratings": { "results": [{ "iso_3166_1": "US", "rating": "TV-14" }] }
    })))
}

async fn record(state: &FakeState, kind: &str, id: i64, query: &HashMap<String, String>) {
    let appended = query.get("append_to_response").cloned().unwrap_or_default();
    state
        .recorded
        .lock()
        .expect("lock")
        .paths
        .push(format!("/{kind}/{id}?append_to_response={appended}"));

    if state.slow.contains(&id) {
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}
