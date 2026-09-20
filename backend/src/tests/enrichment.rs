//! Enrichment against a live fake TMDb.
//!
//! The seam worth defending is attribution: results come back out of order, and
//! pairing them with the input list by position files a movie's metadata under
//! `series`.

use std::sync::Arc;

use crate::services::enrichment;
use crate::state::AppState;

use super::TestApp;
use super::fake_tmdb::FakeTmdb;

/// A library with an instance and the given `(arr_id, media_type, tmdb_id)` items.
async fn library(tmdb: &FakeTmdb, items: &[(i64, &str, i64)]) -> TestApp {
    let app = TestApp::new().await;

    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    let state = AppState { config: Arc::new(config), ..app.state.clone() };
    let app = TestApp { state, ..app };

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Arr', 'radarr', 'http://x', 'k', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    for (arr_id, media_type, tmdb_id) in items {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, monitored, has_files)
             VALUES (?, 'inst-1', ?, ?, ?, ?, 1, 1)",
        )
        .bind(format!("m-{arr_id}"))
        .bind(arr_id)
        .bind(*media_type)
        .bind(format!("Title {arr_id}"))
        .bind(tmdb_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    app
}

async fn cached(app: &TestApp) -> Vec<(i64, String, String, String, Option<String>, String)> {
    sqlx::query_as(
        "SELECT CAST(external_id AS INTEGER), media_type, genres, keywords, certification,
         origin_countries FROM metadata_cache WHERE source = 'tmdb'
         ORDER BY CAST(external_id AS INTEGER), media_type",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn enriches_movies_and_series_from_one_call_each() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    assert_eq!((report.considered, report.enriched, report.failed), (2, 2, 0));

    let rows = cached(&app).await;
    assert_eq!(rows.len(), 2);

    let requested = tmdb.recorded().paths.clone();
    assert_eq!(requested.len(), 2, "one request per media, not two: {requested:?}");
    assert!(
        requested.iter().all(|p| p.contains("keywords")),
        "keywords must be appended rather than fetched separately: {requested:?}"
    );
}

#[tokio::test]
async fn results_are_filed_against_the_media_they_belong_to() {
    // The movie answers slowly, so its result comes back *after* the series'.
    // Paired with the input list by position, the movie's metadata is filed
    // under `series` and both rows are lost.
    let tmdb = FakeTmdb::with(vec![], vec![100]).await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let rows = cached(&app).await;
    let movie = rows.iter().find(|r| r.0 == 100).expect("movie row");
    let series = rows.iter().find(|r| r.0 == 200).expect("series row");

    assert_eq!(movie.1, "movie");
    assert!(movie.2.contains("Animation"), "movie genres: {}", movie.2);
    assert_eq!(series.1, "series");
    assert!(series.2.contains("Drama"), "series genres: {}", series.2);
}

#[tokio::test]
async fn certifications_and_countries_are_extracted() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('certification_regions', 'FR, US')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    let rows = cached(&app).await;

    let movie = rows.iter().find(|r| r.0 == 100).unwrap();
    assert_eq!(movie.4.as_deref(), Some("Tous publics"), "the preferred region wins");
    assert!(movie.5.contains("JP"), "origin falls back to production_countries: {}", movie.5);
    assert!(movie.3.contains("anime"), "movie keywords: {}", movie.3);

    let series = rows.iter().find(|r| r.0 == 200).unwrap();
    assert_eq!(series.4.as_deref(), Some("TV-14"), "series use content_ratings");
    assert!(series.3.contains("documentary"), "series keywords come under `results`");
}

#[tokio::test]
async fn the_same_title_in_two_instances_is_fetched_once() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "movie", 100)]).await;

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    assert_eq!(report.considered, 1, "targets are deduplicated");
    assert_eq!(tmdb.recorded().paths.len(), 1);
}

#[tokio::test]
async fn one_failing_item_does_not_abort_the_pass() {
    let tmdb = FakeTmdb::with(vec![100], vec![]).await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    assert_eq!((report.enriched, report.failed), (1, 1));
    let rows = cached(&app).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 200, "the reachable item was still cached");
}

#[tokio::test]
async fn fresh_entries_are_not_re_fetched() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    let second = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    assert_eq!(second.considered, 0, "a fresh cache entry is left alone");
    assert_eq!(tmdb.recorded().paths.len(), 1);
}

#[tokio::test]
async fn expired_entries_are_re_fetched() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    sqlx::query("UPDATE metadata_cache SET expires_at = datetime('now', '-1 day')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let second = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    assert_eq!(second.enriched, 1);
    assert_eq!(tmdb.recorded().paths.len(), 2);
}

#[tokio::test]
async fn media_without_an_external_id_is_skipped() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[]).await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
         VALUES ('m-1', 'inst-1', 1, 'movie', 'Sans id', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    assert_eq!(report.considered, 0);
    assert!(tmdb.recorded().paths.is_empty());
}

#[tokio::test]
async fn enrichment_is_a_no_op_without_an_api_key() {
    let app = TestApp::new().await;
    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    assert_eq!(report.considered, 0);
}

#[tokio::test]
async fn a_concurrent_pass_is_refused() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;
    let _held = app.state.jobs.try_lock("enrich").expect("first lock");

    assert!(matches!(
        enrichment::enrich_all_media(&app.state, "manual").await,
        Err(crate::error::AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn the_pass_is_recorded_as_a_job() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let (kind, status): (String, String) =
        sqlx::query_as("SELECT kind, status FROM jobs ORDER BY started_at DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), status.as_str()), ("enrich", "success"));
}

#[tokio::test]
async fn enriched_metadata_reaches_the_rule_engine() {
    // The whole point of enrichment: a genre rule that could not match before
    // matches after.
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;
    sqlx::query("UPDATE media SET current_root_folder = '/movies/standard'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-1', 'inst-1', 1, '/movies/anime', 1, 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category)
         VALUES ('r1', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let before = crate::services::routing::run_simulation(
        &app.state.pool,
        crate::services::routing::SimulationOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(before.no_category_match, 1, "no metadata yet, nothing can match");

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let after = crate::services::routing::run_simulation(
        &app.state.pool,
        crate::services::routing::SimulationOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(after.moves_required, 1);
    assert_eq!(after.decisions[0].target_root_folder.as_deref(), Some("/movies/anime"));
}
