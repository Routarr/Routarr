//! In-process stand-ins for the four sources added after TMDb.
//!
//! One server, four shapes: they are exercised together — a library enriched by
//! several sources at once is the whole point — and one listener keeps the test
//! setup to a single line. Everything is recorded, so a test can assert on what
//! actually went over the wire rather than on what the client meant to send.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Debug, Default)]
pub struct Recorded {
    /// Every path requested, in order.
    pub paths: Vec<String>,
    /// Search terms, in order, per source.
    pub searches: Vec<(&'static str, String)>,
    /// Credentials the client presented, per source.
    pub credentials: Vec<(&'static str, String)>,
}

#[derive(Clone)]
struct FakeState {
    recorded: Arc<Mutex<Recorded>>,
    /// When false, the search answers with a work whose year does not match.
    matching_year: bool,
    /// When set, every route answers with this status instead of a payload —
    /// the "the source is down" case the real Jikan produces whenever
    /// MyAnimeList is unavailable.
    fail_with: Option<u16>,
    /// The generation of the TheTVDB token. A login answers the current one;
    /// a read presenting an older one is refused, as TheTVDB refuses a token
    /// past its month.
    tvdb_token: Arc<Mutex<u32>>,
}

pub struct FakeSources {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    tvdb_token: Arc<Mutex<u32>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeSources {
    pub async fn start() -> Self {
        Self::with(true).await
    }

    /// A fake whose only candidate is the right title from the wrong year.
    pub async fn with_mismatched_year() -> Self {
        Self::with(false).await
    }

    /// A fake where every source answers `status`, whatever is asked.
    pub async fn failing(status: u16) -> Self {
        Self::build(true, Some(status)).await
    }

    async fn with(matching_year: bool) -> Self {
        Self::build(matching_year, None).await
    }

    async fn build(matching_year: bool, fail_with: Option<u16>) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let tvdb_token = Arc::new(Mutex::new(1));
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            matching_year,
            fail_with,
            tvdb_token: Arc::clone(&tvdb_token),
        };

        let app = Router::new()
            // AniList speaks GraphQL over a single endpoint.
            .route("/anilist", post(anilist))
            // Jikan.
            .route("/jikan/anime", get(jikan_search))
            .route("/jikan/anime/{id}/full", get(jikan_details))
            .route("/jikan/anime/{id}", get(jikan_details))
            // OMDb answers everything on its root.
            .route("/omdb", get(omdb))
            // TheTVDB: a login, then bearer-authenticated reads.
            .route("/tvdb/login", post(tvdb_login))
            .route("/tvdb/movies/{id}/extended", get(tvdb_record))
            .route("/tvdb/series/{id}/extended", get(tvdb_record))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake sources");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self { base_url: format!("http://{addr}"), recorded, tvdb_token, shutdown: Some(tx) }
    }

    /// What TheTVDB does after a month: the token every client holds stops
    /// working, and only a new login gets a working one.
    pub fn expire_tvdb_token(&self) {
        *self.tvdb_token.lock().expect("token") += 1;
    }

    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().expect("recorded lock")
    }

    pub fn anilist_url(&self) -> String {
        format!("{}/anilist", self.base_url)
    }
    pub fn jikan_url(&self) -> String {
        format!("{}/jikan", self.base_url)
    }
    pub fn omdb_url(&self) -> String {
        format!("{}/omdb", self.base_url)
    }
    pub fn tvdb_url(&self) -> String {
        format!("{}/tvdb", self.base_url)
    }
}

impl Drop for FakeSources {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

fn record(state: &FakeState, path: &str) {
    state.recorded.lock().expect("lock").paths.push(path.to_string());
}

// ------------------------------------------------------------------ AniList

async fn anilist(
    State(state): State<FakeState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    record(&state, "/anilist");
    let query = body["query"].as_str().unwrap_or_default();

    if query.contains("Page(") {
        let search = body["variables"]["search"].as_str().unwrap_or_default().to_string();
        state.recorded.lock().expect("lock").searches.push(("anilist", search));

        let year = if state.matching_year { 1988 } else { 1972 };
        return Json(serde_json::json!({
            "data": { "Page": { "media": [{
                "id": 523,
                "startDate": { "year": year },
                // The library says "My Neighbor Totoro"; AniList indexes the
                // romaji first. Matching has to survive that.
                "title": {
                    "romaji": "Tonari no Totoro",
                    "english": "My Neighbor Totoro",
                    "native": "となりのトトロ"
                },
                "synonyms": ["Totoro"]
            }] } }
        }));
    }

    Json(serde_json::json!({
        "data": { "Media": {
            "genres": ["Adventure", "Slice of Life"],
            "countryOfOrigin": "JP",
            "status": "FINISHED",
            "description": "Two sisters meet a forest spirit.",
            "isAdult": false,
            "tags": [
                { "name": "Iyashikei", "rank": 90 },
                { "name": "Rural", "rank": 75 },
                // Below the agreement floor: noise in a rule, and dropped.
                { "name": "Time Skip", "rank": 12 }
            ]
        } }
    }))
}

// -------------------------------------------------------------------- Jikan

async fn jikan_search(
    State(state): State<FakeState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record(&state, "/jikan/anime");
    if let Some(status) = state.fail_with {
        return Err(StatusCode::from_u16(status).unwrap());
    }
    let search = params.get("q").cloned().unwrap_or_default();
    state.recorded.lock().expect("lock").searches.push(("jikan", search));

    Ok(Json(serde_json::json!({
        "data": [{
            "mal_id": 523,
            "year": if state.matching_year { 1988 } else { 1972 },
            "title": "Tonari no Totoro",
            "title_english": "My Neighbor Totoro",
            "title_japanese": "となりのトトロ",
            "titles": [{ "title": "Totoro" }]
        }]
    })))
}

async fn jikan_details(
    State(state): State<FakeState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record(&state, &format!("/jikan/anime/{id}/full"));
    if let Some(status) = state.fail_with {
        return Err(StatusCode::from_u16(status).unwrap());
    }
    Ok(Json(serde_json::json!({
        "data": {
            "mal_id": 523,
            "genres": [{ "name": "Adventure" }],
            "themes": [{ "name": "Iyashikei" }],
            "demographics": [{ "name": "Kids" }],
            "rating": "G - All Ages",
            "status": "Finished Airing",
            "synopsis": "Two sisters meet a forest spirit."
        }
    })))
}

// --------------------------------------------------------------------- OMDb

async fn omdb(
    State(state): State<FakeState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record(&state, "/omdb");
    if let Some(key) = params.get("apikey") {
        state.recorded.lock().expect("lock").credentials.push(("omdb", key.clone()));
    }

    // OMDb answers 200 with Response=False for an unknown id.
    if params.get("i").map(String::as_str) == Some("tt0000000") {
        return Json(serde_json::json!({ "Response": "False", "Error": "Incorrect IMDb ID." }));
    }

    Json(serde_json::json!({
        "Response": "True",
        "Genre": "Animation, Family, Fantasy",
        // Names, not codes — the whole reason the client normalises.
        "Language": "Japanese, English",
        "Country": "Japan",
        "Rated": "G",
        "Plot": "Two sisters meet a forest spirit."
    }))
}

// ------------------------------------------------------------------ TheTVDB

async fn tvdb_login(
    State(state): State<FakeState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    record(&state, "/tvdb/login");
    let key = body["apikey"].as_str().unwrap_or_default().to_string();
    let pin = body["pin"].as_str().unwrap_or_default().to_string();
    state.recorded.lock().expect("lock").credentials.push(("tvdb", format!("{key}/{pin}")));

    let token = format!("tvdb-token-{}", *state.tvdb_token.lock().expect("token"));
    Json(serde_json::json!({ "status": "success", "data": { "token": token } }))
}

async fn tvdb_record(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record(&state, &format!("/tvdb/{id}/extended"));
    let expected = format!("Bearer tvdb-token-{}", *state.tvdb_token.lock().expect("token"));
    let presented = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if presented != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "status": "failure", "message": "Unauthorized" })),
        ));
    }
    Ok(Json(serde_json::json!({
        "data": {
            "genres": [{ "name": "Anime" }, { "name": "Animation" }],
            "originalCountry": "jpn",
            "originalLanguage": "jpn",
            "contentRatings": [
                { "name": "TV-14", "country": "usa" },
                { "name": "-12", "country": "fra" }
            ],
            "status": { "name": "Ended" },
            "overview": "A bounty hunter crew."
        }
    })))
}
