//! Live validation against the **real** metadata APIs.
//!
//! The rest of the suite is offline, which keeps it fast and hermetic but
//! shares one blind spot: a fake written to match the client drifts with it,
//! agreeing forever while the real API moves. Captured payloads narrow that;
//! only a real request closes it.
//!
//! Ignored by default and opt-in:
//!
//! ```bash
//! cargo test --test-threads=1 live_sources -- --ignored --nocapture   # keyless only
//! OMDB_API_KEY=… TVDB_API_KEY=… cargo test live_sources -- --ignored  # everything
//! ```
//!
//! A source with no credential skips itself rather than failing, so the keyless
//! pair is runnable by anyone. Excluded from CI: a green build must not depend
//! on a third party's uptime.

use crate::config::Config;
use crate::integrations::anilist::AniListClient;
use crate::integrations::jikan::JikanClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tvdb::TvdbClient;

/// A real HTTP client with the project's own timeouts and redirect policy, so
/// what is exercised is the shipped configuration and not a bare reqwest.
fn client() -> reqwest::Client {
    crate::http::build_client(&Config::for_tests()).expect("test http client")
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

    // The resolution contract: at least one candidate must carry a title that
    // matches once normalised, and a year that agrees. If AniList ever stops
    // returning `startDate.year`, resolution silently stops working — and this
    // is the only test that would notice.
    let matched = candidates.iter().find(|c| {
        c.titles.iter().any(|t| {
            crate::services::metadata::normalise_title(t)
                == crate::services::metadata::normalise_title(TITLE)
        })
    });
    let matched = matched.expect("no AniList candidate matched on title");
    assert_eq!(matched.year, Some(YEAR), "AniList year drifted or stopped being returned");

    let details = anilist.get_details(matched.id).await.expect("AniList details");
    assert!(!details.genres.is_empty(), "AniList returned no genres");
    assert_eq!(details.origin_countries, vec!["JP"], "countryOfOrigin drifted");
    // Tags are the reason to have AniList at all.
    assert!(!details.keywords.is_empty(), "no tag cleared the agreement floor");
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
                crate::services::metadata::normalise_title(t)
                    == crate::services::metadata::normalise_title(TITLE)
            })
        })
        .expect("no Jikan candidate matched on title");

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
    println!("Jikan {}: {:?} / {:?}", matched.id, details.genres, details.keywords);
}

#[tokio::test]
#[ignore = "hits the real OMDb API; needs OMDB_API_KEY"]
async fn omdb_answers_the_shape_the_client_expects() {
    let Ok(key) = std::env::var("OMDB_API_KEY") else {
        eprintln!("skipped: OMDB_API_KEY is not set");
        return;
    };

    let omdb = OmdbClient::new(client(), &key, crate::integrations::omdb::DEFAULT_BASE_URL);

    assert!(omdb.test_connection().await.expect("OMDb probe"), "OMDb rejected the key");

    let details = omdb.get_details(IMDB_ID).await.expect("OMDb details");
    assert!(!details.genres.is_empty(), "OMDb returned no genres");
    // The normalisation contract: prose in, ISO codes out.
    assert_eq!(details.original_language.as_deref(), Some("ja"), "language normalisation drifted");
    assert_eq!(details.origin_countries, vec!["JP"], "country normalisation drifted");
    println!("OMDb {IMDB_ID}: {:?} / {:?}", details.genres, details.certification);
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
        &["US".to_string()],
        std::sync::Arc::new(tokio::sync::Mutex::new(None)),
    );

    // The login flow is the part no other source has, so it is the part most
    // worth proving against the real service.
    assert!(tvdb.test_connection().await.expect("TheTVDB login"), "TheTVDB rejected the key");

    let details = tvdb.get_details(TVDB_SERIES, "series").await.expect("TheTVDB details");
    assert!(!details.genres.is_empty(), "TheTVDB returned no genres");
    assert_eq!(details.original_language.as_deref(), Some("ja"), "639-3 mapping drifted");
    assert_eq!(details.origin_countries, vec!["JP"], "alpha-3 mapping drifted");
    println!("TheTVDB {TVDB_SERIES}: {:?} / {:?}", details.genres, details.certification);
}

/// Rate limiting is the one behaviour the offline suite cannot observe. A burst
/// at the configured concurrency must either succeed throughout or fail with
/// honest 429s, never with panics or malformed responses.
#[tokio::test]
#[ignore = "hits the real Jikan API repeatedly"]
async fn jikan_survives_a_burst_at_the_configured_concurrency() {
    use futures::stream::{self, StreamExt};

    let jikan = JikanClient::new(client(), crate::integrations::jikan::DEFAULT_BASE_URL);

    let outcomes: Vec<_> = stream::iter(1..=12i64)
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
            // MAL being down is not a Routarr defect; say so and stop.
            Err(e) if upstream_is_down(e) => {
                eprintln!("skipped: Jikan or MyAnimeList is unavailable ({e})");
                return;
            }
            Err(e) => panic!("Jikan {id} failed in an unexpected way: {e}"),
        }
    }

    println!("Jikan burst: {} of {} throttled", throttled, outcomes.len());
    // Documented, not asserted: how much Jikan throttles is its business. What
    // matters is that throttling arrives as a typed 429 the enrichment pass
    // already knows how to back off from.
}
