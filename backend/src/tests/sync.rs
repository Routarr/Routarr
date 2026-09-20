//! Synchronization against a live fake Arr.
//!
//! This is the code path that deletes rows, and deleting a media row cascades to
//! the user's overrides — so the orphan-cleanup guardrails get the most attention
//! here.

use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// The webhook path stamped its `read_at` *after* reading the Arr, where the
/// full sync stamps it before its first request. A move applied while that
/// read was in flight was then older than the stamp, and the path the Arr had
/// answered with — the one from before the move — was written back over it.
#[tokio::test]
async fn a_webhook_read_started_before_a_move_does_not_put_the_old_path_back() {
    let arr = FakeArr::holding_edits(std::time::Duration::from_millis(2500)).await;
    let app = TestApp::new().await;
    // The hold has to straddle a whole second, since the stamps compare at
    // that resolution, and the test client's 300 ms budget would cut it off.
    let mut config = crate::config::Config::for_tests();
    config.http_timeout = std::time::Duration::from_secs(5);
    let app = TestApp::around(crate::state::AppState {
        http: crate::http::build_client(&config).expect("test http client"),
        config: std::sync::Arc::new(config),
        ..app.state.clone()
    });
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let instance = app.state.instance("inst-1").await.unwrap();

    // An apply that will land one second from now, seen from the row: the
    // read below starts before it and answers after it.
    let moved_at = crate::services::routing::format_timestamp(
        chrono::Utc::now() + chrono::Duration::seconds(1),
    );
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
         current_root_folder, moved_at)
         VALUES ('m-inst-1-10', 'inst-1', 10, 'movie', 'My Neighbor Totoro',
                 '/movies/anime/My Neighbor Totoro (1988)', '/movies/anime', ?)",
    )
    .bind(&moved_at)
    .execute(&app.state.pool)
    .await
    .unwrap();

    sync::sync_single_media(&app.state, &instance, 10).await.unwrap();

    let root: String =
        sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-inst-1-10'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime", "a stale read put the pre-move path back");
}

#[tokio::test]
async fn sync_populates_media_and_root_folders() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let report = sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    assert_eq!(report.media, 1);
    assert_eq!(report.root_folders, 3);

    let (id, root, has_files): (String, String, bool) =
        sqlx::query_as("SELECT id, current_root_folder, has_files FROM media")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    assert_eq!(id, "m-inst-1-10", "local ids are derived from the instance and arr id");
    assert_eq!(root, "/movies/standard");
    assert!(has_files);
}

#[tokio::test]
async fn sync_decrypts_the_stored_api_key_before_calling_the_arr() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let recorded = arr.recorded();
    assert!(!recorded.api_keys.is_empty());
    assert!(
        recorded.api_keys.iter().all(|key| key == "arr-key"),
        "the Arr must receive the plaintext key, never the ciphertext: {:?}",
        recorded.api_keys
    );
}

#[tokio::test]
async fn sync_records_its_outcome_on_the_instance_and_as_a_job() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let status: String =
        sqlx::query_scalar("SELECT last_sync_status FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "success");

    let (kind, job_status): (String, String) =
        sqlx::query_as("SELECT kind, status FROM jobs ORDER BY started_at DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), job_status.as_str()), ("sync", "success"));
}

#[tokio::test]
async fn a_failing_sync_is_recorded_rather_than_swallowed() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;

    assert!(sync::sync_instance(&app.state, "inst-1", "manual").await.is_err());

    let status: String =
        sqlx::query_scalar("SELECT last_sync_status FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(status.starts_with("error:"), "got {status}");

    let job_status: String =
        sqlx::query_scalar("SELECT status FROM jobs ORDER BY started_at DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(job_status, "failed");
}

#[tokio::test]
async fn media_removed_upstream_is_removed_locally() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    // A row the fake Arr does not know about.
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
         VALUES ('m-inst-1-999', 'inst-1', 999, 'movie', 'Deleted upstream', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    // With a proposal still pending for it. Decisions do not reference the
    // row, so nothing cascades: left as it was, it is listed for ever and never
    // applicable.
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
         current_root_folder, target_category, target_root_folder, action, status, reasons,
         alternatives, confidence)
         VALUES ('d-999', 'm-inst-1-999', 'Deleted upstream', 'movie', 'inst-1',
                 '/movies/standard', 'anime', '/movies/anime', 'move', 'pending', '[]', '[]', 1.0)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    assert_eq!(report.removed, 1);
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT id FROM media").fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(remaining, vec!["m-inst-1-10"]);
    let superseded: bool =
        sqlx::query_scalar("SELECT superseded FROM decisions WHERE id = 'd-999'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(superseded, "the proposal for a row the Arr no longer has still stands");
}

#[tokio::test]
async fn a_long_sync_does_not_delete_its_own_early_rows() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    // Two consecutive syncs: the second must recognise the first one's row as
    // current and leave it alone. A time-window cleanup would drop it.
    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();
    let second = sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    assert_eq!(second.removed, 0);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn an_empty_upstream_response_does_not_wipe_the_library() {
    let app = TestApp::new().await;
    // This fake answers with no movies and no root folders.
    let arr = EmptyArr::start().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
         VALUES ('m-inst-1-1', 'inst-1', 1, 'movie', 'Precious', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o1', 'm-inst-1-1', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    assert_eq!(report.removed, 0, "a misconfigured Arr must not look like a mass deletion");
    let overrides: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM overrides")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(overrides, 1, "overrides cascade from media and must survive");
}

#[tokio::test]
async fn re_syncing_updates_rather_than_duplicates() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();
    sqlx::query("UPDATE media SET title = 'Stale title'").execute(&app.state.pool).await.unwrap();
    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM media").fetch_all(&app.state.pool).await.unwrap();
    assert_eq!(titles, vec!["My Neighbor Totoro"]);
}

#[tokio::test]
async fn syncing_an_unknown_instance_is_a_not_found() {
    let app = TestApp::new().await;
    assert!(matches!(
        sync::sync_instance(&app.state, "nope", "manual").await,
        Err(crate::error::AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_concurrent_sync_of_the_same_instance_is_refused() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;

    // Hold the lock the way an in-flight sync would.
    let _held = app.state.jobs.try_lock("sync:inst-1").expect("first lock");

    assert!(matches!(
        sync::sync_instance(&app.state, "inst-1", "manual").await,
        Err(crate::error::AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn sync_all_isolates_per_instance_failures() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("good", "radarr", &arr.base_url).await;
    app.seed_instance_at("bad", "sonarr", "http://127.0.0.1:1").await;

    let reports = sync::sync_all_instances(&app.state, "manual").await.unwrap();

    assert_eq!(reports.len(), 2, "the failure must appear in the report, not vanish from it");
    let good = reports.iter().find(|r| r.instance_id == "good").unwrap();
    assert!(good.error.is_none());
    assert!(good.media > 0, "the reachable instance must still be synced");
    let bad = reports.iter().find(|r| r.instance_id == "bad").unwrap();
    assert!(bad.error.is_some(), "the unreachable instance must carry its error");
}

/// A fake that reports an empty library, to exercise the cleanup guardrail.
struct EmptyArr {
    base_url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl EmptyArr {
    async fn start() -> Self {
        use axum::routing::get;
        use axum::{Json, Router};

        let app = Router::new()
            .route(
                "/api/v3/system/status",
                get(|| async { Json(serde_json::json!({ "version": "5.0" })) }),
            )
            .route("/api/v3/rootfolder", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/v3/movie", get(|| async { Json(serde_json::json!([])) }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self { base_url: format!("http://{addr}"), shutdown: Some(tx) }
    }
}

impl Drop for EmptyArr {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

// ------------------------------------------------- the routes, not the service
//
// Everything above drives `sync::` directly. These go through the router, so
// the middleware, the extractors and the error mapping are exercised too — a
// handler can be perfect and still be unreachable, or return the right thing
// with the wrong status.

#[tokio::test]
async fn the_sync_route_reports_what_it_fetched() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let body = app.post("/api/v1/instances/inst-1/sync", serde_json::json!({})).await;
    let report = body.assert_ok();
    assert!(report["media"].as_i64().unwrap() > 0, "got {report}");
    assert!(report["root_folders"].as_i64().unwrap() > 0, "got {report}");
}

#[tokio::test]
async fn the_sync_all_route_covers_every_enabled_instance() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &arr.base_url).await;

    let body = app.post("/api/v1/instances/sync", serde_json::json!({})).await;
    // One report per instance, in a list — the shape the Instances screen reads.
    let reports = body.assert_ok().as_array().unwrap().clone();
    assert_eq!(reports.len(), 2, "got {reports:?}");
}

#[tokio::test]
async fn the_connectivity_route_answers_with_the_version_it_found() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let body = app.post("/api/v1/instances/inst-1/test", serde_json::json!({})).await;
    let report = body.assert_ok();
    assert_eq!(report["success"], true, "got {report}");
    assert!(report["version"].is_string(), "got {report}");
    assert!(report["root_folders"].as_i64().unwrap() > 0, "got {report}");
}

#[tokio::test]
async fn the_connectivity_route_says_so_when_the_arr_cannot_be_reached() {
    let app = TestApp::new().await;
    // A port nothing listens on.
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;

    let body = app.post("/api/v1/instances/inst-1/test", serde_json::json!({})).await;
    // Unreachable is an answer, not a crash: the upstream could not be reached,
    // and the screen has something to show for it.
    assert_eq!(body.status, axum::http::StatusCode::BAD_GATEWAY, "got {}", body.message());
    assert!(body.message().contains("unreachable"), "got {}", body.message());
}

/// A `for` loop with an `.await` in it syncs instances one after another. The
/// cost is not the database — `do_sync` fetches everything before it opens its
/// transaction — it is the waiting: an unreachable Arr costs the full HTTP
/// timeout, and two of them make the button look dead for twice that.
///
/// The fake counts how many listings it ever had open at once. A sequential
/// caller can only ever reach one, whatever the machine is doing, because it
/// does not issue the second request until the first has answered.
#[tokio::test]
async fn syncing_every_instance_does_them_at_the_same_time() {
    let arr = FakeArr::observing_concurrency().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-3", "sonarr", &arr.base_url).await;

    let reports = sync::sync_all_instances(&app.state, "manual").await.unwrap();

    assert_eq!(reports.len(), 3, "every instance should report");
    assert!(
        arr.max_concurrent() >= 2,
        "the fake never had more than {} request open at once, which is what a \
         sequential loop produces",
        arr.max_concurrent()
    );
}

/// Concurrency must not reach the caller as reordering: the reports are what
/// the API returns and what the interface lists, and a set of rows that shuffle
/// between two runs is a table nobody can read. `buffered` overlaps the work
/// and still yields in the order the instances were listed — `buffer_unordered`
/// would not.
#[tokio::test]
async fn the_reports_keep_the_order_the_instances_were_listed_in() {
    let arr = FakeArr::observing_concurrency().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &arr.base_url).await;
    app.seed_instance_at("inst-3", "radarr", &arr.base_url).await;

    let expected: Vec<String> =
        app.state.instances(true).await.unwrap().iter().map(|i| i.id.clone()).collect();
    let reports = sync::sync_all_instances(&app.state, "manual").await.unwrap();

    assert_eq!(
        reports.iter().map(|r| r.instance_id.clone()).collect::<Vec<_>>(),
        expected,
        "the reports came back in a different order from the instance list"
    );
}

// ------------------------------------------------- declared destinations

/// A target used to have to be a root folder in Radarr or Sonarr already, so
/// routing into `/movies/anime/kids` meant declaring it *there* first. What an
/// operator wants is one root folder per Arr and the targets beneath it
/// declared here.
#[tokio::test]
async fn a_declared_destination_survives_the_sync_that_does_not_report_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    let created = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/anime/kids" }),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["verified"], true, "the instance can see it: {}", created.json);

    app.post("/api/v1/instances/i-1/sync", serde_json::json!({})).await.assert_ok();

    // The orphan cleanup deletes what the Arr stops returning, and the Arr has
    // never returned this one. Deleting it would take the category with it.
    let kept: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM root_folders WHERE path = '/movies/anime/kids'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(kept, 1, "the sync deleted a destination the operator declared");
}

/// The whole point of allowing one: a path beneath a synced root is on that
/// root's volume, so its free space and its reachability are known — which is
/// what keeps the capacity and reachability guards meaningful for a folder no
/// Arr reports.
#[tokio::test]
async fn a_declared_destination_inherits_from_the_folder_it_sits_under() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    // Synced first, so a parent exists to inherit from.
    app.post("/api/v1/instances/i-1/sync", serde_json::json!({})).await.assert_ok();
    app.post(
        "/api/v1/root-folders",
        // Under `/movies/anime`, which the fake reports inaccessible with
        // 2048 bytes free — the deepest match, not `/movies`.
        serde_json::json!({ "instance_id": "i-1", "path": "/movies/anime/films" }),
    )
    .await
    .assert_ok();

    let (free, accessible): (Option<i64>, bool) = sqlx::query_as(
        "SELECT free_space, accessible FROM root_folders WHERE path = '/movies/anime/films'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    // Immediately, not at the next pass: a new destination showing no figures
    // for a whole sync interval is a row nobody can judge.
    assert_eq!(free, Some(2048), "it did not take its parent's free space");
    assert!(!accessible, "nor its parent's reachability");
}

/// A path the instance cannot see is refused at the point it is typed.
///
/// Routarr and the Arr run in different containers as often as not, so
/// `/media/films` existing here says nothing about the process that will do the
/// writing. Unchecked, the mistake surfaces as a refused move long afterwards.
#[tokio::test]
async fn a_path_the_instance_cannot_see_is_refused_when_it_is_typed() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/mnt/typo" }),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.json);
    assert!(refused.message().contains("/mnt/typo"), "{}", refused.message());
}

/// A folder removed and re-added in Radarr keeps its path and changes its id.
///
/// The upsert conflicts on `(instance, arr_id)`, so the new id inserts a second
/// row carrying the same path — and the orphan cleanup that removes the old one
/// runs after the loop, in the same transaction. Under a unique index on the
/// path the insert failed, the transaction rolled back, and that instance never
/// synchronised again.
#[tokio::test]
async fn a_root_folder_renumbered_by_the_arr_does_not_break_the_sync() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    // The library as a previous pass left it: the same path under the id the
    // Arr used to give it.
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, last_synced_at, origin)
         VALUES ('rf-old', 'i-1', 99, '/movies/standard', 1, 'a-previous-pass', 'arr')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let synced = app.post("/api/v1/instances/i-1/sync", serde_json::json!({})).await;
    assert_eq!(synced.status, 200, "the renumbering broke the sync: {}", synced.json);

    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM root_folders WHERE path = '/movies/standard'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(rows, 1, "the old row survived beside the new one");
    let arr_id: Option<i64> =
        sqlx::query_scalar("SELECT arr_id FROM root_folders WHERE path = '/movies/standard'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(arr_id, Some(1), "the row kept the id the Arr no longer uses");
}

/// A declared folder has no id in the Arr, and must not publish one.
///
/// `arr_id` is nullable since the row can be Routarr's own; typed as `i64` sqlx
/// decodes the NULL to `0`, and every declared destination went out over the
/// wire carrying a fabricated Arr id.
#[tokio::test]
async fn a_declared_folder_publishes_no_arr_id() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    app.post(
        "/api/v1/root-folders",
        serde_json::json!({ "instance_id": "i-1", "path": "/movies/anime/kids" }),
    )
    .await
    .assert_ok();

    let listed = app.get("/api/v1/root-folders").await.assert_ok().clone();
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["origin"] == "declared")
        .expect("the declared folder");
    assert!(row["arr_id"].is_null(), "a declared folder published an Arr id: {row}");
}

/// A misspelt last segment is a misspelling, not a folder.
///
/// The endpoint lists the *contents* of the directory it is given, and answers
/// about the nearest one above when the path does not exist — so asking about
/// the path itself returns the parent that does exist, and every typo under a
/// real root read as verified. The parent is listed and the leaf looked for
/// among its children.
#[tokio::test]
async fn a_typo_in_the_last_segment_is_not_verified() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    // `/movies/anime` exists; `/movies/anmie` does not.
    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/anmie" }),
        )
        .await;
    assert_eq!(refused.status, 400, "a misspelling was verified: {}", refused.json);
    assert!(refused.message().contains("/movies/anmie"), "{}", refused.message());
}

/// The series half of the same journey.
///
/// `SonarrClient::directory_exists` is a second copy of the parent-listing
/// trick, and every declared-destination test above went through a *Radarr*
/// instance — so the movie copy was covered and the series one was reachable by
/// nothing. An operator naming `/tv/anime` under Sonarr is doing what this
/// feature was built for.
#[tokio::test]
async fn a_declared_series_destination_is_checked_against_sonarr() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-tv", "sonarr", &arr.base_url).await;

    let created = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-tv", "path": "/tv/anime" }),
        )
        .await;
    // 200 and not 201: the declaration is idempotent, so it may be a no-op.
    assert_eq!(created.status, 200, "a folder Sonarr can see was refused: {}", created.json);
    assert_eq!(created.json["verified"], true, "{}", created.json);

    // And the same misspelling that the movie side refuses.
    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-tv", "path": "/tv/anmie" }),
        )
        .await;
    assert_eq!(refused.status, 400, "a misspelling was verified: {}", refused.json);
}

/// An unreachable Arr must not block configuration: refusing on an unavailable
/// probe locks the operator out at the worst possible moment.
#[tokio::test]
async fn an_instance_that_cannot_be_asked_does_not_block_the_declaration() {
    let app = TestApp::new().await;
    app.seed_instance_at("i-dead", "radarr", "http://127.0.0.1:1").await;

    let created = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-dead", "path": "/anything" }),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.json);
    // Saved, but honestly: nothing confirmed the path.
    assert_eq!(created.json["verified"], false);
}

/// A folder the instance reports is not ours to delete: it would come back on
/// the next sync, without the category mapped onto it.
#[tokio::test]
async fn a_synced_folder_cannot_be_deleted_here() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let refused = app.delete("/api/v1/root-folders/rf-1").await;
    assert_eq!(refused.status, 409, "{}", refused.json);
}
