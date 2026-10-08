//! A refresh asked of the sources: one title at once, or a source's answers
//! through a pass.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;
use super::fake_sources::FakeSources;
use crate::jobs::Attribution;
use crate::services::enrichment;

/// The reads and the searches each source was sent.
fn asked(sources: &FakeSources, source: &str) -> (usize, usize) {
    let recorded = sources.recorded();
    let count =
        |list: &[(&'static str, String)]| list.iter().filter(|(id, _)| *id == source).count();
    (count(&recorded.details), count(&recorded.searches))
}

async fn enriched(order: &str) -> (FakeSources, TestApp) {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, order).await;
    enrichment::enrich_all_media(&app.state, &Attribution::manual(None)).await.unwrap();
    (sources, app)
}

/// A title asked again is read again from each source, and searched again
/// where it was found by a search, so a wrong AniList or MyAnimeList match
/// can be corrected. Its answers were current, so nothing but the refresh
/// asks for them.
#[tokio::test]
async fn a_refreshed_title_is_read_and_searched_again() {
    let (sources, app) = enriched("arr,jikan,omdb").await;
    assert_eq!((asked(&sources, "jikan"), asked(&sources, "omdb")), ((1, 1), (1, 0)));

    let answer = app.post("/api/v1/metadata/refresh", json!({ "media_id": "m-1" })).await;

    assert_eq!(answer.assert_ok()["answered"], 2);
    assert_eq!((asked(&sources, "jikan"), asked(&sources, "omdb")), ((2, 2), (2, 0)));
    let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache WHERE stale = 1")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stale, 0, "an answer read again is still marked due");
}

/// A source refreshed has every answer it gave read again by the pass that
/// follows, by the id it found, and the other sources' answers are left.
/// Asked for everything, every source's are.
#[tokio::test]
async fn a_refreshed_source_has_its_answers_read_again() {
    let (sources, app) = enriched("arr,jikan,omdb").await;

    app.post("/api/v1/metadata/refresh", json!({ "source": "omdb" })).await.assert_ok();
    assert_eq!((asked(&sources, "jikan"), asked(&sources, "omdb")), ((1, 1), (2, 0)));

    app.post("/api/v1/metadata/refresh", json!({})).await.assert_ok();
    assert_eq!((asked(&sources, "jikan"), asked(&sources, "omdb")), ((2, 1), (3, 0)));
}

/// A pass already running has counted its titles: a refresh then is refused
/// before it marks anything, or its answers would wait for a pass nobody
/// asked for.
#[tokio::test]
async fn a_refresh_is_refused_while_a_pass_runs() {
    let (_sources, app) = enriched("arr,omdb").await;
    let _running = app.state.jobs.try_lock("enrich").expect("no pass runs yet");

    let answer = app.post("/api/v1/metadata/refresh", json!({ "source": "omdb" })).await;

    answer.assert_status(StatusCode::CONFLICT);
    let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache WHERE stale = 1")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stale, 0);
}

/// A refresh names a source Routarr asks and a title it holds.
#[tokio::test]
async fn a_refresh_names_a_source_routarr_asks_and_a_title_it_holds() {
    let (_sources, app) = enriched("arr,omdb").await;
    for (body, status) in [
        (json!({ "source": "arr" }), StatusCode::BAD_REQUEST),
        (json!({ "source": "imdb" }), StatusCode::BAD_REQUEST),
        (json!({ "media_id": "m-404" }), StatusCode::NOT_FOUND),
    ] {
        app.post("/api/v1/metadata/refresh", body.clone()).await.assert_status(status);
    }
}
