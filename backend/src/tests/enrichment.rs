//! Enrichment against a live fake TMDB. Results come back out of order, and paired with the
//! input by position a movie's metadata is filed under `series`.

use crate::services::enrichment;

use super::TestApp;
use super::fake_tmdb::FakeTmdb;

/// A library with an instance and the given `(arr_id, media_type, tmdb_id)` items.
async fn library(tmdb: &FakeTmdb, items: &[(i64, &str, i64)]) -> TestApp {
    library_configured(tmdb, items, |_| {}).await
}

/// The same, with the configuration `adjust` leaves.
async fn library_configured(
    tmdb: &FakeTmdb,
    items: &[(i64, &str, i64)],
    adjust: impl FnOnce(&mut crate::config::Config),
) -> TestApp {
    let app = TestApp::new().await;

    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    adjust(&mut config);
    let state = app.state.clone().with_config(config);
    let app = TestApp::around(state);
    // What startup does with a key from the environment: list TMDB.
    crate::services::maintenance::converge_metadata_sources(&app.state).await.unwrap();

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Arr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
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

async fn cached(app: &TestApp) -> Vec<(i64, String, String, String, String, String)> {
    sqlx::query_as(
        "SELECT CAST(external_id AS INTEGER), media_type, genres, keywords, certifications,
         origin_countries FROM metadata_cache WHERE source = 'tmdb'
         ORDER BY CAST(external_id AS INTEGER), media_type",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

/// TMDB writes ISO 639-1 but for two codes. `cn`, its Cantonese, matches no
/// rule written against `zh`, and `xx`, no language, claims the field so that
/// no lower source fills it.
#[tokio::test]
async fn tmdb_s_two_codes_outside_iso_are_read_as_a_rule_is_written() {
    use super::fake_tmdb::{CANTONESE, NO_LANGUAGE};
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", CANTONESE), (2, "movie", NO_LANGUAGE)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let languages: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT CAST(external_id AS INTEGER), original_language FROM metadata_cache
         WHERE source = 'tmdb' ORDER BY CAST(external_id AS INTEGER)",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(languages, [(CANTONESE, Some("zh".to_string())), (NO_LANGUAGE, None)]);
}

/// TMDB's score is cached with its votes, under its own scale, beside what the row answered.
#[tokio::test]
async fn tmdb_s_score_is_cached_with_its_votes() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(8392, "movie", 8392)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT ratings FROM metadata_cache WHERE source = 'tmdb'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored, serde_json::json!({ "tmdb": { "value": 7.9, "votes": 1200 } }));
}

/// A v4 token in the query string would be written into the access log of every mirror and
/// proxy on the path.
#[tokio::test]
async fn the_tmdb_credential_travels_the_way_its_kind_is_read() {
    const V4_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ4In0.c2lnbmF0dXJl";
    for (credential, expected) in [
        ("tmdb-key", (Some("tmdb-key".to_string()), None)),
        (V4_TOKEN, (None, Some(format!("Bearer {V4_TOKEN}")))),
    ] {
        let tmdb = FakeTmdb::start().await;
        let app = library_configured(&tmdb, &[(1, "movie", 100)], |config| {
            config.tmdb_api_key = Some(credential.into());
        })
        .await;

        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();

        let sent = tmdb.recorded().credentials.clone();
        assert_eq!(sent, vec![expected], "for {credential}");
    }
}

#[tokio::test]
async fn a_retry_after_from_the_source_holds_back_the_next_request() {
    let tmdb = FakeTmdb::throttling_once(1).await;
    // One request at a time, so the second leaves after the first is answered.
    let app = library_configured(&tmdb, &[(1, "movie", 100), (2, "movie", 101)], |config| {
        config.metadata_concurrency = 1;
    })
    .await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let arrivals = tmdb.recorded().arrivals.clone();
    assert_eq!(arrivals.len(), 2, "the pass should ask once per item");
    let gap = arrivals[1] - arrivals[0];
    assert!(gap >= std::time::Duration::from_secs(1), "the next request left after {gap:?}");
}

/// Sonarr sends one delivery per imported episode, each enriching the series. What the cache
/// holds is not fetched again, a series TMDB does not have included, whose 404 is an answer.
#[tokio::test]
async fn a_second_delivery_for_the_same_series_fetches_nothing() {
    for (missing, id) in [(vec![], 1399), (vec![1400], 1400)] {
        let tmdb = FakeTmdb::with(missing, vec![]).await;
        let app = library(&tmdb, &[(1, "series", id)]).await;

        let media = crate::api::media::load_media(&app.state, "m-1").await.unwrap();
        for _ in 0..2 {
            enrichment::enrich_one(&app.state, &media).await.unwrap();
        }

        let asked = format!("/tv/{id}");
        let fetched = tmdb.recorded().paths.iter().filter(|p| p.starts_with(&asked)).count();
        assert_eq!(fetched, 1, "series {id}, cached, was fetched again");
    }
}

#[tokio::test]
async fn enriches_movies_and_series_from_one_call_each() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
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
    // The movie, 100, answers slowly, so its result comes back after the series'. Paired by
    // position, its metadata would be filed under `series` and both rows lost.
    let tmdb = FakeTmdb::with(vec![], vec![100]).await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

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

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let rows = cached(&app).await;

    let movie = rows.iter().find(|r| r.0 == 100).unwrap();
    assert_eq!(movie.4, r#"{"FR":"Tous publics","US":"PG"}"#, "every country's rating is kept");
    assert!(movie.5.contains("JP"), "origin falls back to production_countries: {}", movie.5);
    assert!(movie.3.contains("anime"), "movie keywords: {}", movie.3);

    let series = rows.iter().find(|r| r.0 == 200).unwrap();
    assert_eq!(series.4, r#"{"US":"TV-14"}"#, "series use content_ratings");
    assert!(series.3.contains("documentary"), "series keywords come under `results`");
}

/// TMDB's "Released" and "Ended" read as Radarr and Sonarr write them.
#[tokio::test]
async fn a_status_is_stored_in_the_words_the_arrs_use() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let statuses: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT media_type, status FROM metadata_cache ORDER BY media_type")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    let statuses: Vec<(&str, Option<&str>)> =
        statuses.iter().map(|(kind, status)| (kind.as_str(), status.as_deref())).collect();
    assert_eq!(statuses, [("movie", Some("released")), ("series", Some("ended"))]);
}

#[tokio::test]
async fn the_same_title_in_two_instances_is_fetched_once() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "movie", 100)]).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.considered, 1, "targets are deduplicated");
    assert_eq!(tmdb.recorded().paths.len(), 1);
}

#[tokio::test]
async fn one_failing_item_does_not_abort_the_pass() {
    let tmdb = FakeTmdb::erroring(vec![100]).await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!((report.enriched, report.failed), (1, 1));
    let rows = cached(&app).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 200, "the reachable item was still cached");
}

/// A 400, 410 or 422 refuses that title alone, so it is stored empty.
#[tokio::test]
async fn a_title_the_source_refuses_is_not_asked_again_next_pass() {
    for status in [400, 410, 422] {
        let tmdb = FakeTmdb::down(status).await;
        let app = library(&tmdb, &[(1, "movie", 100)]).await;

        for _ in 0..2 {
            enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
                .await
                .unwrap();
        }

        assert_eq!(tmdb.recorded().paths.len(), 1, "a {status} was asked again");
    }
}

/// The breaker counts failures in a row, so one title in four refused costs those titles alone.
#[tokio::test]
async fn scattered_failures_do_not_abandon_a_working_source() {
    let ids: Vec<i64> = (100..132).collect();
    let refused: Vec<i64> = ids.iter().copied().filter(|id| id % 4 == 0).collect();
    let tmdb = FakeTmdb::erroring(refused.clone()).await;
    let titles: Vec<(i64, &str, i64)> =
        ids.iter().enumerate().map(|(n, id)| (n as i64 + 1, "movie", *id)).collect();
    let app = library(&tmdb, &titles).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.skipped, 0, "{report:?}");
    assert_eq!((report.enriched, report.failed), (ids.len() - refused.len(), refused.len()));
}

/// The others are written as they arrive, and the next pass tries the failed one.
#[tokio::test]
async fn one_answer_that_cannot_be_stored_does_not_abort_the_pass() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100), (2, "series", 200)]).await;
    app.execute(&["CREATE TRIGGER a_full_disk BEFORE INSERT ON metadata_cache
                   WHEN NEW.external_id = '100'
                   BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END"])
        .await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .expect("the pass ended");

    assert_eq!((report.enriched, report.failed), (1, 1));
    let rows = cached(&app).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 200, "the answer that could be stored was not");
}

#[tokio::test]
async fn fresh_entries_are_not_re_fetched() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let second = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(second.considered, 0, "a fresh cache entry is left alone");
    assert_eq!(tmdb.recorded().paths.len(), 1);
}

#[tokio::test]
async fn expired_entries_are_re_fetched() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    sqlx::query("UPDATE metadata_cache SET cached_at = datetime('now', '-8 days')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let second = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
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

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    assert_eq!(report.considered, 0);
    assert!(tmdb.recorded().paths.is_empty());
}

/// Every request would be refused. The same library with a key is the control.
#[tokio::test]
async fn enrichment_is_a_no_op_without_an_api_key() {
    let tmdb = FakeTmdb::start().await;
    let keyed = library(&tmdb, &[(1, "movie", 100)]).await;
    let keyless = TestApp::around(keyed.state.clone().with_config(crate::config::Config {
        tmdb_api_key: None,
        ..(*keyed.state.config).clone()
    }));
    keyless.store_setting("metadata_providers", "arr,tmdb").await;

    let report =
        enrichment::enrich_all_media(&keyless.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();
    assert_eq!(report.considered, 0, "{report:?}");
    assert!(tmdb.recorded().paths.is_empty(), "a keyless source was asked");

    enrichment::enrich_all_media(&keyed.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    assert!(!tmdb.recorded().paths.is_empty(), "the control: the keyed source was asked");
}

#[tokio::test]
async fn a_concurrent_pass_is_refused() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;
    let _held = app.state.jobs.try_lock("enrich").expect("first lock");

    assert!(matches!(
        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None)).await,
        Err(crate::error::AppError::InProgress { reason: "enrichment_running", .. })
    ));
}

#[tokio::test]
async fn the_pass_is_recorded_as_a_job() {
    let tmdb = FakeTmdb::start().await;
    let app = library(&tmdb, &[(1, "movie", 100)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let (kind, status): (String, String) =
        sqlx::query_as("SELECT kind, status FROM jobs ORDER BY started_at DESC LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), status.as_str()), ("enrich", "success"));
}

#[tokio::test]
async fn enriched_metadata_reaches_the_rule_engine() {
    // A genre rule that cannot match before enrichment matches after it.
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
        &app.state.library_passes,
        crate::services::routing::SimulationOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(before.no_category_match, 1, "no metadata yet, nothing can match");

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let after = crate::services::routing::run_simulation(
        &app.state.pool,
        &app.state.library_passes,
        crate::services::routing::SimulationOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(after.moves_required, 1);
    assert_eq!(after.decisions[0].target_root_folder.as_deref(), Some("/movies/anime"));
}

/// A 404 from a TMDB that answers other titles says it does not have that title, and nothing
/// about the next one: ten unknown titles in a row leave the twelfth to be asked and stored.
#[tokio::test]
async fn titles_tmdb_does_not_have_never_cut_it_off() {
    let unknown: Vec<i64> = (2..=11).collect();
    let tmdb = FakeTmdb::with(unknown.clone(), vec![]).await;
    let mut items: Vec<(i64, &str, i64)> = unknown.iter().map(|id| (*id, "movie", *id)).collect();
    items.extend([(1, "movie", 1), (12, "movie", 500)]);
    let app = library(&tmdb, &items).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.skipped, 0, "{report:?}");
    let asked = tmdb.recorded().paths.clone();
    assert_eq!(asked.len(), 12, "{asked:?}");
    // Each answered, ten of them with nothing.
    assert_eq!(report.enriched, 12, "{report:?}");
    let described: Vec<i64> =
        cached(&app).await.into_iter().filter(|row| row.2 != "[]").map(|row| row.0).collect();
    assert_eq!(described, [1, 500]);
}

/// TMDB down answers every title alike, so the pass stops at the breaker and leaves the rest to
/// the next one.
#[tokio::test]
async fn a_tmdb_outage_is_abandoned_rather_than_asked_once_per_title() {
    let tmdb = FakeTmdb::down(503).await;
    let items: Vec<(i64, &str, i64)> = (1..=20).map(|id| (id, "movie", id)).collect();
    let app = library(&tmdb, &items).await;
    let in_flight = app.state.config.metadata_concurrency;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let asked = tmdb.recorded().paths.len();
    let ceiling = 5 + in_flight - 1;
    assert!(asked <= ceiling, "{asked} requests for 20 titles to a source that was down");
    assert!(report.skipped > 0, "{report:?}");
}

/// A TMDB answer lives six months at most, as its terms require, and lowering the setting
/// refreshes what is cached, or a stale answer outlives it by months.
#[tokio::test]
async fn a_cached_answer_lives_as_many_days_as_the_setting_says() {
    for (setting, days) in [(None, 7), (Some("1"), 1), (Some("365"), 180)] {
        let tmdb = FakeTmdb::start().await;
        let app = library(&tmdb, &[(1, "movie", 100)]).await;
        let by = crate::jobs::Attribution::manual(None);
        let pass = || enrichment::enrich_all_media(&app.state, &by);
        if let Some(value) = setting {
            app.save_setting("metadata_cache_ttl_days", "30").await.assert_ok();
            pass().await.unwrap();
            app.save_setting("metadata_cache_ttl_days", value).await.assert_ok();
        } else {
            pass().await.unwrap();
        }

        for (hours, asked) in [(days * 24 - 1, 1), (days * 24 + 1, 2)] {
            sqlx::query("UPDATE metadata_cache SET cached_at = datetime('now', ?)")
                .bind(format!("-{hours} hours"))
                .execute(&app.state.pool)
                .await
                .unwrap();
            pass().await.unwrap();
            assert_eq!(tmdb.recorded().paths.len(), asked, "{setting:?}, {hours} hours old");
        }
    }
}

#[tokio::test]
async fn a_title_tmdb_does_not_have_is_not_asked_again_next_pass() {
    let tmdb = FakeTmdb::with(vec![7], vec![]).await;
    let app = library(&tmdb, &[(1, "movie", 7)]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let asked = tmdb.recorded().paths.clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
}
