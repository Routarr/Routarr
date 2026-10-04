//! An in-process Radarr/Sonarr stand-in.
//!
//! Pure helpers cover none of what an integration client does over the wire:
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
}

/// The ids the movie editor treats apart.
#[derive(Clone, Default)]
struct EditorQuirks {
    /// Refused with every other movie of their batch.
    refused: Vec<i64>,
    /// Gone from the library.
    forgotten: Vec<i64>,
    /// Edited and left out of the answer.
    unreported: Vec<i64>,
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
    /// The folder name the movie editor gives a movie moved with its files,
    /// as Radarr's naming format does. `None` keeps the folder it had.
    renames_to: Arc<Mutex<Option<String>>>,
    /// Whether the movie and series listings answer an empty list.
    no_titles: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the root folder listing answers an empty list.
    no_root_folders: Arc<std::sync::atomic::AtomicBool>,
    /// The series ids an update is refused for, as Sonarr refuses a path it
    /// cannot write.
    refused_series: Arc<Mutex<Vec<i64>>>,
    /// How the movie editor answers particular ids.
    editor: Arc<Mutex<EditorQuirks>>,
    /// Fields the film now has in the Arr, laid over its body.
    movie_edits: Arc<Mutex<serde_json::Map<String, serde_json::Value>>>,
    /// Fields the series now has in the Arr, laid over its body.
    series_edits: Arc<Mutex<serde_json::Map<String, serde_json::Value>>>,
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
    renames_to: Arc<Mutex<Option<String>>>,
    no_titles: Arc<std::sync::atomic::AtomicBool>,
    no_root_folders: Arc<std::sync::atomic::AtomicBool>,
    refused_series: Arc<Mutex<Vec<i64>>>,
    editor: Arc<Mutex<EditorQuirks>>,
    movie_edits: Arc<Mutex<serde_json::Map<String, serde_json::Value>>>,
    series_edits: Arc<Mutex<serde_json::Map<String, serde_json::Value>>>,
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

    /// Start a fake whose movie editor names a movie moved with its files
    /// `folder`, as Radarr's naming format may.
    pub async fn renaming_folders_to(folder: &str) -> Self {
        let fake = Self::start().await;
        fake.renames_to.lock().expect("lock").replace(folder.to_string());
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

    /// From now on the film has these fields, as an edit in the Arr leaves it.
    pub fn edit_movie(&self, edits: serde_json::Value) {
        let mut held = self.movie_edits.lock().expect("lock");
        for (field, value) in edits.as_object().expect("an object of fields") {
            held.insert(field.clone(), value.clone());
        }
    }

    /// From now on the series has these fields, as an edit in the Arr leaves it.
    pub fn edit_series(&self, edits: serde_json::Value) {
        let mut held = self.series_edits.lock().expect("lock");
        for (field, value) in edits.as_object().expect("an object of fields") {
            held.insert(field.clone(), value.clone());
        }
    }

    /// From now on the root folder listing reports `folder` as well.
    pub fn report_root_folder(&self, folder: serde_json::Value) {
        self.more_root_folders.lock().expect("lock").push(folder);
    }

    /// From now on series `id` is held, and an update of it refused.
    pub fn refuse_series(&self, id: i64) {
        self.refused_series.lock().expect("lock").push(id);
    }

    /// From now on an edit naming movie `id` is refused, with every other
    /// movie of its batch.
    pub fn refuse_movie(&self, id: i64) {
        self.editor.lock().expect("lock").refused.push(id);
    }

    /// From now on movie `id` is gone from the library: an edit naming it
    /// fails whole, as Radarr's lookup of a batch fails on one id it lacks.
    pub fn forget_movie(&self, id: i64) {
        self.editor.lock().expect("lock").forgotten.push(id);
    }

    /// From now on an edit naming movie `id` succeeds without listing it.
    pub fn leave_out_of_the_answer(&self, id: i64) {
        self.editor.lock().expect("lock").unreported.push(id);
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
        let renames_to: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let no_titles = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let no_root_folders = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let refused_series: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
        let editor = Arc::new(Mutex::new(EditorQuirks::default()));
        let movie_edits = Arc::new(Mutex::new(serde_json::Map::new()));
        let series_edits = Arc::new(Mutex::new(serde_json::Map::new()));
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
            renames_to: Arc::clone(&renames_to),
            no_titles: Arc::clone(&no_titles),
            no_root_folders: Arc::clone(&no_root_folders),
            refused_series: Arc::clone(&refused_series),
            editor: Arc::clone(&editor),
            movie_edits: Arc::clone(&movie_edits),
            series_edits: Arc::clone(&series_edits),
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
            .route("/api/v3/movie/{id}", get(movie_one))
            .route("/api/v3/movie/editor", put(movie_editor))
            .route("/api/v3/series/lookup", get(series_lookup))
            .route("/api/v3/series", get(series_list))
            .route("/api/v3/series/{id}", get(series_one).put(series_update))
            .route("/api/v3/command", post(command))
            .layer(axum::middleware::from_fn(refuse_without_a_key))
            .with_state(state);

        let server = super::serve(app).await;

        Self {
            base_url: server.address.clone(),
            recorded,
            series_body,
            max_in_flight,
            tags_broken,
            renames_to,
            no_titles,
            no_root_folders,
            refused_series,
            editor,
            movie_edits,
            series_edits,
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
    for (field, value) in state.movie_edits.lock().expect("lock").iter() {
        movie[field] = value.clone();
    }
    movie
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

async fn movie_editor(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    record_key(&state, &headers);
    state.recorded.lock().expect("lock").writes.push(body.clone());

    if let Some(status) = state.fail_with {
        return Err((StatusCode::from_u16(status).unwrap(), refusal()));
    }
    let quirks = state.editor.lock().expect("lock").clone();
    let ids = body["movieIds"].as_array().into_iter().flatten().filter_map(|id| id.as_i64());
    if ids.clone().any(|id| quirks.refused.contains(&id)) {
        return Err((StatusCode::BAD_REQUEST, refusal()));
    }
    let held = ids.clone().filter(|id| !quirks.forgotten.contains(id)).count();
    if held < ids.clone().count() {
        // Radarr's own words, from the lookup of every id at once.
        let message =
            format!("Expected query to return {} rows but returned {held}", ids.clone().count());
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "message": message }).to_string(),
        ));
    }
    let ids = ids.filter(move |id| !quirks.unreported.contains(id));
    // Recorded first, then held: a test can see the edit arrive before the
    // Arr is done with it.
    if !state.hold.is_zero() {
        tokio::time::sleep(state.hold).await;
    }
    // As Radarr answers: each movie edited, with the path it now has. Without
    // its files a movie keeps its folder name, with them the naming format
    // may give Totoro another. Any other id is one of `films_to_move`, whose
    // folder is `Film <id>`.
    let separator = if state.windows { '\\' } else { '/' };
    let root = body["rootFolderPath"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches(['/', '\\'])
        .to_string();
    let movie = totoro(&state);
    let kept =
        movie["path"].as_str().unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or_default();
    let renamed = state.renames_to.lock().expect("lock").clone();
    let totoro_folder = match renamed {
        Some(name) if body["moveFiles"].as_bool() == Some(true) => name,
        _ => kept.to_string(),
    };
    let moved: Vec<serde_json::Value> = ids
        .map(|id| {
            let folder = if Some(id) == movie["id"].as_i64() {
                totoro_folder.clone()
            } else {
                format!("Film {id}")
            };
            serde_json::json!({ "id": id, "path": format!("{root}{separator}{folder}") })
        })
        .collect();
    // 202, as Radarr's movie editor answers.
    Ok((StatusCode::ACCEPTED, Json(serde_json::Value::Array(moved))))
}

async fn series_list(
    State(state): State<FakeState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    record_key(&state, &headers);
    record_read(&state, "/api/v3/series");
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
    for (field, value) in state.series_edits.lock().expect("lock").iter() {
        series[field] = value.clone();
    }
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
    if state.refused_series.lock().expect("lock").contains(&id) {
        return Err((StatusCode::BAD_REQUEST, "Path is not writable".to_string()));
    }
    // 202, as Sonarr's series update answers.
    Ok((StatusCode::ACCEPTED, Json(body)))
}

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
