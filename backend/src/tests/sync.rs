//! Synchronization against a live fake Arr.
//!
//! This is the code path that deletes rows, and deleting a media row cascades to
//! the user's overrides, so the orphan-cleanup guardrails get the most attention
//! here.

use crate::services::sync;

use super::fake_arr::FakeArr;
use super::{TestApp, finished, preferring_async};

/// A reverse proxy that times out, or a tab closed, drops the request. The
/// sync it started runs to its end: rolled back with the request, the library
/// stays stale and the Tasks screen shows a failure nobody caused.
#[tokio::test]
async fn hanging_up_mid_sync_still_finishes_the_sync() {
    use std::time::Duration;

    // The listing is held so the hang-up lands while it is in flight, under
    // a budget it cannot reach on a loaded machine.
    let arr = FakeArr::holding_edits(Duration::from_millis(200)).await;
    let app = TestApp::new().await.with_http_budget(Duration::from_secs(5));
    app.seed_instance_at("inst-held", "radarr", &arr.base_url).await;

    {
        let mut syncing =
            Box::pin(app.post("/api/v1/instances/inst-held/sync", serde_json::json!({})));
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            while !arr.recorded().reads.iter().any(|read| read == "/api/v3/movie") {
                let _ = futures::poll!(syncing.as_mut());
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(reached.is_ok(), "the sync never asked for the library");
    }

    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM jobs WHERE kind = 'sync' AND status != 'running'",
            )
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
            if let Some(status) = status {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the sync job never ended");
    assert_eq!(finished, "success");
    let media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE instance_id = 'inst-held'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(media > 0, "the library was not stored");
}

/// A large library takes longer to list than a probe is given to answer: the
/// listing has a budget of its own, so the sync still reads it.
#[tokio::test]
async fn a_library_slower_to_list_than_a_probe_is_still_read() {
    let arr = FakeArr::holding_edits(std::time::Duration::from_millis(600)).await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .expect("the listing was cut at the probe's budget");

    assert_eq!(report.media, 1);
}

/// The webhook path stamps its `read_at` *before* reading the Arr, as the full
/// sync does before its first request. Stamped after, a move applied while that
/// read is in flight would be older than the stamp, and the path the Arr
/// answered with, the one from before the move, would be written back over it.
#[tokio::test]
async fn a_webhook_read_started_before_a_move_does_not_put_the_old_path_back() {
    let arr = FakeArr::holding_edits(std::time::Duration::from_millis(2500)).await;
    // The hold has to straddle a whole second, since the stamps compare at
    // that resolution.
    let app = TestApp::new().await.with_http_budget(std::time::Duration::from_secs(5));
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

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

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

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

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

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

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

/// A failure is written on the instance and as a job. It moves the time of the
/// last attempt and leaves the time of the last success where it was: the
/// Instances screen reads that one as how fresh the library is.
#[tokio::test]
async fn a_failing_sync_is_recorded_rather_than_swallowed() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    let succeeded = "2026-09-01 08:00:00";
    sqlx::query(
        "UPDATE instances SET last_sync_at = ?, last_sync_attempt_at = ? WHERE id = 'inst-1'",
    )
    .bind(succeeded)
    .bind(succeeded)
    .execute(&app.state.pool)
    .await
    .unwrap();

    assert!(
        sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
            .await
            .is_err()
    );

    let (status, last_sync, attempted): (String, String, String) = sqlx::query_as(
        "SELECT last_sync_status, last_sync_at, last_sync_attempt_at FROM instances
         WHERE id = 'inst-1'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(status.starts_with("error:"), "got {status}");
    assert_eq!(last_sync, succeeded, "a failure refreshed the time of the last success");
    assert!(attempted.as_str() > succeeded, "the attempt was not recorded: {attempted}");

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

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

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

/// An Arr answering no title while it still reports its folders is restoring,
/// or a base URL points elsewhere: the titles stay, and their exceptions with
/// them, since deleting a title cascades to its exception.
/// A library leaving an Arr outnumbers what one statement binds, so the
/// retirement runs in chunks and the report counts every one of them.
#[tokio::test]
async fn a_sync_retiring_more_rows_than_one_statement_binds_retires_and_counts_them_all() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let gone = crate::services::routing::BIND_CHUNK + 1;
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?)
         INSERT INTO media (id, instance_id, arr_id, media_type, title)
         SELECT 'm-inst-1-' || (1000 + i), 'inst-1', 1000 + i, 'movie', 'Gone ' || i FROM n",
    )
    .bind(gone as i64)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.removed, gone as u64);
    assert_eq!(app.count("SELECT COUNT(*) FROM media").await, 1, "a chunk was left behind");
}

#[tokio::test]
async fn an_arr_answering_no_title_keeps_the_library() {
    let arr = FakeArr::start().await;
    arr.answer_no_titles();
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
             VALUES ('m-inst-1-10', 'inst-1', 10, 'movie', 'Precious', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category)
             VALUES ('o1', 'm-inst-1-10', 'anime')",
    ])
    .await;

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.removed, 0, "a misconfigured Arr must not look like a mass deletion");
    assert_eq!(app.count("SELECT COUNT(*) FROM media").await, 1);
    assert_eq!(app.count("SELECT COUNT(*) FROM overrides").await, 1, "the exception went");
}

/// The same for folders: an Arr answering no root folder keeps the ones it
/// reported, and the category each is mapped to.
#[tokio::test]
async fn an_arr_answering_no_root_folder_keeps_the_folders_and_their_categories() {
    let arr = FakeArr::start().await;
    arr.answer_no_root_folders();
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.execute(&["INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
           VALUES ('rf-anime', 'inst-1', 2, '/movies/anime', 1, 'anime')"])
        .await;

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let mapped = app
        .count("SELECT COUNT(*) FROM root_folders WHERE id = 'rf-anime' AND category = 'anime'")
        .await;
    assert_eq!(mapped, 1, "the folder or its category went with an empty answer");
}

/// The unreachable warning and its question print when a folder last
/// answered, so a pass stamps that time on a folder that answers and leaves it
/// on one that does not.
#[tokio::test]
async fn a_sync_stamps_when_a_folder_answered_and_keeps_it_while_it_does_not() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.execute(&["INSERT INTO root_folders (id, instance_id, arr_id, path, accessible,
                                             last_accessible_at)
           VALUES ('rf-inst-1-2', 'inst-1', 2, '/movies/anime', 1, '2026-09-05 03:00:00')"])
        .await;

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let asleep: (bool, Option<String>) = sqlx::query_as(
        "SELECT accessible, last_accessible_at FROM root_folders WHERE path = '/movies/anime'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(asleep, (false, Some("2026-09-05 03:00:00".into())), "the last answer moved");
    let awake: (Option<String>, String) = sqlx::query_as(
        "SELECT last_accessible_at, last_synced_at FROM root_folders
          WHERE path = '/movies/standard'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(awake.0.as_ref(), Some(&awake.1), "a folder that answered was not stamped");
}

/// A title the webhook writes while a full sync is reading the Arr was not in
/// what the sync read, and is not gone: the sync retires only what was last
/// seen before it began reading.
#[tokio::test]
async fn a_title_written_after_the_sync_began_reading_survives_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files,
                            last_synced_at)
         VALUES ('m-inst-1-50', 'inst-1', 50, 'movie', 'Just added', 1, 0, '2999-01-01 00:00:00')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files,
                            last_synced_at)
         VALUES ('m-inst-1-51', 'inst-1', 51, 'movie', 'Long gone', 1, 1, '2000-01-01 00:00:00')",
    ])
    .await;

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(app.count("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-50'").await, 1);
    assert_eq!(app.count("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-51'").await, 0);
}

/// An Arr rebuilt, or an instance pointed at another Arr, hands its ids out
/// again: the title id 10 now names is not the one the row, its exception and
/// its proposal described, and none of them carries over to it.
#[tokio::test]
async fn an_arr_id_that_now_names_another_title_takes_nothing_of_the_old_one() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id,
                            current_root_folder, monitored, has_files)
         VALUES ('m-inst-1-10', 'inst-1', 10, 'movie', 'Spirited Away', 129,
                 '/movies/standard', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category)
         VALUES ('o-1', 'm-inst-1-10', 'kids')",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                current_root_folder, target_root_folder, target_category,
                                action, status)
         VALUES ('d-1', 'm-inst-1-10', 'Spirited Away', 'movie', 'inst-1', '/movies/standard',
                 '/movies/kids', 'kids', 'move', 'pending')",
    ])
    .await;

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(app.count("SELECT COUNT(*) FROM overrides").await, 0, "the exception carried over");
    assert_eq!(app.count("SELECT superseded FROM decisions WHERE id = 'd-1'").await, 1);
    assert_eq!(app.count("SELECT tmdb_id FROM media WHERE id = 'm-inst-1-10'").await, 8392);
}

/// Listing its root folders makes the Arr walk every folder inside each, so a
/// scheduled sync lists them once a day, and reads the free space of the
/// mounts in between. A sync somebody asked for lists them whatever the hour.
#[tokio::test]
async fn a_scheduled_sync_lists_the_root_folders_once_a_day() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let (scheduled, manual) =
        (crate::jobs::Attribution::unattended("schedule"), crate::jobs::Attribution::manual(None));
    let listings = || arr.recorded().reads.iter().filter(|p| *p == "/api/v3/rootfolder").count();

    for _ in 0..2 {
        sync::sync_instance(&app.state, "inst-1", &scheduled).await.unwrap();
    }
    assert_eq!(listings(), 1, "a scheduled sync listed the folders again within the day");
    let free: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT free_space FROM root_folders ORDER BY path")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(free, [Some(777); 3], "the free space was not read from the mount");

    sync::sync_instance(&app.state, "inst-1", &manual).await.unwrap();
    assert_eq!(listings(), 2, "a sync somebody asked for did not list the folders");
    app.execute(&["UPDATE instances SET root_folders_read_at = '2020-01-01 00:00:00'"]).await;
    sync::sync_instance(&app.state, "inst-1", &scheduled).await.unwrap();
    assert_eq!(listings(), 3, "a day on, the folders were not listed");
}

/// A corrected secondary id is a metadata fix, not an Arr id given to another
/// title: only the id the Arr holds unique tells that, the TMDb id of a film
/// and the TheTVDB id of a series. The exception and the proposal stay.
#[tokio::test]
async fn a_title_whose_secondary_id_was_corrected_keeps_its_exception() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, imdb_id,
                            current_root_folder, monitored, has_files)
         VALUES ('m-inst-1-10', 'inst-1', 10, 'movie', 'Totoro', 8392, 'tt0000001',
                 '/movies/standard', 1, 1)",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tvdb_id, tmdb_id,
                            current_root_folder, monitored, has_files)
         VALUES ('m-inst-2-20', 'inst-2', 20, 'series', 'Cowboy Bebop', 76885, 1,
                 '/tv/standard', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category)
         VALUES ('o-1', 'm-inst-1-10', 'kids'), ('o-2', 'm-inst-2-20', 'kids')",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                current_root_folder, target_root_folder, target_category,
                                action, status)
         VALUES ('d-1', 'm-inst-1-10', 'Totoro', 'movie', 'inst-1', '/movies/standard',
                 '/movies/kids', 'kids', 'move', 'pending')",
    ])
    .await;

    for instance in ["inst-1", "inst-2"] {
        sync::sync_instance(&app.state, instance, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();
    }

    assert_eq!(app.count("SELECT COUNT(*) FROM overrides").await, 2, "an exception was dropped");
    assert_eq!(app.count("SELECT superseded FROM decisions WHERE id = 'd-1'").await, 0);
    let imdb: String = sqlx::query_scalar("SELECT imdb_id FROM media WHERE id = 'm-inst-1-10'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(imdb, "tt0096283", "the correction was not taken");
    assert_eq!(app.count("SELECT tmdb_id FROM media WHERE id = 'm-inst-2-20'").await, 30991);
}

/// A transaction that reads before it writes fails at once with "database is
/// locked" behind another writer under a plain BEGIN, whatever the busy
/// timeout. The webhook's sync of one title, and a full sync whose rating
/// country is unknown, which both read first, wait their turn instead.
#[tokio::test]
async fn a_sync_waits_for_another_writer_rather_than_failing() {
    let dir = super::TempDir::new("busy-sync");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    let pool = crate::db::init_pool(&config).await.unwrap();
    let app =
        TestApp::around(crate::state::AppState::for_tests_on(pool.clone()).with_config(config));
    let arr = FakeArr::start().await;
    arr.rate_for("");
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let instance = app.state.instance("inst-1").await.unwrap();

    for full in [false, true] {
        let mut writer = pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *writer).await.unwrap();
        sqlx::query("UPDATE settings SET value = value WHERE key = 'batch_limit'")
            .execute(&mut *writer)
            .await
            .unwrap();
        let releasing = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            sqlx::query("COMMIT").execute(&mut *writer).await.unwrap();
        });

        let synced = if full {
            let by = crate::jobs::Attribution::manual(None);
            sync::sync_instance(&app.state, "inst-1", &by).await.map(|_| ())
        } else {
            sync::sync_single_media(&app.state, &instance, 10).await.map(|_| ())
        };

        releasing.await.unwrap();
        assert!(synced.is_ok(), "full sync {full}: {synced:?}");
    }
}

/// A sync writes and cleans only its own instance: another instance holding
/// the same Arr ids, a folder, an exception and a proposal keeps all of them,
/// while a title the synced Arr stopped reporting goes.
#[tokio::test]
async fn syncing_one_instance_leaves_another_instances_rows_alone() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "radarr", "http://127.0.0.1:1").await;
    app.execute(&[
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
             VALUES ('rf-other', 'inst-2', 1, '/movies/elsewhere', 1, 'anime')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
                                monitored, has_files)
             VALUES ('m-other', 'inst-2', 10, 'movie', 'Other', '/movies/elsewhere', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category)
             VALUES ('o-other', 'm-other', 'anime')",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                    current_root_folder, target_root_folder, target_category,
                                    action, status)
             VALUES ('d-other', 'm-other', 'Other', 'movie', 'inst-2', '/movies/elsewhere',
                     '/movies/anime', 'anime', 'move', 'pending')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
             VALUES ('m-inst-1-999', 'inst-1', 999, 'movie', 'Gone', 1, 1)",
    ])
    .await;

    let report = sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.removed, 1, "the title the synced Arr stopped reporting stayed");
    assert_eq!(app.count("SELECT COUNT(*) FROM media WHERE id = 'm-inst-1-999'").await, 0);
    let other = [
        "SELECT COUNT(*) FROM media WHERE id = 'm-other' AND current_root_folder = '/movies/elsewhere'",
        "SELECT COUNT(*) FROM overrides WHERE id = 'o-other'",
        "SELECT COUNT(*) FROM decisions WHERE id = 'd-other' AND superseded = 0",
        "SELECT COUNT(*) FROM root_folders WHERE id = 'rf-other' AND category = 'anime'",
    ];
    for sql in other {
        assert_eq!(app.count(sql).await, 1, "another instance's row changed: {sql}");
    }
}

#[tokio::test]
async fn a_re_sync_takes_every_change_the_arr_made_to_a_title() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    let by = crate::jobs::Attribution::manual(None);
    sync::sync_instance(&app.state, "inst-1", &by).await.unwrap();
    arr.edit_movie(serde_json::json!({
        "title": "Tonari no Totoro", "sortTitle": "tonari no totoro", "year": 1989,
        "path": "/movies/kids/Tonari no Totoro (1988)", "rootFolderPath": "/movies/kids",
        "monitored": false, "hasFile": false, "status": "announced", "sizeOnDisk": 1024,
        "tags": [2], "genres": ["Fantasy"], "certification": "PG",
        "originalLanguage": { "id": 1, "name": "English" }
    }));

    sync::sync_instance(&app.state, "inst-1", &by).await.unwrap();

    assert_eq!(app.count("SELECT COUNT(*) FROM media").await, 1, "the re-sync added a row");
    let place: (String, String, i64, String, String, bool, bool) = sqlx::query_as(
        "SELECT title, sort_title, year, current_path, current_root_folder, monitored, has_files
           FROM media WHERE id = 'm-inst-1-10'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    let moved = "/movies/kids/Tonari no Totoro (1988)";
    let renamed = ("Tonari no Totoro", "tonari no totoro", 1989, moved, "/movies/kids");
    assert_eq!(
        (place.0.as_str(), place.1.as_str(), place.2, place.3.as_str(), place.4.as_str()),
        renamed
    );
    assert_eq!((place.5, place.6), (false, false), "monitoring and files");
    let facts: (String, i64, String, String, String, String) = sqlx::query_as(
        "SELECT status, size_on_disk, tags, genres, original_language, certification
           FROM media WHERE id = 'm-inst-1-10'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    let (status, size, tags, genres, language, rating) = facts;
    assert_eq!(
        (status.as_str(), size, tags.as_str(), genres.as_str(), language.as_str(), rating.as_str()),
        ("announced", 1024, r#"["kids"]"#, r#"["Fantasy"]"#, "en", "PG")
    );
}

#[tokio::test]
async fn syncing_an_unknown_instance_is_a_not_found() {
    let app = TestApp::new().await;
    assert!(matches!(
        sync::sync_instance(&app.state, "nope", &crate::jobs::Attribution::manual(None)).await,
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
        sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None)).await,
        Err(crate::error::AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn sync_all_isolates_per_instance_failures() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("good", "radarr", &arr.base_url).await;
    app.seed_instance_at("bad", "sonarr", "http://127.0.0.1:1").await;

    let reports = sync::sync_all_instances(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(reports.len(), 2, "the failure must appear in the report, not vanish from it");
    let good = reports.iter().find(|r| r.instance_id == "good").unwrap();
    assert!(good.error.is_none());
    assert!(good.media > 0, "the reachable instance must still be synced");
    let bad = reports.iter().find(|r| r.instance_id == "bad").unwrap();
    assert!(bad.error.is_some(), "the unreachable instance must carry its error");
}

// ------------------------------------------------- the routes, not the service
//
// Everything above drives `sync::` directly. These go through the router, so
// the middleware, the extractors and the error mapping are exercised too: a
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

/// "Simulate after each sync": a sync somebody asked for is followed as a
/// scheduled one is, so with background sync off the library is still
/// enriched and simulated, by the automation.
#[tokio::test]
async fn a_sync_somebody_asked_for_is_followed_by_a_simulation() {
    for route in ["/api/v1/instances/inst-1/sync", "/api/v1/instances/sync"] {
        let arr = FakeArr::start().await;
        let app = TestApp::new().await;
        app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
        app.store_setting("auto_sync_enabled", "false").await;

        app.post(route, serde_json::json!({})).await.assert_ok();
        let followed = app.state.post_sync.lock().await.take();
        followed.expect("nothing followed the sync").await.unwrap();

        let simulated = app.count("SELECT COUNT(*) FROM decisions WHERE actor = 'auto'").await;
        assert!(simulated > 0, "{route} was not followed by a simulation");
    }
}

/// What the automation does after a sync is its own, not the work of who set
/// the sync off, and the simulation it runs is a task the Tasks screen lists,
/// with its outcome: it replaces every pending proposal.
#[tokio::test]
async fn the_work_after_a_sync_is_a_task_of_the_automation() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    let by = crate::jobs::Attribution::manual(Some("alice"));
    crate::jobs::scheduler::follow_sync(&app.state, &by).await;
    let followed = app.state.post_sync.lock().await.take();
    followed.expect("nothing followed the sync").await.unwrap();

    let task: (String, Option<String>, String) =
        sqlx::query_as("SELECT trigger, subject, status FROM jobs WHERE kind = 'simulate'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(task, ("auto".into(), Some("alice".into()), "success".into()));
    let actors: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT DISTINCT actor, subject FROM decisions")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(actors, [("auto".to_string(), Some("alice".to_string()))]);

    app.execute(&["ALTER TABLE rules RENAME TO rules_gone"]).await;
    crate::jobs::scheduler::follow_sync(&app.state, &by).await;
    let followed = app.state.post_sync.lock().await.take();
    followed.expect("nothing followed the sync").await.unwrap();
    let failed =
        app.count("SELECT COUNT(*) FROM jobs WHERE kind = 'simulate' AND status = 'failed'");
    assert_eq!(failed.await, 1, "a failed simulation after a sync left no task");
}

#[tokio::test]
async fn the_sync_all_route_covers_every_enabled_instance() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &arr.base_url).await;

    let body = app.post("/api/v1/instances/sync", serde_json::json!({})).await;
    // One report per instance, in a list: the shape the Instances screen reads.
    let reports = body.assert_ok().as_array().unwrap().clone();
    assert_eq!(reports.len(), 2, "got {reports:?}");
}

/// Asked not to wait, a sync of every instance answers its own task, which
/// ends holding one report per instance. Following the first instance's task
/// instead would answer one report where the call answers two.
#[tokio::test]
async fn a_sync_of_every_instance_can_be_followed_to_every_report() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &arr.base_url).await;

    let started = app.send(preferring_async("/api/v1/instances/sync", serde_json::json!({}))).await;
    assert_eq!(started.status, axum::http::StatusCode::ACCEPTED, "{:?}", started.json);
    let task = finished(&app, started.json["job_id"].as_str().unwrap()).await;

    assert_eq!(task["kind"], "sync_all");
    assert_eq!(task["status"], "success");
    let reports = task["result"].as_array().expect("the task holds no reports");
    let synced: Vec<&str> = reports.iter().map(|r| r["instance_id"].as_str().unwrap()).collect();
    assert_eq!(synced, ["inst-1", "inst-2"]);
    // Each instance keeps its own task beside it.
    let each = app.count("SELECT COUNT(*) FROM jobs WHERE kind = 'sync'").await;
    assert_eq!(each, 2);
}

/// Port 1 on the loopback: nothing answers there, and no resolver is asked.
const NOWHERE: &str = "http://127.0.0.1:1";

/// Every instance failing is a failed task, which still says why instance by
/// instance. One of two failing is a partial result, not a failure.
#[tokio::test]
async fn a_sync_of_every_instance_fails_only_when_every_instance_does() {
    let up = FakeArr::start().await;
    for (second, outcome) in [(NOWHERE, "failed"), (up.base_url.as_str(), "success")] {
        let app = TestApp::new().await;
        app.seed_instance_at("inst-1", "radarr", NOWHERE).await;
        app.seed_instance_at("inst-2", "radarr", second).await;

        let started =
            app.send(preferring_async("/api/v1/instances/sync", serde_json::json!({}))).await;
        let task = finished(&app, started.json["job_id"].as_str().unwrap()).await;

        assert_eq!(task["status"], outcome, "{task}");
        assert_eq!(task["result"].as_array().map(Vec::len), Some(2), "{task}");
    }
}

/// Syncing every instance syncs the enabled ones: a switched-off instance is
/// one its owner stopped, and is left alone.
#[tokio::test]
async fn syncing_every_instance_leaves_a_disabled_one_alone() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_instance_at("inst-off", "radarr", &arr.base_url).await;
    app.execute(&["UPDATE instances SET enabled = 0 WHERE id = 'inst-off'"]).await;

    let reports = sync::sync_all_instances(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let synced: Vec<&str> = reports.iter().map(|r| r.instance_id.as_str()).collect();
    assert_eq!(synced, ["inst-1"]);
    assert_eq!(app.count("SELECT COUNT(*) FROM media WHERE instance_id = 'inst-off'").await, 0);
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
    assert_eq!(report["root_folders"], 3, "got {report}");
    // `/movies/anime` reports itself not accessible, and the form says so.
    assert_eq!(report["inaccessible_root_folders"], 1, "got {report}");
}

/// A `for` loop with an `.await` in it syncs instances one after another. The
/// cost is not the database (`do_sync` fetches everything before it opens its
/// transaction) but the waiting: an unreachable Arr costs the full HTTP
/// timeout, and two of them make the button look dead for twice that.
///
/// The fake counts how many listings it ever had open at once. A sequential
/// caller can only ever reach one, whatever the machine is doing, because it
/// does not issue the second request until the first has answered. Past the
/// bound, a pass would hold every connection of the pool at once.
#[tokio::test]
async fn syncing_every_instance_does_them_at_the_same_time_within_the_bound() {
    let arr = FakeArr::observing_concurrency().await;
    let app = TestApp::new().await;
    let instances = sync::SYNC_CONCURRENCY + 2;
    for index in 0..instances {
        let kind = if index % 2 == 0 { "radarr" } else { "sonarr" };
        app.seed_instance_at(&format!("inst-{index}"), kind, &arr.base_url).await;
    }

    let reports = sync::sync_all_instances(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(reports.len(), instances, "every instance should report");
    assert!(
        arr.max_concurrent() >= 2,
        "the fake never had more than {} request open at once, which is what a \
         sequential loop produces",
        arr.max_concurrent()
    );
    assert!(arr.max_concurrent() <= sync::SYNC_CONCURRENCY, "{} at once", arr.max_concurrent());
}

/// Concurrency must not reach the caller as reordering: the reports are what
/// the API returns and what the interface lists, and a set of rows that shuffle
/// between two runs is a table nobody can read. `buffered` overlaps the work
/// and still yields in the order the instances were listed, which
/// `buffer_unordered` would not.
#[tokio::test]
async fn the_reports_keep_the_order_the_instances_were_listed_in() {
    // The first listed answers last, so a loop yielding in completion order
    // puts it at the end.
    let slow = FakeArr::holding_edits(std::time::Duration::from_millis(150)).await;
    let fast = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &slow.base_url).await;
    app.seed_instance_at("inst-2", "sonarr", &fast.base_url).await;
    app.seed_instance_at("inst-3", "radarr", &fast.base_url).await;

    let expected: Vec<String> =
        app.state.instances(true).await.unwrap().iter().map(|i| i.id.clone()).collect();
    let reports = sync::sync_all_instances(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(
        reports.iter().map(|r| r.instance_id.clone()).collect::<Vec<_>>(),
        expected,
        "the reports came back in a different order from the instance list"
    );
}

// ------------------------------------------------- declared destinations

/// A declared destination takes the free space and the reachability of the
/// deepest folder the Arr reports above it, at every sync: kept from the day
/// it was typed, they would vouch for a disk that filled up or went to sleep.
#[tokio::test]
async fn a_declared_destination_follows_the_figures_of_the_folder_above_it() {
    let arr = FakeArr::start().await;
    arr.report_root_folder(
        serde_json::json!({ "id": 9, "path": "/movies", "freeSpace": 999_999, "accessible": true }),
    );
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO root_folders (id, instance_id, arr_id, path, free_space, accessible,
                                            origin)
                   VALUES ('rf-declared', 'i-1', NULL, '/movies/anime/kids', 1, 1, 'declared')",
    ])
    .await;

    sync::sync_instance(&app.state, "i-1", &crate::jobs::Attribution::manual(None)).await.unwrap();

    let figures: (i64, bool) =
        sqlx::query_as("SELECT free_space, accessible FROM root_folders WHERE id = 'rf-declared'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(figures, (2048, false), "not the figures of /movies/anime, the folder above it");
}

/// A target need not be a root folder in Radarr or Sonarr, or routing into
/// `/movies/anime/kids` would mean declaring it *there* first. What an operator
/// wants is one root folder per Arr and the targets beneath it declared here.
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
/// root's volume, so its free space and its reachability are known, which is
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
        // 2048 bytes free: the deepest match, not `/movies`.
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

/// `/movies/anime/../kids` is `/movies/kids`, whatever its letters say: it
/// would take the free space and reachability of a folder it is not under.
#[tokio::test]
async fn a_destination_with_a_parent_segment_is_refused() {
    let app = TestApp::new().await;
    // An Arr that cannot be asked does not block a save, so the refusal is
    // this one alone.
    app.seed_instance_at("i-1", "radarr", "http://127.0.0.1:1").await;

    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/anime/../kids" }),
        )
        .await;

    assert_eq!(refused.status, 400, "{}", refused.json);
    let message = refused.message();
    assert!(message.contains("without") && message.contains("/movies/anime/../kids"), "{message}");
    assert_eq!(app.count("SELECT COUNT(*) FROM root_folders").await, 0);
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
/// row carrying the same path, and the orphan cleanup that removes the old one
/// runs after the loop, in the same transaction. Under a unique index on the
/// path the insert would fail, the transaction roll back, and that instance
/// never synchronise again.
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

/// A folder as a previous pass left it, with the category mapped onto it.
async fn left_by_a_previous_pass(
    app: &TestApp,
    arr_id: Option<i64>,
    path: &str,
    category: &str,
    origin: &str,
) {
    sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES (?, ?)")
        .bind(format!("cat-{category}"))
        .bind(category)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category,
         last_synced_at, origin)
         VALUES (?, 'i-1', ?, ?, 1, ?, 'a-previous-pass', ?)",
    )
    .bind(format!("rf-old-{path}"))
    .bind(arr_id)
    .bind(path)
    .bind(category)
    .bind(origin)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

async fn category_of(app: &TestApp, path: &str) -> Option<String> {
    sqlx::query_scalar("SELECT category FROM root_folders WHERE path = ?")
        .bind(path)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

/// A rebuilt Arr hands its folder ids out again, to other paths. The category
/// was set on a path, and a folder that merely inherits the id is not that
/// folder: carried over, the next simulation moves the library into it.
#[tokio::test]
async fn a_folder_the_arr_reuses_an_id_for_does_not_take_the_old_category_with_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    // Id 1 was `/movies/old-anime`. The Arr now reports `/movies/standard` under it.
    left_by_a_previous_pass(&app, Some(1), "/movies/old-anime", "anime", "arr").await;

    sync::sync_instance(&app.state, "i-1", &crate::jobs::Attribution::manual(None)).await.unwrap();

    assert_eq!(category_of(&app, "/movies/standard").await, None, "the category followed the id");
}

/// The same path under a new id is the same folder, and keeps its category.
#[tokio::test]
async fn a_category_follows_its_folder_through_a_renumbering() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    left_by_a_previous_pass(&app, Some(99), "/movies/kids", "kids", "arr").await;

    sync::sync_instance(&app.state, "i-1", &crate::jobs::Attribution::manual(None)).await.unwrap();

    assert_eq!(category_of(&app, "/movies/kids").await.as_deref(), Some("kids"));
}

/// A declared path the Arr adopts is promoted to the id the Arr gives it. A
/// stale row still holding that id would make the promotion break the unique
/// id, and the instance's whole sync fail on every pass.
#[tokio::test]
async fn a_declared_path_adopted_under_a_taken_id_does_not_fail_the_sync() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    // The Arr reports `/movies/kids` under id 3, which a folder it dropped held.
    left_by_a_previous_pass(&app, None, "/movies/kids", "kids", "declared").await;
    left_by_a_previous_pass(&app, Some(3), "/movies/gone", "gone", "arr").await;

    let synced = app.post("/api/v1/instances/i-1/sync", serde_json::json!({})).await;

    assert_eq!(synced.status, 200, "the adoption failed the sync: {}", synced.json);
    let (rows, arr_id, origin): (i64, Option<i64>, String) = sqlx::query_as(
        "SELECT COUNT(*), MAX(arr_id), MAX(origin) FROM root_folders WHERE path = '/movies/kids'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!((rows, arr_id, origin.as_str()), (1, Some(3), "arr"));
    assert_eq!(category_of(&app, "/movies/kids").await.as_deref(), Some("kids"));
}

/// A failing tag endpoint fails no sync, since tags are one signal among
/// many, and strips the library of no tag: every `tag_in` rule would stop
/// matching until a later pass read them again. The catalogue the last good
/// pass stored resolves the ids meanwhile, for a full sync and for the
/// webhook's one item alike.
#[tokio::test]
async fn a_failing_tag_endpoint_keeps_the_tags_the_last_pass_read() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;
    let tags = || async {
        sqlx::query_scalar::<_, String>("SELECT tags FROM media")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    };
    sync::sync_instance(&app.state, "i-1", &crate::jobs::Attribution::manual(None)).await.unwrap();
    let read = tags().await;
    assert!(read.contains("anime"), "the fixture carries no tag to begin with: {read}");

    arr.break_tag_endpoint();
    sync::sync_instance(&app.state, "i-1", &crate::jobs::Attribution::manual(None)).await.unwrap();
    assert_eq!(tags().await, read, "a full sync stripped the tags");

    app.post(
        "/api/v1/webhook/i-1/tok",
        serde_json::json!({ "eventType": "Download", "movie": { "id": 10 } }),
    )
    .await
    .assert_ok();
    assert_eq!(tags().await, read, "the webhook's sync stripped the tags");
}

/// A declared folder has no id in the Arr, and must not publish one.
///
/// `arr_id` is nullable since the row can be Routarr's own. Typed as `i64`,
/// sqlx decodes the NULL to `0`, and every declared destination would go out
/// over the wire carrying a fabricated Arr id.
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
/// The endpoint answers about the directory above the one asked, so a typo in
/// the last segment still comes back with a listing, of the folder that does
/// exist. Only an entry naming the whole path verifies it.
#[tokio::test]
async fn a_typo_in_the_last_segment_is_not_verified() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    // `/movies/anime` exists, and `/movies/anmie` does not.
    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/anmie" }),
        )
        .await;
    assert_eq!(refused.status, 400, "a misspelling was verified: {}", refused.json);
    assert!(refused.message().contains("/movies/anmie"), "{}", refused.message());
}

/// A path is checked exactly as it is stored. On the Arr's filesystem a
/// backslash is a character of a folder name, so `/movies/anime\` names a
/// folder beside `/movies/anime` that does not exist, and moves sent there
/// would create it.
#[tokio::test]
async fn a_path_is_checked_exactly_as_it_is_stored() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    let refused = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/anime\\" }),
        )
        .await;
    assert_eq!(refused.status, 400, "a folder that does not exist was verified: {}", refused.json);
}

/// A folder at the top of the Arr's filesystem is found in the listing of its
/// root, the listing a query ending in a separator never reaches.
#[tokio::test]
async fn a_folder_at_the_top_of_the_filesystem_is_verified() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    let created = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies" }),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["verified"], true, "{}", created.json);
}

/// A folder name is sent as a query value, where a space, an ampersand or a
/// plus sign mean something else unless encoded.
#[tokio::test]
async fn a_folder_whose_name_needs_encoding_is_verified() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("i-1", "radarr", &arr.base_url).await;

    let created = app
        .post(
            "/api/v1/root-folders",
            serde_json::json!({ "instance_id": "i-1", "path": "/movies/Kids & Family+" }),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.json);
    assert_eq!(created.json["verified"], true, "{}", created.json);
}

/// Every declared-destination test above goes through a *Radarr* instance, and
/// `SonarrClient` asks through a `directory_exists` of its own. An operator
/// naming `/tv/anime` under Sonarr is exactly what this feature is for.
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
