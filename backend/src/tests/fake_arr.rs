//! An in-process Radarr/Sonarr stand-in.
//!
//! Pure helpers cover none of what an integration client does over the wire:
//! the auth header, the error mapping, the timeouts, the read-patch-write dance
//! Sonarr needs. Pointing the clients at a real socket exercises all of it
//! without reaching the network, and lets webhook tests run against a reachable
//! instance instead of waiting for a connection to time out.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// How long [`FakeArr::observing_concurrency`] holds a listing open.
const HOLD: std::time::Duration = std::time::Duration::from_millis(50);

/// Where a Windows Arr keeps what this fake keeps under `/`.
const WINDOWS_ROOT: &str = "D:\\Media";

/// A path of the library as this Arr writes it: unchanged on Linux, under
/// `D:\Media` with backslashes on Windows.
fn shown(state: &FakeState, path: &str) -> String {
    if state.windows {
        format!("{WINDOWS_ROOT}{}", path.replace('/', "\\"))
    } else {
        path.to_string()
    }
}

/// A root folder as this Arr writes it: a Windows Arr ends it with its
/// separator.
fn shown_folder(state: &FakeState, path: &str) -> String {
    if state.windows { format!("{}\\", shown(state, path)) } else { path.to_string() }
}

/// What the fake recorded, so a test can assert on what the client actually sent.
#[derive(Debug, Default)]
pub struct Recorded {
    pub api_keys: Vec<String>,
    /// The `Authorization` header of every request that carried one: the Basic
    /// credentials a proxy in front of an Arr asks for.
    pub authorizations: Vec<String>,
    /// Bodies of every mutating request, in order.
    pub writes: Vec<serde_json::Value>,
    /// Query strings of every mutating request.
    pub query_strings: Vec<String>,
    /// Paths of every read, so a test can tell one item from the whole library.
    pub reads: Vec<String>,
    /// The query of every listing of the films.
    pub listing_queries: Vec<HashMap<String, String>>,
}

/// The films this Radarr holds beside Totoro, and those it treats apart.
#[derive(Clone, Default)]
struct Films {
    /// Held as `Film <id>`, in `/movies/standard`.
    held: Vec<i64>,
    /// An update of these is refused.
    refused: Vec<i64>,
}

/// A move the Arr queued as a command of its own, as both Arrs carry files.
#[derive(Clone)]
struct QueuedMove {
    id: i64,
    /// `MoveMovie` or `MoveSeries`.
    name: &'static str,
    item: i64,
    source: String,
    destination: String,
    ends_at: std::time::Instant,
    /// Whether its end has been laid over the title.
    settled: bool,
}

/// How the moves the Arr queues end.
#[derive(Clone, Default)]
struct Moving {
    /// How long a queued move runs before it ends.
    lasts: std::time::Duration,
    /// Titles whose move fails on disk: the Arr puts the old path back and
    /// ends the command as completed, as Radarr's and Sonarr's move services do.
    rolled_back: Vec<i64>,
    /// Titles whose move command fails, leaving the new path recorded.
    failed: Vec<i64>,
    queued: Vec<QueuedMove>,
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
    /// 2xx with something that is not a series object, and the move path must
    /// refuse it rather than patch two fields into whatever came back.
    series_body: Arc<Mutex<Option<serde_json::Value>>>,
    /// Whether the movie has been downloaded yet. `false` is what Radarr reports
    /// between `MovieAdded` and the first import, the window auto-apply exists
    /// for.
    movie_has_file: bool,
    /// How long the library listing, a one-movie read and a movie edit are
    /// held open before answering.
    ///
    /// Zero for every ordinary test. A non-zero hold is what makes overlap
    /// *observable*: sequential callers can never raise `max_in_flight` above
    /// one however long the hold is, because the second request is not issued
    /// until the first has returned. So this is not a race the test might
    /// lose: it only has to outlast the microseconds between two requests.
    hold: std::time::Duration,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    /// Whether the tag catalogue answers a 500, as an Arr failing on it does.
    tags_broken: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the movie and series listings answer an empty list.
    no_titles: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the root folder listing answers an empty list.
    no_root_folders: Arc<std::sync::atomic::AtomicBool>,
    /// The series ids an update is refused for, as Sonarr refuses a path it
    /// cannot write.
    refused_series: Arc<Mutex<Vec<i64>>>,
    /// The films held beside Totoro.
    films: Arc<Mutex<Films>>,
    /// Fields each film now has in the Arr, by id, laid over its body.
    movie_edits: Arc<Mutex<HashMap<i64, serde_json::Map<String, serde_json::Value>>>>,
    /// Fields each series now has in the Arr, by id, laid over its body.
    series_edits: Arc<Mutex<HashMap<i64, serde_json::Map<String, serde_json::Value>>>>,
    /// The moves queued, and how they end.
    moving: Arc<Mutex<Moving>>,
    /// How long an update is answered after it was made.
    answers_late: Arc<Mutex<std::time::Duration>>,
    /// Root folders reported beside the usual three.
    more_root_folders: Arc<Mutex<Vec<serde_json::Value>>>,
    /// The country Radarr's metadata settings rate films for, as its
    /// `/config/metadata` answers it.
    certification_country: Arc<Mutex<String>>,
    /// Whether the Arr runs on Windows: every path it writes is under
    /// `D:\Media`, with backslashes, and a root folder ends with one, as a
    /// Windows Radarr or Sonarr writes it.
    windows: bool,
}

pub struct FakeArr {
    pub base_url: String,
    recorded: Arc<Mutex<Recorded>>,
    series_body: Arc<Mutex<Option<serde_json::Value>>>,
    max_in_flight: Arc<AtomicUsize>,
    tags_broken: Arc<std::sync::atomic::AtomicBool>,
    no_titles: Arc<std::sync::atomic::AtomicBool>,
    no_root_folders: Arc<std::sync::atomic::AtomicBool>,
    refused_series: Arc<Mutex<Vec<i64>>>,
    films: Arc<Mutex<Films>>,
    movie_edits: Arc<Mutex<HashMap<i64, serde_json::Map<String, serde_json::Value>>>>,
    series_edits: Arc<Mutex<HashMap<i64, serde_json::Map<String, serde_json::Value>>>>,
    moving: Arc<Mutex<Moving>>,
    answers_late: Arc<Mutex<std::time::Duration>>,
    more_root_folders: Arc<Mutex<Vec<serde_json::Value>>>,
    certification_country: Arc<Mutex<String>>,
    /// Serving until the fake is dropped.
    _server: super::Served,
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

    /// The same movie, not yet downloaded, on a fake that rejects mutating
    /// calls with `status`: an unattended apply the Arr refuses.
    pub async fn refusing_unimported_movie(status: u16) -> Self {
        Self::with(Some(status), "/tv/standard/Cowboy Bebop (1998)", false).await
    }

    /// Start a fake that holds each request open long enough for overlapping
    /// callers to be counted. See [`FakeArr::max_concurrent`].
    pub async fn observing_concurrency() -> Self {
        Self::holding_edits(HOLD).await
    }

    /// Start a fake that takes `hold` to perform each edit, so a caller can be
    /// observed, or dropped, while the Arr is still at work.
    pub async fn holding_edits(hold: std::time::Duration) -> Self {
        Self::build(None, "/tv/standard/Cowboy Bebop (1998)", true, hold).await
    }

    /// From now on the tag catalogue answers a 500.
    pub fn break_tag_endpoint(&self) {
        self.tags_broken.store(true, Ordering::SeqCst);
    }

    /// From now on the movie and series listings answer an empty list, as an
    /// Arr does while it restores, or behind a base URL pointing elsewhere.
    pub fn answer_no_titles(&self) {
        self.no_titles.store(true, Ordering::SeqCst);
    }

    /// From now on the root folder listing answers an empty list.
    pub fn answer_no_root_folders(&self) {
        self.no_root_folders.store(true, Ordering::SeqCst);
    }

    /// From now on Totoro has these fields, as an edit in the Arr leaves it.
    pub fn edit_movie(&self, edits: serde_json::Value) {
        lay_over(&self.movie_edits, 10, &edits);
    }

    /// From now on Cowboy Bebop has these fields, as an edit in the Arr leaves it.
    pub fn edit_series(&self, edits: serde_json::Value) {
        lay_over(&self.series_edits, 20, &edits);
    }

    /// From now on this Radarr holds `Film <id>` in `/movies/standard`.
    pub fn hold_film(&self, id: i64) {
        self.films.lock().expect("lock").held.push(id);
    }

    /// From now on an update is answered `late` after it was made, as an Arr
    /// whose answer is lost on the way back.
    pub fn answering_updates_after(&self, late: std::time::Duration) {
        *self.answers_late.lock().expect("lock") = late;
    }

    /// From now on each move queued runs for `lasts` before it ends.
    pub fn moving_for(&self, lasts: std::time::Duration) {
        self.moving.lock().expect("lock").lasts = lasts;
    }

    /// From now on a move of title `id` fails on disk, and the Arr puts its
    /// old path back.
    pub fn rolling_back(&self, id: i64) {
        self.moving.lock().expect("lock").rolled_back.push(id);
    }

    /// From now on the command moving title `id` fails, its new path kept.
    pub fn failing_the_move_of(&self, id: i64) {
        self.moving.lock().expect("lock").failed.push(id);
    }

    /// From now on the root folder listing reports `folder` as well.
    pub fn report_root_folder(&self, folder: serde_json::Value) {
        self.more_root_folders.lock().expect("lock").push(folder);
    }

    /// From now on series `id` is held, and an update of it refused.
    pub fn refuse_series(&self, id: i64) {
        self.refused_series.lock().expect("lock").push(id);
    }

    /// From now on an update of movie `id` is refused.
    pub fn refuse_movie(&self, id: i64) {
        self.films.lock().expect("lock").refused.push(id);
    }

    /// From now on movie `id` is gone from the library.
    pub fn forget_movie(&self, id: i64) {
        self.films.lock().expect("lock").held.retain(|held| *held != id);
    }

    /// The most requests this fake ever had open at the same moment.
    ///
    /// Rate films for `country` in the metadata settings, as Radarr writes a
    /// country there (`gb`).
    pub fn rate_for(&self, country: &str) {
        *self.certification_country.lock().expect("lock") = country.to_string();
    }

    /// One means the caller was sequential: not slow, sequential.
    pub fn max_concurrent(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    /// Start a fake Arr running on Windows, holding the same library and
    /// folders under `D:\Media`, with a share at `\\nas\films` it can see.
    pub async fn on_windows() -> Self {
        Self::build_on(
            None,
            "/tv/standard/Cowboy Bebop (1998)",
            true,
            std::time::Duration::ZERO,
            true,
        )
        .await
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
        Self::build_on(fail_with, series_path, movie_has_file, hold, false).await
    }

    async fn build_on(
        fail_with: Option<u16>,
        series_path: &str,
        movie_has_file: bool,
        hold: std::time::Duration,
        windows: bool,
    ) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let series_body: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let tags_broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let no_titles = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let no_root_folders = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let refused_series: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
        let films = Arc::new(Mutex::new(Films::default()));
        let movie_edits = Arc::new(Mutex::new(HashMap::new()));
        let series_edits = Arc::new(Mutex::new(HashMap::new()));
        let moving = Arc::new(Mutex::new(Moving::default()));
        let answers_late = Arc::new(Mutex::new(std::time::Duration::ZERO));
        let more_root_folders: Arc<Mutex<Vec<serde_json::Value>>> =
            Arc::new(Mutex::new(Vec::new()));
        let certification_country = Arc::new(Mutex::new("us".to_string()));
        let state = FakeState {
            recorded: Arc::clone(&recorded),
            fail_with,
            series_path: series_path.to_string(),
            series_body: Arc::clone(&series_body),
            movie_has_file,
            hold,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::clone(&max_in_flight),
            tags_broken: Arc::clone(&tags_broken),
            no_titles: Arc::clone(&no_titles),
            no_root_folders: Arc::clone(&no_root_folders),
            refused_series: Arc::clone(&refused_series),
            films: Arc::clone(&films),
            movie_edits: Arc::clone(&movie_edits),
            series_edits: Arc::clone(&series_edits),
            moving: Arc::clone(&moving),
            answers_late: Arc::clone(&answers_late),
            more_root_folders: Arc::clone(&more_root_folders),
            certification_country: Arc::clone(&certification_country),
            windows,
        };

        let app = Router::new()
            .route("/api/v3/system/status", get(system_status))
            .route("/api/v3/rootfolder", get(root_folders))
            .route("/api/v3/config/metadata", get(metadata_config))
            .route("/api/v3/filesystem", get(filesystem))
            .route("/api/v3/tag", get(tags))
            .route("/api/v3/movie", get(movies))
            .route("/api/v3/movie/lookup/tmdb", get(movie_lookup_tmdb))
            .route("/api/v3/movie/lookup/imdb", get(movie_lookup_imdb))
            .route("/api/v3/movie/{id}", get(movie_one).put(movie_update))
            .route("/api/v3/series/lookup", get(series_lookup))
            .route("/api/v3/series", get(series_list))
            .route("/api/v3/series/{id}", get(series_one).put(series_update))
            .route("/api/v3/command", get(commands).post(command))
            .layer(axum::middleware::from_fn(refuse_without_a_key))
            .with_state(state);

        let server = super::serve(app).await;

        Self {
            base_url: server.address.clone(),
            recorded,
            series_body,
            max_in_flight,
            tags_broken,
            no_titles,
            no_root_folders,
            refused_series,
            films,
            movie_edits,
            series_edits,
            moving,
            answers_late,
            more_root_folders,
            certification_country,
            _server: server,
        }
    }

    pub fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.recorded.lock().expect("recorded lock")
    }
}

/// A request carrying no `X-Api-Key` is refused, as Radarr and Sonarr refuse
/// it: a client that drops the header from a write fails here as it would
/// there.
async fn refuse_without_a_key(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if request.headers().contains_key("x-api-key") {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn record_read(state: &FakeState, path: &str) {
    state.recorded.lock().expect("lock").reads.push(path.to_string());
}

fn record_key(state: &FakeState, headers: &HeaderMap) {
    let mut recorded = state.recorded.lock().expect("lock");
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        recorded.api_keys.push(key.to_string());
    }
    if let Some(credentials) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        recorded.authorizations.push(credentials.to_string());
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
    if state.no_root_folders.load(Ordering::SeqCst) {
        return Json(serde_json::json!([]));
    }
    let mut folders = serde_json::json!([
        { "id": 1, "path": shown_folder(&state, "/movies/standard"), "freeSpace": 1024, "accessible": true },
        { "id": 2, "path": shown_folder(&state, "/movies/anime"), "freeSpace": 2048, "accessible": false },
        // A second *usable* destination. Without one, no test can produce a
        // move at all, since the library sits in the only folder it could go
        // to, and an assertion that "nothing was written" passes for the wrong
        // reason.
        { "id": 3, "path": shown_folder(&state, "/movies/kids"), "freeSpace": 4096, "accessible": true },
    ]);
    let listed = folders.as_array_mut().expect("a list");
    listed.extend(state.more_root_folders.lock().expect("lock").iter().cloned());
    Json(folders)
}

/// The Arr's own view of its filesystem, which is the only one that counts:
/// Routarr and the Arr run in different containers as often as not.
///
/// The folders listed below are all the instance can see, which is enough to
/// tell a path it can reach from one it cannot.
///
/// Answered the way Radarr's and Sonarr's `FileSystemLookupService.LookupContents`
/// answers when `allowFoldersWithoutTrailingSlashes` is left at its default:
/// the query is cut after its last separator and what remains is listed, each
/// directory's path ending in a separator. Asked `/movies/anime`, it lists
/// `/movies/`, and asked `/movies`, it lists `/`. Listing exactly the path given is
/// the behaviour a client would wish for and no Arr has, and a fake with it
/// passes a client that asks every real Arr about the wrong directory.
async fn filesystem(
    State(state): State<FakeState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    let asked =
        query.get("path").filter(|path| !path.trim().is_empty()).map_or("/", String::as_str);
    // Windows compares names without their case, and both Arrs cut at
    // either separator there.
    let separators: &[char] = if state.windows { &['\\', '/'] } else { &['/'] };
    let Some(cut) = asked.rfind(separators) else {
        return Json(serde_json::json!({ "parent": null, "directories": [], "files": [] }));
    };
    let listed = asked[..cut].to_string();
    let mut known: Vec<String> = [
        "/movies",
        "/movies/standard",
        "/movies/anime",
        "/movies/anime/films",
        "/movies/anime/kids",
        "/movies/kids",
        "/movies/Kids & Family+",
        "/tv",
        "/tv/standard",
        "/tv/anime",
    ]
    .iter()
    .map(|path| shown(&state, path))
    .collect();
    if state.windows {
        known.extend(["\\\\nas\\films".to_string(), "\\\\nas\\films\\4k".to_string()]);
    }
    let same = |a: &str, b: &str| {
        if state.windows { a.replace('/', "\\").to_lowercase() == b.to_lowercase() } else { a == b }
    };
    let children: Vec<serde_json::Value> = known
        .iter()
        .filter_map(|candidate| {
            let (parent, name) = candidate.rsplit_once(separators)?;
            let ends = if state.windows { '\\' } else { '/' };
            same(&listed, parent).then(|| {
                serde_json::json!({ "type": "folder", "name": name, "path": format!("{candidate}{ends}") })
            })
        })
        .collect();
    Json(serde_json::json!({ "parent": null, "directories": children, "files": [] }))
}

/// Radarr's metadata settings, of which only the certification country
/// matters here.
async fn metadata_config(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    let country = state.certification_country.lock().expect("lock").clone();
    Json(serde_json::json!({ "id": 1, "certificationCountry": country }))
}

async fn tags(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record_key(&state, &headers);
    if state.tags_broken.load(Ordering::SeqCst) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(serde_json::json!([
        { "id": 1, "label": "anime" },
        { "id": 2, "label": "kids" },
    ])))
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

/// The library, or with `tmdbId` the one film it holds under that id, as
/// Radarr filters its list.
async fn movies(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/movie");
    state.recorded.lock().expect("lock").listing_queries.push(query.clone());
    settle_moves(&state);
    hold_and_count(&state).await;
    if state.no_titles.load(Ordering::SeqCst) {
        return Json(serde_json::json!([]));
    }
    let movie = totoro(&state);
    match query.get("tmdbId") {
        Some(asked) if asked.parse::<i64>().ok() != movie["tmdbId"].as_i64() => {
            Json(serde_json::json!([]))
        }
        _ => Json(serde_json::json!([movie])),
    }
}

/// The one movie by id: Totoro is 10, a film [`FakeArr::hold_film`] names is
/// held as well, and anything else is unknown to this Radarr.
async fn movie_one(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record_key(&state, &headers);
    record_read(&state, &format!("/api/v3/movie/{id}"));
    settle_moves(&state);
    let movie = movie_by_id(&state, id).ok_or(StatusCode::NOT_FOUND)?;
    // Held like an edit: a webhook's read can be in flight while an apply
    // lands, and that overlap is what the `moved_at` gate exists for.
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    Ok(Json(movie))
}

fn movie_by_id(state: &FakeState, id: i64) -> Option<serde_json::Value> {
    if id == 10 {
        return Some(totoro(state));
    }
    if !state.films.lock().expect("lock").held.contains(&id) {
        return None;
    }
    let mut film = serde_json::json!({
        "id": id,
        "title": format!("Film {id}"),
        "year": 2001,
        "tmdbId": 8392,
        "path": shown(state, &format!("/movies/standard/Film {id}")),
        "rootFolderPath": shown(state, "/movies/standard"),
        "monitored": true,
        "hasFile": true,
        "status": "released",
        "tags": [],
    });
    laid_over(&state.movie_edits, id, &mut film);
    Some(film)
}

fn totoro(state: &FakeState) -> serde_json::Value {
    let mut movie = serde_json::json!({
        "id": 10,
        "title": "My Neighbor Totoro",
        "sortTitle": "my neighbor totoro",
        "year": 1988,
        "tmdbId": 8392,
        "imdbId": "tt0096283",
        "path": shown(state, "/movies/standard/My Neighbor Totoro (1988)"),
        "rootFolderPath": shown(state, "/movies/standard"),
        "monitored": true,
        "hasFile": state.movie_has_file,
        "status": "released",
        "added": "2026-08-20T10:00:00Z",
        "sizeOnDisk": 8_589_934_592i64,
        "tags": [1],
        // Metadata Radarr carries itself. The language arrives as a *name*,
        // never a code: the sync is what turns it into the `ja` a rule is
        // written against.
        "genres": ["Animation", "Family"],
        "originalLanguage": { "id": 8, "name": "Japanese" },
        "certification": "G"
    });
    laid_over(&state.movie_edits, 10, &mut movie);
    movie
}

type Edits = Arc<Mutex<HashMap<i64, serde_json::Map<String, serde_json::Value>>>>;

/// Record `edits` as fields title `id` now has.
fn lay_over(held: &Edits, id: i64, edits: &serde_json::Value) {
    let mut held = held.lock().expect("lock");
    let fields = held.entry(id).or_default();
    for (field, value) in edits.as_object().expect("an object of fields") {
        fields.insert(field.clone(), value.clone());
    }
}

/// Title `id`'s body with the fields it now has laid over it.
fn laid_over(held: &Edits, id: i64, body: &mut serde_json::Value) {
    for (field, value) in held.lock().expect("lock").get(&id).into_iter().flatten() {
        body[field] = value.clone();
    }
}

/// A film by its TMDb id, as Radarr's lookup answers: built afresh from TMDb,
/// so with no id, no folder and no file even when the library holds it.
/// Holding is what `GET /api/v3/movie?tmdbId=` tells. A miss answers 500
/// with the exception Radarr throws (`MovieNotFoundException`), not 404.
async fn movie_lookup_tmdb(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record_key(&state, &headers);
    let asked = query.get("tmdbId").cloned().unwrap_or_default();
    record_read(&state, &format!("/api/v3/movie/lookup/{asked}"));
    match asked.as_str() {
        "8392" => Ok(Json(as_looked_up(totoro(&state)))),
        "129" => Ok(Json(spirited_away())),
        _ => Err(movie_not_found("tmdbId", &asked)),
    }
}

/// The same by IMDb id, on its own path: an IMDb id sent to the TMDb path
/// finds nothing, as in Radarr.
async fn movie_lookup_imdb(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    record_key(&state, &headers);
    let asked = query.get("imdbId").cloned().unwrap_or_default();
    record_read(&state, &format!("/api/v3/movie/lookup/{asked}"));
    match asked.as_str() {
        "tt0096283" => Ok(Json(as_looked_up(totoro(&state)))),
        "tt0245429" => Ok(Json(spirited_away())),
        _ => Err(movie_not_found("IMDBId", &asked)),
    }
}

/// A held film as the lookup answers it: what TMDb says, none of the
/// library's own fields.
fn as_looked_up(mut movie: serde_json::Value) -> serde_json::Value {
    let fields = movie.as_object_mut().expect("an object");
    for library_only in ["id", "path", "rootFolderPath", "hasFile", "sizeOnDisk", "tags", "added"] {
        fields.remove(library_only);
    }
    fields.insert("monitored".into(), serde_json::json!(false));
    movie
}

fn movie_not_found(kind: &str, asked: &str) -> (StatusCode, Json<serde_json::Value>) {
    let message =
        format!("Movie with {kind} {asked} was not found, it may have been removed from TMDb.");
    let description = format!("NzbDrone.Core.Exceptions.MovieNotFoundException: {message}");
    let body = serde_json::json!({ "message": message, "description": description });
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body))
}

/// A movie Radarr knows and does not hold: no id, no folder, and no `hasFile`,
/// which Radarr's lookup leaves out.
fn spirited_away() -> serde_json::Value {
    serde_json::json!({
        "title": "Spirited Away",
        "sortTitle": "spirited away",
        "year": 2001,
        "tmdbId": 129,
        "imdbId": "tt0245429",
        "monitored": false,
        "status": "released",
        "tags": [],
        "genres": ["Animation", "Family", "Fantasy"],
        "originalLanguage": { "id": 8, "name": "Japanese" },
        "certification": "PG"
    })
}

/// Series by a term, as Sonarr's lookup answers: Cowboy Bebop by its TheTVDB
/// id, which the library holds, and nothing for any other term.
async fn series_lookup(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    let term = query.get("term").cloned().unwrap_or_default();
    record_read(&state, &format!("/api/v3/series/lookup/{term}"));
    match term.as_str() {
        "tvdb:76885" => Json(serde_json::json!([bebop(&state, 20)])),
        "tvdb:81178" | "imdb:tt0807832" => Json(serde_json::json!([mushishi()])),
        _ => Json(serde_json::json!([])),
    }
}

/// What a refused edit carries: a reason, then as much detail as an Arr's
/// validation list, longer than any error message may keep.
fn refusal() -> String {
    format!("upstream rejected the edit: {}", "a folder the Arr cannot write to, ".repeat(30))
}

/// An update of one film, as Radarr takes it: the record as sent, and with
/// `moveFiles` a command queued to carry the files to the path it names.
async fn movie_update(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    record_key(&state, &headers);
    record_write(&state, &body, &params);

    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), refusal()));
    }
    if state.films.lock().expect("lock").refused.contains(&id) {
        return Err((StatusCode::BAD_REQUEST, refusal()));
    }
    let Some(held) = movie_by_id(&state, id) else {
        return Err((StatusCode::NOT_FOUND, "Movie not found".to_string()));
    };
    // Recorded first, then held: a test can see the edit arrive before the
    // Arr is done with it.
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    take_update(&state, &state.movie_edits, "MoveMovie", id, &held, &body, &params);
    let late = *state.answers_late.lock().expect("lock");
    tokio::time::sleep(late).await;
    // 202, as Radarr's movie update answers.
    Ok((StatusCode::ACCEPTED, Json(body)))
}

fn record_write(state: &FakeState, body: &serde_json::Value, params: &HashMap<String, String>) {
    let mut recorded = state.recorded.lock().expect("lock");
    recorded.writes.push(body.clone());
    recorded
        .query_strings
        .push(params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
}

/// What an update leaves in the Arr: the path written at once, as both Arrs
/// write it before the files move, and with `moveFiles` a move queued from
/// the folder the title had.
fn take_update(
    state: &FakeState,
    edits: &Edits,
    command: &'static str,
    id: i64,
    held: &serde_json::Value,
    body: &serde_json::Value,
    params: &HashMap<String, String>,
) {
    if params.get("moveFiles").map(String::as_str) == Some("true") {
        let mut moving = state.moving.lock().expect("lock");
        let next = moving.queued.len() as i64 + 1;
        let ends_at = std::time::Instant::now() + moving.lasts;
        moving.queued.push(QueuedMove {
            id: next,
            name: command,
            item: id,
            source: held["path"].as_str().unwrap_or_default().to_string(),
            destination: body["path"].as_str().unwrap_or_default().to_string(),
            ends_at,
            settled: false,
        });
    }
    lay_over(
        edits,
        id,
        &serde_json::json!({ "path": body["path"], "rootFolderPath": body["rootFolderPath"] }),
    );
}

/// Lay the end of every move that has run its course over its title: one
/// that failed on disk puts the old path back.
fn settle_moves(state: &FakeState) {
    let mut moving = state.moving.lock().expect("lock");
    let now = std::time::Instant::now();
    let rolled_back = moving.rolled_back.clone();
    for queued in moving.queued.iter_mut().filter(|q| !q.settled && q.ends_at <= now) {
        queued.settled = true;
        if rolled_back.contains(&queued.item) {
            let edits =
                if queued.name == "MoveMovie" { &state.movie_edits } else { &state.series_edits };
            lay_over(edits, queued.item, &serde_json::json!({ "path": queued.source }));
        }
    }
}

/// The commands the Arr lists, as `GET /api/v3/command` answers.
async fn commands(State(state): State<FakeState>, headers: HeaderMap) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/command");
    settle_moves(&state);
    let moving = state.moving.lock().expect("lock");
    let now = std::time::Instant::now();
    let listed: Vec<serde_json::Value> = moving
        .queued
        .iter()
        .map(|queued| {
            let (status, message) = if queued.ends_at > now {
                ("started", "Moving")
            } else if moving.failed.contains(&queued.item) {
                ("failed", "Access to the path is denied")
            } else {
                ("completed", "Completed")
            };
            let key = if queued.name == "MoveMovie" { "movieId" } else { "seriesId" };
            serde_json::json!({
                "id": queued.id,
                "name": queued.name,
                "status": status,
                "message": message,
                "body": {
                    key: queued.item,
                    "sourcePath": queued.source,
                    "destinationPath": queued.destination,
                },
            })
        })
        .collect();
    Json(serde_json::Value::Array(listed))
}

async fn series_list(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/series");
    settle_moves(&state);
    hold_and_count(&state).await;
    if state.no_titles.load(Ordering::SeqCst) {
        return Json(serde_json::json!([]));
    }
    Json(serde_json::json!([bebop(&state, 20)]))
}

fn bebop(state: &FakeState, id: i64) -> serde_json::Value {
    let mut series = serde_json::json!({
        "id": id,
        "titleSlug": "cowboy-bebop",
        "seasonFolder": true,
        "qualityProfileId": 3,
        "title": "Cowboy Bebop",
        "year": 1998,
        "tvdbId": 76885,
        "tmdbId": 30991,
        "path": shown(state, &state.series_path),
        "rootFolderPath": shown(state, "/tv/standard"),
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
    });
    laid_over(&state.series_edits, id, &mut series);
    series
}

/// A series Sonarr knows and does not hold, as its lookup answers one: no id,
/// no folder, and the defaults Sonarr would replace when it adds it.
fn mushishi() -> serde_json::Value {
    serde_json::json!({
        "title": "Mushishi",
        "year": 2005,
        "tvdbId": 81178,
        "tmdbId": 26209,
        "imdbId": "tt0807832",
        "monitored": false,
        "status": "ended",
        "added": "0001-01-01T00:00:00Z",
        "seriesType": "standard",
        "seasons": [{ "seasonNumber": 1 }],
        "tags": [],
        "genres": ["Animation", "Drama"],
        "originalLanguage": { "id": 8, "name": "Japanese" },
        "certification": "TV-14"
    })
}

/// The one series by id: Cowboy Bebop is 20, a series [`FakeArr::refuse_series`]
/// names is held as well, and anything else is unknown to this Sonarr, unless
/// a body was set to answer every id with.
async fn series_one(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    record_key(&state, &headers);
    record_read(&state, &format!("/api/v3/series/{id}"));
    settle_moves(&state);
    if let Some(body) = state.series_body.lock().expect("lock").clone() {
        return Ok(Json(body));
    }
    if id != 20 && !state.refused_series.lock().expect("lock").contains(&id) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(bebop(&state, id)))
}

async fn series_update(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    record_key(&state, &headers);
    record_write(&state, &body, &params);

    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), "nope".to_string()));
    }
    if state.refused_series.lock().expect("lock").contains(&id) {
        return Err((StatusCode::BAD_REQUEST, "Path is not writable".to_string()));
    }
    let held = bebop(&state, id);
    take_update(&state, &state.series_edits, "MoveSeries", id, &held, &body, &params);
    // 202, as Sonarr's series update answers.
    Ok((StatusCode::ACCEPTED, Json(body)))
}

/// A command posted, recorded with the writes, as both Arrs queue it.
async fn command(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    record_key(&state, &headers);
    state.recorded.lock().expect("lock").writes.push(body);
    // 201, as both Arrs answer a command they queued.
    (StatusCode::CREATED, Json(serde_json::json!({ "id": 1, "status": "queued" })))
}
