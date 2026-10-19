//! Live checks against a **real** Radarr and Sonarr: what the fakes cannot prove, since they
//! answer as Routarr expects. `.github/workflows/live-arrs.yml` starts both every week, and the
//! module runs against any pair a person points it at. Ignored by default, and skipped without
//! its variables.
//!
//! ```bash
//! RADARR_URL=… RADARR_API_KEY=… SONARR_URL=… SONARR_API_KEY=… LIVE_ARRS_MEDIA=… \
//!   cargo test live_arrs -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `LIVE_ARRS_MEDIA` is the folder both Arrs see as `/media`: the tests create titles' folders
//! and files there. Every write goes to the Arrs given, so never point it at a library in use.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::TestApp;
use crate::jobs::Attribution;
use crate::services::{executor, sync};

/// Where both Arrs see the media folder.
const MEDIA: &str = "/media";

/// A real Arr, driven through its own API to set the stage Routarr then reads.
struct Arr {
    kind: &'static str,
    url: String,
    key: String,
    http: reqwest::Client,
}

/// Both Arrs and the folder they share, or `None` when the variables are missing.
fn live() -> Option<(Arr, Arr, PathBuf)> {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let http = crate::http::build_client(&crate::config::Config {
        http_timeout: Duration::from_secs(60),
        ..crate::config::Config::for_tests()
    })
    .expect("live http client");
    let arr = |kind, url: String, key: String| Arr { kind, url, key, http: http.clone() };
    match (
        var("RADARR_URL"),
        var("RADARR_API_KEY"),
        var("SONARR_URL"),
        var("SONARR_API_KEY"),
        var("LIVE_ARRS_MEDIA"),
    ) {
        (Some(ru), Some(rk), Some(su), Some(sk), Some(media)) => {
            Some((arr("radarr", ru, rk), arr("sonarr", su, sk), PathBuf::from(media)))
        }
        // The weekly job sets it: a run that skipped everything would read as green.
        _ if var("LIVE_ARRS_REQUIRED").is_some() => {
            panic!("RADARR_*, SONARR_* or LIVE_ARRS_MEDIA unset where LIVE_ARRS_REQUIRED is")
        }
        _ => {
            eprintln!("live_arrs: RADARR_*, SONARR_* or LIVE_ARRS_MEDIA unset, skipped");
            None
        }
    }
}

impl Arr {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let mut request = self
            .http
            .request(method.clone(), format!("{}/api/v3{path}", self.url))
            .header("X-Api-Key", &self.key);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap_or_else(|e| panic!("{method} {path}: {e:?}"));
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        assert!(status.is_success(), "{} {method} {path}: {status} {text}", self.kind);
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    async fn get(&self, path: &str) -> Value {
        self.call(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    async fn delete(&self, path: &str) {
        self.call(reqwest::Method::DELETE, path, None).await;
    }

    /// The root folder at `path`, added unless the Arr lists it, its folder made first.
    async fn root_folder(&self, media: &Path, path: &str) {
        std::fs::create_dir_all(on_host(media, path)).expect("a root folder on the host");
        let listed = self.get("/rootfolder").await;
        if !listed.as_array().unwrap().iter().any(|folder| same_folder(&folder["path"], path)) {
            self.post("/rootfolder", json!({ "path": path })).await;
        }
    }

    async fn quality_profile(&self) -> i64 {
        self.get("/qualityprofile").await[0]["id"].as_i64().expect("a quality profile")
    }

    /// The film with TMDB id `tmdb`, added unmonitored under `/media/movies` unless held. Adding
    /// asks Radarr's metadata server, whose failure is said as such.
    async fn film(&self, media: &Path, tmdb: i64) -> Value {
        if let Some(held) = self.get(&format!("/movie?tmdbId={tmdb}")).await.get(0) {
            return held.clone();
        }
        self.root_folder(media, "/media/movies").await;
        let mut found = self.get(&format!("/movie/lookup/tmdb?tmdbId={tmdb}")).await;
        assert!(found["title"].is_string(), "Radarr's metadata server did not answer: {found}");
        found["qualityProfileId"] = json!(self.quality_profile().await);
        found["rootFolderPath"] = json!("/media/movies");
        found["monitored"] = json!(false);
        found["addOptions"] = json!({ "searchForMovie": false });
        self.post("/movie", found).await
    }

    /// The series with TVDB id `tvdb`, added unmonitored under `/media/tv` unless held.
    async fn series(&self, media: &Path, tvdb: i64) -> Value {
        let held = self.get("/series").await;
        if let Some(series) = held.as_array().unwrap().iter().find(|s| s["tvdbId"] == tvdb) {
            return series.clone();
        }
        self.root_folder(media, "/media/tv").await;
        let found = self.get(&format!("/series/lookup?term=tvdb:{tvdb}")).await;
        let mut series = found[0].clone();
        assert!(series["title"].is_string(), "Sonarr's metadata server did not answer: {found}");
        series["qualityProfileId"] = json!(self.quality_profile().await);
        series["rootFolderPath"] = json!("/media/tv");
        series["monitored"] = json!(false);
        series["seasonFolder"] = json!(true);
        series["addOptions"] = json!({ "monitor": "none", "searchForMissingEpisodes": false });
        self.post("/series", series).await
    }
}

/// `path`, as the Arrs write it under `/media`, on the host.
fn on_host(media: &Path, path: &str) -> PathBuf {
    media.join(path.trim_start_matches(MEDIA).trim_start_matches('/'))
}

fn same_folder(listed: &Value, path: &str) -> bool {
    listed.as_str().is_some_and(|listed| crate::paths::same(listed, path))
}

/// A harness reading the real Arrs with real timeouts, `kind` reached under `id`.
async fn routarr(arrs: &[(&str, &Arr)]) -> TestApp {
    let app = TestApp::new()
        .await
        .with_http_budget(Duration::from_secs(60))
        .with_move_wait(Duration::from_secs(60));
    app.store_setting("global_dry_run", "false").await;
    for (id, arr) in arrs {
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
                                    webhook_token)
             VALUES (?, ?, ?, ?, ?, 1, 'live-token')",
        )
        .bind(id)
        .bind(format!("Live {}", arr.kind))
        .bind(arr.kind)
        .bind(&arr.url)
        .bind(app.state.secrets.seal(&arr.key).unwrap())
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    app
}

async fn sync(app: &TestApp, instance: &str) {
    sync::sync_instance(&app.state, instance, &Attribution::manual(None))
        .await
        .unwrap_or_else(|e| panic!("the sync of {instance} failed: {e}"));
}

/// The Routarr row of the title the Arr numbers `arr_id`, if any.
async fn row(app: &TestApp, instance: &str, arr_id: i64) -> Option<(String, String)> {
    sqlx::query_as("SELECT id, current_root_folder FROM media WHERE instance_id = ? AND arr_id = ?")
        .bind(instance)
        .bind(arr_id)
        .fetch_optional(&app.state.pool)
        .await
        .unwrap()
}

/// Wait until `reached` holds, a real Arr taking seconds where a fake takes none.
async fn eventually<F: std::future::Future<Output = bool>>(what: &str, reached: impl Fn() -> F) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while !reached().await {
        assert!(tokio::time::Instant::now() < deadline, "never happened: {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Send the title the Arr numbers `arr_id` to `category`, through a declared destination at
/// `path`, and apply the one move the simulation then proposes.
async fn move_to(
    app: &TestApp,
    media: &Path,
    instance: &str,
    arr_id: i64,
    (category, path): (&str, &str),
    move_files: bool,
) -> executor::ApplyReport {
    std::fs::create_dir_all(on_host(media, path)).expect("a destination on the host");
    app.post("/api/v1/categories", json!({ "name": category })).await.assert_ok();
    let declared =
        app.post("/api/v1/root-folders", json!({ "instance_id": instance, "path": path })).await;
    let id = declared.assert_ok()["id"].as_str().expect("a folder id").to_string();
    let mapped = json!({ "category": category });
    app.put(&format!("/api/v1/root-folders/{id}/category"), mapped).await.assert_ok();
    let (media_id, _) = row(app, instance, arr_id).await.expect("the title synced");
    let pinned = json!({ "media_id": media_id, "target_category": category });
    app.post("/api/v1/overrides", pinned).await.assert_ok();
    app.simulate().await;
    let decision: String = sqlx::query_scalar(
        "SELECT id FROM decisions
          WHERE media_id = ? AND action = 'move' AND status = 'pending' AND superseded = 0",
    )
    .bind(&media_id)
    .fetch_one(&app.state.pool)
    .await
    .expect("a move proposed");
    let report = executor::apply_decisions(
        &app.state,
        &[decision],
        move_files,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .expect("the apply ran");
    if report.skipped > 0 {
        describe(app, &media_id).await;
    }
    report
}

/// What Routarr holds about a title whose move was skipped, the reason being in these rows.
async fn describe(app: &TestApp, media_id: &str) {
    let decisions: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT action || ' ' || status || ' superseded ' || superseded,
                COALESCE(current_root_folder, '-'), COALESCE(target_root_folder, '-')
           FROM decisions WHERE media_id = ?",
    )
    .bind(media_id)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    let held: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT current_path, current_root_folder FROM media WHERE id = ?")
            .bind(media_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let folders: Vec<(String, Option<String>, String)> =
        sqlx::query_as("SELECT path, category, origin FROM root_folders")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let targets =
        crate::services::routing::revalidated_targets(&app.state.pool, &[media_id.to_string()])
            .await
            .unwrap();
    eprintln!("skipped: decisions {decisions:?}\n  media {held:?}\n  folders {folders:?}");
    eprintln!("  routed now to {targets:?}");
}

/// Every endpoint a sync reads answers in the shape the adapters parse, on both Arrs.
#[tokio::test]
#[ignore = "drives a real Radarr and Sonarr"]
async fn both_arrs_sync_into_routarr() {
    let Some((radarr, sonarr, media)) = live() else { return };
    let totoro = radarr.film(&media, 8392).await;
    let bebop = sonarr.series(&media, 76885).await;
    let app = routarr(&[("radarr", &radarr), ("sonarr", &sonarr)]).await;

    sync(&app, "radarr").await;
    sync(&app, "sonarr").await;

    let film = row(&app, "radarr", totoro["id"].as_i64().unwrap()).await.expect("Totoro");
    assert!(crate::paths::same(&film.1, "/media/movies"), "{film:?}");
    let series = row(&app, "sonarr", bebop["id"].as_i64().unwrap()).await.expect("Bebop");
    assert!(crate::paths::same(&series.1, "/media/tv"), "{series:?}");
    let folders = app.count("SELECT COUNT(*) FROM root_folders WHERE origin = 'arr'").await;
    assert!(folders >= 2, "the root folders were not read: {folders}");
}

/// A title repointed to a declared destination, beneath a root folder of the Arr or under none,
/// stays there: the Arr reports its own root folder back at every read.
#[tokio::test]
#[ignore = "drives a real Radarr and Sonarr"]
async fn a_title_moved_into_a_declared_destination_is_not_proposed_again() {
    let Some((radarr, _, media)) = live() else { return };
    let totoro = radarr.film(&media, 8392).await;
    let arr_id = totoro["id"].as_i64().unwrap();
    let app = routarr(&[("radarr", &radarr)]).await;
    sync(&app, "radarr").await;

    for destination in [("kids", "/media/movies/kids"), ("elsewhere", "/media/elsewhere")] {
        let report = move_to(&app, &media, "radarr", arr_id, destination, false).await;
        assert_eq!(report.applied, 1, "{report:?}");
        let held = radarr.get(&format!("/movie/{arr_id}")).await;
        let path = held["path"].as_str().unwrap();
        assert!(crate::paths::within(path, destination.1), "Radarr holds {path}");

        // A later sync: within the second of the move, the sync keeps the path the move wrote.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        sync(&app, "radarr").await;
        let (_, root) = row(&app, "radarr", arr_id).await.unwrap();
        assert!(crate::paths::same(&root, destination.1), "read back as {root}");
        app.simulate().await;
        let moves = "SELECT COUNT(*) FROM decisions WHERE action = 'move' AND superseded = 0
                       AND status = 'pending'";
        assert_eq!(app.count(moves).await, 0, "proposed again after {destination:?}");
    }
}

/// A move with files carries the file to its destination, followed through the Arr's own
/// command, and one the Arr cannot write is recorded as failed once the Arr puts it back.
#[tokio::test]
#[ignore = "drives a real Radarr and Sonarr"]
async fn a_move_with_files_lands_on_disk_and_a_refused_one_is_recorded() {
    let Some((radarr, _, media)) = live() else { return };
    let spirited = radarr.film(&media, 129).await;
    let arr_id = spirited["id"].as_i64().unwrap();
    let folder = spirited["path"].as_str().unwrap().to_string();
    let host = on_host(&media, &folder);
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(host.join("Spirited Away (2001).strm"), b"live").unwrap();
    let app = routarr(&[("radarr", &radarr)]).await;
    sync(&app, "radarr").await;

    let report = move_to(&app, &media, "radarr", arr_id, ("big", "/media/movies/big"), true).await;
    assert_eq!((report.applied, report.failed), (1, 0), "{report:?}");
    let moved = on_host(&media, "/media/movies/big").join(host.file_name().unwrap());
    assert!(moved.join("Spirited Away (2001).strm").exists(), "the file is not at {moved:?}");

    let locked = on_host(&media, "/media/locked");
    std::fs::create_dir_all(&locked).unwrap();
    sync(&app, "radarr").await;
    let mut readonly = std::fs::metadata(&locked).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut readonly, 0o555);
    std::fs::set_permissions(&locked, readonly).unwrap();
    let report = move_to(&app, &media, "radarr", arr_id, ("locked", "/media/locked"), true).await;
    assert_eq!((report.applied, report.failed), (0, 1), "{report:?}");
}

/// Each Arr's own webhook reaches Routarr: its Test when the connection is saved, then the
/// titles it adds and deletes. The connection reads right from the real `/notification`.
#[tokio::test]
#[ignore = "drives a real Radarr and Sonarr"]
async fn the_arrs_webhooks_reach_routarr() {
    let Some((radarr, sonarr, media)) = live() else { return };
    radarr.film(&media, 8392).await;
    sonarr.series(&media, 76885).await;
    let app = routarr(&[("radarr", &radarr), ("sonarr", &sonarr)]).await;
    let served = app.served().await;
    for (id, arr) in [("radarr", &radarr), ("sonarr", &sonarr)] {
        sync(&app, id).await;
        let schema = arr.get("/notification/schema").await;
        let mut connection = schema
            .as_array()
            .unwrap()
            .iter()
            .find(|template| template["implementation"] == "Webhook")
            .expect("a Webhook template")
            .clone();
        connection["name"] = json!("Routarr");
        for (name, value) in connection.as_object_mut().unwrap().iter_mut() {
            if name.starts_with("on") && value.is_boolean() {
                *value = json!(true);
            }
        }
        for field in connection["fields"].as_array_mut().unwrap() {
            match field["name"].as_str() {
                Some("url") => {
                    field["value"] = json!(format!("{}/api/v1/webhook/{id}", served.address))
                }
                Some("method") => field["value"] = json!(1),
                Some("headers") => {
                    field["value"] = json!([{ "key": "X-Routarr-Token", "value": "live-token" }])
                }
                _ => {}
            }
        }
        // Saving runs the Arr's Test, which Routarr answers.
        arr.post("/notification", connection).await;
        let heard = || async {
            let event: Option<String> =
                sqlx::query_scalar("SELECT last_webhook_event FROM instances WHERE id = ?")
                    .bind(id)
                    .fetch_one(&app.state.pool)
                    .await
                    .unwrap();
            event.as_deref() == Some("Test")
        };
        eventually(&format!("{id}'s Test"), heard).await;
        let tested = app.post(&format!("/api/v1/instances/{id}/test"), json!({})).await;
        assert_eq!(tested.assert_ok()["webhook"]["state"], "ok", "{}", tested.json);
    }

    let kiki = radarr.film(&media, 16859).await["id"].as_i64().unwrap();
    eventually("MovieAdded", || async { row(&app, "radarr", kiki).await.is_some() }).await;
    radarr.delete(&format!("/movie/{kiki}?deleteFiles=false")).await;
    eventually("MovieDelete", || async { row(&app, "radarr", kiki).await.is_none() }).await;

    let eva = sonarr.series(&media, 70350).await["id"].as_i64().unwrap();
    eventually("SeriesAdd", || async { row(&app, "sonarr", eva).await.is_some() }).await;
    sonarr.delete(&format!("/series/{eva}?deleteFiles=false")).await;
    eventually("SeriesDelete", || async { row(&app, "sonarr", eva).await.is_none() }).await;
}
