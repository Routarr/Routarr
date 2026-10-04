//! In-process stand-ins for AniList, Jikan, OMDb and TheTVDB.
//!
//! One server, four shapes: they are exercised together, since a library
//! enriched by several sources at once is the whole point, and one listener
//! keeps the test setup to a single line. Everything is recorded, so a test can
//! assert on what actually went over the wire rather than on what the client
//! meant to send.
//!
//! Each source holds one work, under the id the real one gives it, and answers
//! any other id as that source answers a miss. A client asking with the wrong
//! id, column or path then reads nothing, rather than the one work every id
//! would otherwise describe.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct Recorded {
    /// Every path requested, in order.
    pub paths: Vec<String>,
    /// Search terms, in order, per source.
    pub searches: Vec<(&'static str, String)>,
    /// Credentials the client presented, per source.
    pub credentials: Vec<(&'static str, String)>,
    /// The id each details read asked for, per source. TheTVDB's carries its
    /// kind, `movies/<id>` or `series/<id>`.
    pub details: Vec<(&'static str, String)>,
    /// The format each search was restricted to, per source: AniList's
    /// `format`, Jikan's `type`, `None` for an unrestricted search.
    pub formats: Vec<(&'static str, Option<String>)>,
    /// When each AniList search arrived.
    pub searched_at: Vec<std::time::Instant>,
}

/// The one work each source holds: My Neighbor Totoro, as AniList and
/// MyAnimeList number it, its IMDb id, and the TheTVDB id the test library
/// gives it. AniList and Jikan also hold their id 1, Cowboy Bebop, a series,
/// which a search restricted to films never answers, and which their probes
/// ask for.
pub const ANILIST_ID: i64 = 523;
pub const MAL_ID: &str = "523";
pub const IMDB_ID: &str = "tt0096283";
pub const TVDB_ID: &str = "76885";

#[derive(Clone)]
struct FakeState {
    recorded: Arc<Mutex<Recorded>>,
    /// When false, the search answers with a work whose year does not match.
    matching_year: bool,
    /// When set, every route of every source answers with this status instead
    /// of a payload, the "the source is down" case the real Jikan produces
    /// whenever MyAnimeList is unavailable.
    fail_with: Option<u16>,
    /// When set, AniList answers 200 with `data: null` and an `errors` list,
    /// which is how GraphQL reports a failure.
    graphql_error: bool,
    /// The generation of the TheTVDB token. A login answers the current one,
    /// and a read presenting an older one is refused, as TheTVDB refuses a token
    /// past its month.
    tvdb_token: Arc<Mutex<u32>>,
    /// Whether TheTVDB has revoked the key: a login with it is refused.
    tvdb_revoked: Arc<Mutex<bool>>,
    /// Whether a TheTVDB login answers a success that carries no token.
    tvdb_empty_token: Arc<Mutex<bool>>,
    /// Whether the OMDb key has spent its daily quota.
    omdb_spent: Arc<Mutex<bool>>,
    /// The `Retry-After` seconds the next AniList search is refused with.
    search_throttle: Arc<Mutex<Option<u64>>>,
}

/// The one key the OMDb stand-in accepts.
pub const OMDB_KEY: &str = "omdb-key";

pub struct FakeSources {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    tvdb_token: Arc<Mutex<u32>>,
    tvdb_revoked: Arc<Mutex<bool>>,
    tvdb_empty_token: Arc<Mutex<bool>>,
    omdb_spent: Arc<Mutex<bool>>,
    search_throttle: Arc<Mutex<Option<u64>>>,
    /// Serving until it is dropped.
    _server: super::Served,
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
        Self::build(true, Some(status), false).await
    }

    /// A fake whose AniList reports a failure the GraphQL way.
    pub async fn with_graphql_errors() -> Self {
        Self::build(true, None, true).await
    }

    async fn with(matching_year: bool) -> Self {
        Self::build(matching_year, None, false).await
    }

    async fn build(matching_year: bool, fail_with: Option<u16>, graphql_error: bool) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let tvdb_token = Arc::new(Mutex::new(1));
        let tvdb_revoked = Arc::new(Mutex::new(false));
        let omdb_spent = Arc::new(Mutex::new(false));
        let tvdb_empty_token = Arc::new(Mutex::new(false));
        let search_throttle = Arc::new(Mutex::new(None));
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            matching_year,
            fail_with,
            graphql_error,
            tvdb_token: Arc::clone(&tvdb_token),
            tvdb_revoked: Arc::clone(&tvdb_revoked),
            omdb_spent: Arc::clone(&omdb_spent),
            tvdb_empty_token: Arc::clone(&tvdb_empty_token),
            search_throttle: Arc::clone(&search_throttle),
        };

        let app = Router::new()
            // AniList speaks GraphQL over a single endpoint.
            .route("/anilist", post(anilist))
            .route("/jikan/anime", get(jikan_search))
            .route("/jikan/anime/{id}/full", get(jikan_details))
            .route("/jikan/anime/{id}", get(jikan_details))
            // OMDb answers everything on its root.
            .route("/omdb", get(omdb))
            // TheTVDB: a login, then bearer-authenticated reads.
            .route("/tvdb/login", post(tvdb_login))
            .route("/tvdb/genres", get(tvdb_genres))
            .route("/tvdb/{kind}/{id}/extended", get(tvdb_record))
            .with_state(state);

        let server = super::serve(app).await;

        Self {
            base_url: server.address.clone(),
            recorded,
            tvdb_token,
            tvdb_revoked,
            omdb_spent,
            tvdb_empty_token,
            search_throttle,
            _server: server,
        }
    }

    /// What TheTVDB does after a month: the token every client holds stops
    /// working, and only a new login gets a working one.
    pub fn expire_tvdb_token(&self) {
        *self.tvdb_token.lock().expect("token") += 1;
    }

    /// What AniList does to a client going too fast: the next search is refused
    /// with a 429 asking for `seconds` of quiet, and the ones after it answer.
    pub fn throttle_next_search(&self, seconds: u64) {
        *self.search_throttle.lock().expect("throttle") = Some(seconds);
    }

    /// What OMDb does once a key has made its thousand requests of the day:
    /// every request is refused until the next.
    pub fn spend_omdb_quota(&self) {
        *self.omdb_spent.lock().expect("spent") = true;
    }

    /// From now on a TheTVDB login succeeds without handing a token.
    pub fn answer_logins_without_a_token(&self) {
        *self.tvdb_empty_token.lock().expect("empty") = true;
    }

    /// What TheTVDB does to a revoked key: its tokens stop working, and a login
    /// with it is refused.
    pub fn revoke_tvdb_key(&self) {
        *self.tvdb_revoked.lock().expect("revoked") = true;
        self.expire_tvdb_token();
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

fn record(state: &FakeState, path: &str) {
    state.recorded.lock().expect("lock").paths.push(path.to_string());
}

fn record_details(state: &FakeState, source: &'static str, id: String) {
    state.recorded.lock().expect("lock").details.push((source, id));
}

// ------------------------------------------------------------------ AniList

/// A search refused for going too fast, as the next one is after
/// [`FakeSources::throttle_next_search`], or the answer.
async fn anilist(
    State(state): State<FakeState>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let searching = body["query"].as_str().unwrap_or_default().contains("Page(");
    let throttled = searching.then(|| state.search_throttle.lock().expect("throttle").take());
    if let Some(Some(seconds)) = throttled {
        record(&state, "/anilist");
        state.recorded.lock().expect("lock").searched_at.push(std::time::Instant::now());
        let mut refused = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "errors": [{ "message": "Too Many Requests." }] })),
        )
            .into_response();
        refused.headers_mut().insert("retry-after", seconds.to_string().parse().unwrap());
        return refused;
    }
    anilist_answer(state, body).await.into_response()
}

async fn anilist_answer(
    state: FakeState,
    body: serde_json::Value,
) -> (StatusCode, Json<serde_json::Value>) {
    record(&state, "/anilist");
    if let Some(status) = state.fail_with {
        return (StatusCode::from_u16(status).unwrap(), Json(serde_json::json!({})));
    }
    let query = body["query"].as_str().unwrap_or_default();

    if query.contains("Page(") {
        let search = body["variables"]["search"].as_str().unwrap_or_default().to_string();
        let format = body["variables"]["format"].as_str().map(str::to_string);
        {
            let mut recorded = state.recorded.lock().expect("lock");
            recorded.searches.push(("anilist", search));
            recorded.formats.push(("anilist", format.clone()));
            recorded.searched_at.push(std::time::Instant::now());
        }
        if state.graphql_error {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "data": null,
                    "errors": [{ "message": "Internal Server Error", "status": 500 }]
                })),
            );
        }

        let year = if state.matching_year { 1988 } else { 1972 };
        // The series first: a search for a film that is not restricted to
        // films finds it, and only the title then keeps it apart.
        let bebop = serde_json::json!({
            "id": 1,
            "format": "TV",
            "startDate": { "year": 1998 },
            "title": { "romaji": "Cowboy Bebop", "english": "Cowboy Bebop" },
            "synonyms": []
        });
        let series = if format.as_deref() == Some("MOVIE") { None } else { Some(bebop) };
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "data": { "Page": { "media": series.into_iter().chain([serde_json::json!({
                    "id": ANILIST_ID,
                    "format": "MOVIE",
                    "startDate": { "year": year },
                    // The library says "My Neighbor Totoro", and AniList indexes the
                    // romaji first. Matching has to survive that.
                    "title": {
                        "romaji": "Tonari no Totoro",
                        "english": "My Neighbor Totoro",
                        "native": "となりのトトロ"
                    },
                    "synonyms": ["Totoro"]
                })]).collect::<Vec<_>>() } }
            })),
        );
    }

    // The probe writes its id into the query, a details read passes it as a
    // variable.
    let id = &body["variables"]["id"];
    let id = id.as_i64().or_else(|| query.contains("Media(id: 1)").then_some(1));
    record_details(&state, "anilist", id.map_or_else(|| "null".into(), |id| id.to_string()));
    if id == Some(1) {
        return (StatusCode::OK, Json(serde_json::json!({ "data": { "Media": { "id": 1 } } })));
    }
    // AniList answers an id it does not hold with a 404 and `Media: null`.
    if id != Some(ANILIST_ID) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "data": { "Media": null },
                "errors": [{ "message": "Not Found.", "status": 404 }]
            })),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "data": { "Media": {
                "genres": ["Adventure", "Slice of Life"],
                "countryOfOrigin": "JP",
                "status": "FINISHED",
                "description": "Two sisters meet a forest spirit.",
                "tags": [
                    { "name": "Iyashikei", "rank": 90 },
                    { "name": "Rural", "rank": 75 },
                    // Below the agreement floor: noise in a rule, and dropped.
                    { "name": "Time Skip", "rank": 12 }
                ]
            } }
        })),
    )
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
    let restricted = params.get("type").cloned();
    {
        let mut recorded = state.recorded.lock().expect("lock");
        recorded.searches.push(("jikan", search));
        recorded.formats.push(("jikan", restricted.clone()));
    }

    // Jikan's `year` is the broadcast season, null for a film: the release
    // year is the start of `aired`. The series first, as AniList's.
    let released = if state.matching_year { 1988 } else { 1972 };
    let bebop = serde_json::json!({
        "mal_id": 1,
        "type": "TV",
        "year": 1998,
        "aired": {
            "from": "1998-04-03T00:00:00+00:00",
            "prop": { "from": { "day": 3, "month": 4, "year": 1998 } }
        },
        "title": "Cowboy Bebop",
        "title_english": "Cowboy Bebop",
        "titles": [{ "title": "Cowboy Bebop" }]
    });
    let series = if restricted.as_deref() == Some("movie") { None } else { Some(bebop) };
    Ok(Json(serde_json::json!({
        "data": series.into_iter().chain([serde_json::json!({
            "mal_id": 523,
            "type": "Movie",
            "year": null,
            "aired": {
                "from": format!("{released}-04-16T00:00:00+00:00"),
                "prop": { "from": { "day": 16, "month": 4, "year": released } }
            },
            "title": "Tonari no Totoro",
            "title_english": "My Neighbor Totoro",
            "title_japanese": "となりのトトロ",
            "titles": [{ "title": "Totoro" }]
        })]).collect::<Vec<_>>()
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
    record_details(&state, "jikan", id.clone());
    if id == "1" {
        return Ok(Json(serde_json::json!({ "data": { "mal_id": 1, "title": "Cowboy Bebop" } })));
    }
    if id != MAL_ID {
        return Err(StatusCode::NOT_FOUND);
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
) -> (StatusCode, Json<serde_json::Value>) {
    record(&state, "/omdb");
    if let Some(status) = state.fail_with {
        return (StatusCode::from_u16(status).unwrap(), Json(serde_json::json!({})));
    }
    let key = params.get("apikey").cloned().unwrap_or_default();
    state.recorded.lock().expect("lock").credentials.push(("omdb", key.clone()));
    // OMDb refuses a key it does not know, and one past its quota, with a 401
    // and the same body shape as a miss.
    let refusal = if key != OMDB_KEY {
        Some("Invalid API key!")
    } else if *state.omdb_spent.lock().expect("spent") {
        Some("Request limit reached!")
    } else {
        None
    };
    if let Some(error) = refusal {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "Response": "False", "Error": error })),
        );
    }

    let id = params.get("i").cloned().unwrap_or_default();
    record_details(&state, "omdb", id.clone());
    // OMDb answers 200 with Response=False for an id it does not hold.
    if id != IMDB_ID {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "Response": "False", "Error": "Incorrect IMDb ID." })),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "Response": "True",
            "Genre": "Animation, Family, Fantasy",
            // Names, not codes: the whole reason the client normalises.
            "Language": "Japanese, English",
            "Country": "Japan",
            "Rated": "G",
            "Plot": "Two sisters meet a forest spirit."
        })),
    )
}

// ------------------------------------------------------------------ TheTVDB

async fn tvdb_login(
    State(state): State<FakeState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record(&state, "/tvdb/login");
    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), Json(serde_json::json!({}))));
    }
    let key = body["apikey"].as_str().unwrap_or_default().to_string();
    let pin = body["pin"].as_str().unwrap_or_default().to_string();
    state.recorded.lock().expect("lock").credentials.push(("tvdb", format!("{key}/{pin}")));
    if *state.tvdb_revoked.lock().expect("revoked") {
        return Err(unauthorized());
    }

    let token = if *state.tvdb_empty_token.lock().expect("empty") {
        String::new()
    } else {
        format!("tvdb-token-{}", *state.tvdb_token.lock().expect("token"))
    };
    Ok(Json(serde_json::json!({ "status": "success", "data": { "token": token } })))
}

fn unauthorized() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "status": "failure", "message": "Unauthorized" })),
    )
}

/// Whether a read presents the token the last login handed out.
fn holds_the_token(state: &FakeState, headers: &HeaderMap) -> bool {
    let expected = format!("Bearer tvdb-token-{}", *state.tvdb_token.lock().expect("token"));
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(expected.as_str())
}

/// A light read, as a probe makes.
async fn tvdb_genres(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record(&state, "/tvdb/genres");
    if !holds_the_token(&state, &headers) {
        return Err(unauthorized());
    }
    Ok(Json(serde_json::json!({ "data": [{ "id": 27, "name": "Anime" }] })))
}

/// A film or a series, `kind` being `movies` or `series`. The one work is held
/// under both, so the library's film and its series read alike.
async fn tvdb_record(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record(&state, &format!("/tvdb/{id}/extended"));
    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), Json(serde_json::json!({}))));
    }
    if !holds_the_token(&state, &headers) {
        return Err(unauthorized());
    }
    record_details(&state, "tvdb", format!("{kind}/{id}"));
    if !matches!(kind.as_str(), "movies" | "series") || id != TVDB_ID {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "status": "failure", "message": "NotFoundException" })),
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
