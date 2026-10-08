//! OMDb's daily quota: one count every caller takes from, and what the
//! interface says once it is spent.
//!
//! A free key answers 1,000 requests a UTC day and refuses every one after,
//! until midnight. A library larger than that is covered only if each day's
//! requests go to the titles never asked first, and only if the probes and
//! the lookups do not spend them.

use std::collections::HashSet;

use super::fake_sources::{FakeSources, OMDB_KEY};
use super::{TestApp, probe_verdicts};
use crate::jobs::Attribution;
use crate::services::enrichment::{self, EnrichmentReport};
use crate::services::quota::DailyQuota;

/// The film `one_film_on` holds and `more` films OMDb holds nothing for, read
/// from OMDb alone.
async fn films(sources: &FakeSources, more: i64) -> TestApp {
    let app = TestApp::one_film_on(sources, "omdb").await;
    for index in 0..more {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, imdb_id)
             VALUES (?, 'inst-1', ?, 'movie', ?, ?)",
        )
        .bind(format!("m-q{index}"))
        .bind(100 + index)
        .bind(format!("Film {index}"))
        .bind(format!("tt{:07}", 1000 + index))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    app
}

fn omdb_requests(sources: &FakeSources) -> usize {
    sources.recorded().paths.iter().filter(|path| *path == "/omdb").count()
}

fn omdb_asked(sources: &FakeSources) -> HashSet<String> {
    let recorded = sources.recorded();
    recorded
        .details
        .iter()
        .filter(|(source, _)| *source == "omdb")
        .map(|(_, id)| id.clone())
        .collect()
}

async fn enrich(app: &TestApp) -> EnrichmentReport {
    enrichment::enrich_all_media(&app.state, &Attribution::manual(None)).await.unwrap()
}

/// A day's quota goes to the titles never asked, then to the stalest answers:
/// asked in the order the table happens to hold them, the same titles would be
/// refreshed each day and the rest never reached.
#[tokio::test]
async fn titles_never_asked_come_before_refreshes_the_stalest_first() {
    let sources = FakeSources::start().await;
    let app = films(&sources, 29).await;
    // Ten answers past their seven days, the one for tt0001000 by a day,
    // tt0001009 by ten.
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, cached_at)
         SELECT 'omdb', imdb_id, 'movie',
                datetime('now', '-' || (arr_id - 92) || ' days')
           FROM media WHERE arr_id BETWEEN 100 AND 109",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.store_setting("omdb_daily_requests", "25").await;

    enrich(&app).await;

    let never_asked = (1010..1029).map(|n| format!("tt{n:07}")).chain(["tt0096283".to_string()]);
    let stalest = (1005..1010).map(|n| format!("tt{n:07}"));
    assert_eq!(omdb_asked(&sources), never_asked.chain(stalest).collect::<HashSet<_>>());
}

/// The count stops a pass before OMDb refuses anything, the next pass of the
/// day asks nothing, and the next day starts a count of its own.
#[tokio::test]
async fn a_pass_stops_at_the_daily_quota_and_the_rest_waits_for_the_next_day() {
    let sources = FakeSources::start().await;
    let app = films(&sources, 29).await;
    app.store_setting("omdb_daily_requests", "10").await;

    let report = enrich(&app).await;
    assert_eq!(omdb_requests(&sources), 10);
    assert_eq!((report.enriched, report.failed, report.skipped, report.deferred), (10, 0, 0, 20));

    let report = enrich(&app).await;
    assert_eq!(omdb_requests(&sources), 10, "the day's quota was asked past");
    assert_eq!(report.deferred, 20);

    app.execute(&["UPDATE source_requests SET day = date('now', '-1 day')"]).await;
    enrich(&app).await;
    assert_eq!(omdb_requests(&sources), 20);
}

/// OMDb refusing for its quota closes the day before the count does, as when
/// another program shares the key: the requests already sent are the last,
/// nothing is cached as a miss, and the diagnostics name the quota.
#[tokio::test]
async fn a_spent_omdb_quota_closes_the_day_and_caches_nothing() {
    let sources = FakeSources::start().await;
    let app = films(&sources, 29).await;
    sources.spend_omdb_quota();

    let report = enrich(&app).await;

    let in_flight = app.state.config.metadata_concurrency;
    assert!(omdb_requests(&sources) <= in_flight, "asked past the refusal");
    assert_eq!(report.skipped, 0, "the quota was taken for an outage");
    let cached = app.count("SELECT COUNT(*) FROM metadata_cache").await;
    assert_eq!(cached, 0, "a refusal was cached as a miss");

    let asked = omdb_requests(&sources);
    enrich(&app).await;
    assert!(probe_verdicts(&app).await.contains(&"source_quota_spent".to_string()));
    assert_eq!(omdb_requests(&sources), asked, "the closed day was asked again");
}

/// A probe asks OMDb once a day, from the same quota. A new key or a new
/// quota is probed afresh the same day, whichever way it is written, and a
/// spent quota is said rather than taken for a refused key.
#[tokio::test]
async fn omdb_is_probed_once_a_day_from_its_quota() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "omdb").await;

    assert!(probe_verdicts(&app).await.is_empty());
    assert!(probe_verdicts(&app).await.is_empty());
    assert_eq!(omdb_requests(&sources), 1);

    app.save_setting("omdb_api_key", OMDB_KEY).await.assert_ok();
    assert!(probe_verdicts(&app).await.is_empty());
    assert_eq!(omdb_requests(&sources), 2, "a new key was not probed");

    app.store_setting("omdb_daily_requests", "2").await;
    assert_eq!(probe_verdicts(&app).await, ["source_quota_spent"]);
    assert_eq!(omdb_requests(&sources), 2, "the probe asked past the quota");
    let status = app.get("/api/v1/status").await;
    let warnings = status.assert_ok()["warnings"].as_array().unwrap().clone();
    let warning = warnings.iter().find(|warning| warning["code"] == "source_quota_spent").unwrap();
    let said = app.state.localizer().await.translate("WarnProviderQuota", &[("provider", "OMDb")]);
    assert_eq!(warning["message"], said);
}

/// The lookups of `GET /route` and of the webhook spend the same day as the
/// enrichment.
#[tokio::test]
async fn a_lookup_takes_from_the_same_daily_quota() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "omdb").await;
    app.store_setting("omdb_daily_requests", "1").await;

    app.get("/api/v1/route?type=movie&imdb=tt0096283&enrich=true").await.assert_ok();
    assert_eq!(omdb_requests(&sources), 1);

    let report = enrich(&app).await;
    assert_eq!(omdb_requests(&sources), 1, "the pass asked past the day's quota");
    assert_eq!(report.deferred, 1);
}

/// What a pass reserves counts at once, and what it leaves unsent is given
/// back. A raised quota opens a day the count had closed, and the first
/// reservation of a day starts a new count.
#[tokio::test]
async fn the_count_is_kept_per_day_and_a_raised_quota_opens_it_again() {
    let app = TestApp::new().await;
    let pool = &app.state.pool;
    let two = DailyQuota::new("omdb", 2);

    assert_eq!(two.reserve(pool, 5).await.unwrap(), 2);
    assert_eq!(two.reserve(pool, 1).await.unwrap(), 0);
    two.give_back(pool, 1).await.unwrap();
    assert_eq!(two.reserve(pool, 5).await.unwrap(), 1);
    assert_eq!(DailyQuota::new("omdb", 3).reserve(pool, 5).await.unwrap(), 1);

    two.exhaust(pool).await.unwrap();
    assert_eq!(two.reserve(pool, 1).await.unwrap(), 0, "an exhausted day was opened");
    app.execute(&["UPDATE source_requests SET day = date('now', '-1 day')"]).await;
    two.give_back(pool, 2).await.unwrap();
    assert_eq!(two.reserve(pool, 1).await.unwrap(), 1, "yesterday's count closed today");
    assert_eq!(app.count("SELECT spent FROM source_requests").await, 1);
}

/// A pass the breaker stops gives the requests it never sent back to the day,
/// which counts only what reached the source.
#[tokio::test]
async fn requests_a_stopped_pass_never_sent_are_given_back() {
    let sources = FakeSources::failing(503).await;
    let app = films(&sources, 29).await;

    let report = enrich(&app).await;

    assert!(report.skipped > 0, "the breaker never opened");
    let sent = i64::try_from(omdb_requests(&sources)).unwrap();
    assert_eq!(app.count("SELECT spent FROM source_requests WHERE source = 'omdb'").await, sent);
}
