//! Live checks against the **real** metadata APIs: a fake written to match a client agrees with
//! it forever while the real API moves, and only a real request notices. Ignored by default, and
//! kept out of CI so a green build never depends on a third party's uptime.
//!
//! ```bash
//! cargo test live_sources -- --ignored --nocapture --test-threads=1   # keyless only
//! TMDB_API_KEY=… OMDB_API_KEY=… TVDB_API_KEY=… cargo test live_sources -- --ignored  # everything
//! ```
//!
//! A source with no credential skips itself, so the keyless pair runs for anyone.

use crate::config::Config;
use crate::integrations::anilist::AniListClient;
use crate::integrations::jikan::JikanClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tmdb::TmdbClient;
use crate::integrations::tvdb::TvdbClient;

/// The shipped HTTP client, its redirect policy and its timeout: the offline suite's is spent on
/// a loopback socket, and a real API outlasts it, which Jikan's test would read as the source down.
fn client() -> reqwest::Client {
    let config = Config {
        http_timeout: std::time::Duration::from_secs(crate::config::DEFAULT_HTTP_TIMEOUT_SECS),
        ..Config::for_tests()
    };
    crate::http::build_client(&config).expect("live http client")
}

/// The reference work: an anime film every one of these sources knows, with an
/// unambiguous title, year, IMDb id and TVDB id.
const TITLE: &str = "Tonari no Totoro";
const YEAR: i64 = 1988;
const IMDB_ID: &str = "tt0096283";
const TVDB_SERIES: &str = "76885"; // Cowboy Bebop, for the series path.

#[tokio::test]
#[ignore = "hits the real AniList API"]
async fn anilist_answers_the_shape_the_client_expects() {
    let anilist = AniListClient::new(client(), crate::integrations::anilist::DEFAULT_BASE_URL);

    let candidates = anilist.search(TITLE, "movie").await.expect("AniList search");
    assert!(!candidates.is_empty(), "AniList returned no candidate for {TITLE}");

    // A candidate must match the title once normalised and agree on the year: without
    // `startDate.year` the resolution silently stops working, and only this test would notice.
    let matched = candidates.iter().find(|c| {
        c.titles.iter().any(|t| {
            crate::services::rule_engine::normalise_value(t)
                == crate::services::rule_engine::normalise_value(TITLE)
        })
    });
    let matched = matched.expect("no AniList candidate matched on title");
    assert_eq!(matched.year, Some(YEAR), "AniList year drifted or stopped being returned");

    let details = anilist.get_details(matched.id).await.expect("AniList details");
    assert!(!details.genres.is_empty(), "AniList returned no genres");
    assert_eq!(details.origin_countries, vec!["JP"], "countryOfOrigin drifted");
    // Tags are the reason to have AniList at all.
    assert!(!details.keywords.is_empty(), "no tag cleared the agreement floor");
    assert!(details.ratings.contains_key("anilist"), "no AniList score read");
    println!("AniList {}: {:?} / {:?}", matched.id, details.genres, details.keywords);
}

/// True when the failure is the third party being down rather than the client
/// being wrong. Jikan proxies MyAnimeList and answers 5xx whenever MAL is
/// unavailable, which must not be reported as a defect here.
fn upstream_is_down(error: &crate::error::AppError) -> bool {
    matches!(
        error,
        crate::error::AppError::ExternalApi { status: 502..=504, .. }
            | crate::error::AppError::ExternalApi { status: 0, .. }
    )
}

#[tokio::test]
#[ignore = "hits the real Jikan API"]
async fn jikan_answers_the_shape_the_client_expects() {
    let jikan = JikanClient::new(client(), crate::integrations::jikan::DEFAULT_BASE_URL);

    let candidates = match jikan.search(TITLE, "movie").await {
        Ok(candidates) => candidates,
        Err(e) if upstream_is_down(&e) => {
            eprintln!("skipped: Jikan or MyAnimeList is unavailable ({e})");
            return;
        }
        Err(e) => panic!("Jikan search failed: {e}"),
    };
    assert!(!candidates.is_empty(), "Jikan returned no candidate for {TITLE}");

    let matched = candidates
        .iter()
        .find(|c| {
            c.titles.iter().any(|t| {
                crate::services::rule_engine::normalise_value(t)
                    == crate::services::rule_engine::normalise_value(TITLE)
            })
        })
        .expect("no Jikan candidate matched on title");
    // A film has no broadcast season: its year has to come from `aired`, or the
    // resolution refuses every film whose library knows its year.
    assert_eq!(matched.year, Some(YEAR), "Jikan dated {TITLE} as {:?}", matched.year);

    let details = match jikan.get_details(matched.id).await {
        Ok(details) => details,
        Err(e) if upstream_is_down(&e) => {
            eprintln!("skipped: Jikan or MyAnimeList is unavailable ({e})");
            return;
        }
        Err(e) => panic!("Jikan details failed: {e}"),
    };
    assert!(!details.genres.is_empty(), "Jikan returned no genres");
    // Themes/demographics are what this source uniquely brings.
    assert!(!details.keywords.is_empty(), "no themes or demographics came back");
    let mal = details.ratings.get("mal").expect("no MyAnimeList score read");
    assert!(mal.votes.is_some(), "MyAnimeList votes not read: {mal:?}");
    println!("Jikan {}: {:?} / {:?}", matched.id, details.genres, details.keywords);
}

#[tokio::test]
#[ignore = "hits the real TMDB API; needs TMDB_API_KEY"]
async fn tmdb_answers_the_shape_the_client_expects() {
    let Ok(key) = std::env::var("TMDB_API_KEY") else {
        eprintln!("skipped: TMDB_API_KEY is not set");
        return;
    };

    let tmdb = TmdbClient::new(client(), &key, crate::config::DEFAULT_TMDB_BASE_URL);

    let details = tmdb.get_details(8392, "movie").await.expect("TMDB details");
    assert!(details.genres.iter().any(|genre| genre == "Animation"), "{:?}", details.genres);
    assert_eq!(details.original_language.as_deref(), Some("ja"), "language drifted");
    assert_eq!(details.origin_countries, vec!["JP"], "country drifted");
    // The appended blocks: asked for, and answered.
    assert!(!details.keywords.is_empty(), "no keywords came back");
    assert!(details.certifications.contains_key("US"), "no US rating came back");
    let tmdb_score = details.ratings.get("tmdb").expect("no TMDB score read");
    assert!(tmdb_score.votes.is_some(), "TMDB votes not read: {tmdb_score:?}");
    println!("TMDB 8392: {:?} / {:?}", details.genres, details.certifications);
}

#[tokio::test]
#[ignore = "hits the real OMDb API; needs OMDB_API_KEY"]
async fn omdb_answers_the_shape_the_client_expects() {
    let Ok(key) = std::env::var("OMDB_API_KEY") else {
        eprintln!("skipped: OMDB_API_KEY is not set");
        return;
    };

    let omdb = OmdbClient::new(
        client(),
        &key,
        crate::integrations::omdb::DEFAULT_BASE_URL,
        crate::services::settings::default_of("omdb_daily_requests").parse().unwrap(),
    );

    assert!(omdb.test_connection().await.expect("OMDb probe"), "OMDb rejected the key");

    let details = omdb.get_details(IMDB_ID).await.expect("OMDb details");
    assert!(!details.genres.is_empty(), "OMDb returned no genres");
    // The normalisation contract: prose in, ISO codes out.
    assert_eq!(details.original_language.as_deref(), Some("ja"), "language normalisation drifted");
    assert_eq!(details.origin_countries, vec!["JP"], "country normalisation drifted");
    // The scores as text: `8.1`, `393,000`, `86`, `94%`. Each scale read, IMDb with its votes.
    let imdb = details.ratings.get("imdb").expect("no IMDb score read");
    assert!(imdb.votes.is_some_and(|votes| votes > 100_000), "IMDb votes not read: {imdb:?}");
    for scale in ["metacritic", "rotten_tomatoes"] {
        assert!(
            details.ratings.contains_key(scale),
            "no {scale} score read: {:?}",
            details.ratings
        );
    }
    println!(
        "OMDb {IMDB_ID}: {:?} / {:?} / {:?}",
        details.genres, details.certification, details.ratings
    );
}

#[tokio::test]
#[ignore = "hits the real TheTVDB API; needs TVDB_API_KEY"]
async fn thetvdb_answers_the_shape_the_client_expects() {
    let Ok(key) = std::env::var("TVDB_API_KEY") else {
        eprintln!("skipped: TVDB_API_KEY is not set");
        return;
    };
    let pin = std::env::var("TVDB_SUBSCRIBER_PIN").ok();

    let tvdb = TvdbClient::new(
        client(),
        &key,
        pin.as_deref(),
        crate::integrations::tvdb::DEFAULT_BASE_URL,
        std::sync::Arc::new(tokio::sync::Mutex::new(None)),
    );

    // The login flow is the part no other source has, the one most worth proving for real.
    assert!(tvdb.test_connection().await.expect("TheTVDB login"), "TheTVDB rejected the key");

    let details = tvdb.get_details(TVDB_SERIES).await.expect("TheTVDB details");
    assert!(!details.genres.is_empty(), "TheTVDB returned no genres");
    assert_eq!(details.original_language.as_deref(), Some("ja"), "639-3 mapping drifted");
    assert_eq!(details.origin_countries, vec!["JP"], "alpha-3 mapping drifted");
    println!("TheTVDB {TVDB_SERIES}: {:?} / {:?}", details.genres, details.certifications);
}

/// Rate limiting is the one behaviour the offline suite cannot observe. A burst
/// at the configured concurrency must either succeed throughout or fail with
/// honest 429s, never with panics or malformed responses.
#[tokio::test]
#[ignore = "hits the real Jikan API repeatedly"]
async fn jikan_survives_a_burst_at_the_configured_concurrency() {
    use futures::stream::{self, StreamExt};

    let jikan = JikanClient::new(client(), crate::integrations::jikan::DEFAULT_BASE_URL);

    // Ids MyAnimeList holds: its numbering has gaps, and an id it does not
    // hold answers 404, which is not what this burst is about.
    let held = [1i64, 5, 6, 7, 8, 15, 16, 17, 18, 19, 20, 21];
    let outcomes: Vec<_> = stream::iter(held)
        .map(|id| {
            let jikan = jikan.clone();
            async move { (id, jikan.get_details(id).await) }
        })
        // The cap the source declares for itself.
        .buffer_unordered(2)
        .collect()
        .await;

    let mut throttled = 0;
    for (id, outcome) in &outcomes {
        match outcome {
            Ok(_) => {}
            Err(crate::error::AppError::ExternalApi { status: 429, .. }) => throttled += 1,
            // MAL being down is not a Routarr defect, so say so and stop.
            Err(e) if upstream_is_down(e) => {
                eprintln!("skipped: Jikan or MyAnimeList is unavailable ({e})");
                return;
            }
            Err(e) => panic!("Jikan {id} failed in an unexpected way: {e}"),
        }
    }

    println!("Jikan burst: {} of {} throttled", throttled, outcomes.len());
    // Not asserted: how much Jikan throttles is its business, as long as it arrives as a typed
    // 429 the enrichment pass backs off from.
}
