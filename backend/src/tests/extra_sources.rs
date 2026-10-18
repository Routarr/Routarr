//! AniList, Jikan, OMDb and TheTVDB, each through its own client. AniList and Jikan know none of
//! Routarr's identifiers and search for an item, the one place a source can attach the wrong
//! work: most of what follows defends that seam.

use crate::services::{enrichment, metadata};
use crate::state::AppState;

use super::fake_sources::FakeSources;
use super::{TestApp, probe_verdicts, warning_messages};

/// The library's titles made series, the only titles TheTVDB answers for.
const SERIES_ALONE: &str = "UPDATE media SET media_type = 'series'";

/// The ids a source was asked to describe, in order.
fn asked(sources: &FakeSources, source: &str) -> Vec<String> {
    let recorded = sources.recorded();
    recorded
        .details
        .iter()
        .filter(|(asked, _)| *asked == source)
        .map(|(_, id)| id.clone())
        .collect()
}

/// What one source cached for the single film.
async fn cached(app: &TestApp, source: &str) -> Option<(String, String, Option<String>, String)> {
    sqlx::query_as(
        "SELECT genres, keywords, certification, origin_countries FROM metadata_cache
         WHERE source = ?",
    )
    .bind(source)
    .fetch_optional(&app.state.pool)
    .await
    .unwrap()
}

// -------------------------------------------------------- direct addressing

#[tokio::test]
async fn omdb_is_addressed_by_the_imdb_id_and_normalises_its_prose() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "omdb").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(asked(&sources, "omdb"), ["tt0096283"]);
    let (genres, _, certification, countries) = cached(&app, "omdb").await.expect("cached");
    assert_eq!(genres, r#"["Animation","Family","Fantasy"]"#);
    assert_eq!(certification.as_deref(), Some("G"));
    // "Japan" in, `JP` out: a rule is written against the code.
    assert_eq!(countries, r#"["JP"]"#);

    // And the key really travelled, rather than the client meaning to send it.
    let recorded = sources.recorded();
    assert!(
        recorded.credentials.iter().any(|(source, key)| *source == "omdb" && key == "omdb-key")
    );
}

#[tokio::test]
async fn omdb_language_names_become_the_code_a_rule_matches() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "omdb").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let language: Option<String> =
        sqlx::query_scalar("SELECT original_language FROM metadata_cache WHERE source = 'omdb'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    // "Japanese, English": the first is the original one, as `ja`.
    assert_eq!(language.as_deref(), Some("ja"));
}

/// The record is asked in its short form: the characters, artworks and
/// trailers of a series weigh more than the rest of it, and nothing reads them.
#[tokio::test]
async fn the_tvdb_record_is_asked_without_what_nothing_reads() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb").await;
    app.execute(&[SERIES_ALONE]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let recorded = sources.recorded();
    let records: Vec<&String> =
        recorded.paths.iter().filter(|path| path.contains("/extended")).collect();
    assert!(!records.is_empty(), "the control: no record was read");
    assert!(records.iter().all(|path| path.ends_with("/extended?short=true")), "{records:?}");
}

#[tokio::test]
async fn the_tvdb_token_survives_between_passes_and_pages() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb").await;

    // Two passes and a probe each rebuild the client: the token lives on the state.
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let _ = app.get("/api/v1/health").await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let recorded = sources.recorded();
    let logins = recorded.paths.iter().filter(|path| *path == "/tvdb/login").count();
    assert_eq!(logins, 1, "the token was re-fetched instead of reused");
    // The PIN travels alongside the key: a user-supported key needs both.
    assert!(
        recorded
            .credentials
            .iter()
            .any(|(source, value)| *source == "tvdb" && value == "tvdb-key/1234")
    );
}

/// TheTVDB documents a month's validity. A token kept past it answers 401 on every read with
/// nothing naming the cause, and the reads in flight when it expires log in once between them.
#[tokio::test]
async fn an_expired_tvdb_token_is_renewed_once_and_the_read_retried() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb").await;
    app.execute(&[SERIES_ALONE]).await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    sources.expire_tvdb_token();
    // More titles than reads in flight, or the next pass has nothing to say.
    let more = app.state.config.metadata_concurrency as i64 + 2;
    for n in 0..more {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tvdb_id)
             VALUES (?, 'inst-1', ?, 'series', ?, 1998, ?)",
        )
        .bind(format!("m-{n}-more"))
        .bind(20 + n)
        .bind(format!("Series {n}"))
        .bind(77000 + n)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let logins = sources.recorded().paths.iter().filter(|path| *path == "/tvdb/login").count();
    assert_eq!(logins, 2, "the expired token was renewed more than once");
    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache WHERE source = 'tvdb'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(cached, 1 + more, "the reads behind the expired token were not retried");
}

/// TheTVDB answers for series alone: a film carrying a TheTVDB id is neither
/// fetched nor read from its cache.
#[tokio::test]
async fn thetvdb_is_never_asked_about_a_film() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    assert!(asked(&sources, "tvdb").is_empty(), "{:?}", asked(&sources, "tvdb"));

    app.execute(&["INSERT INTO metadata_cache (source, external_id, media_type, genres)
                   VALUES ('tvdb', '76885', 'movie', '[\"Western\"]')"])
        .await;
    let explained = app.get("/api/v1/media/m-1/explain").await;
    assert!(explained.assert_ok()["metadata"].is_null(), "a film read TheTVDB's answer");
}

#[tokio::test]
async fn thetvdb_three_letter_codes_become_the_ones_rules_are_written_against() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb").await;
    app.execute(&[SERIES_ALONE]).await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let row: (Option<String>, String, String) = sqlx::query_as(
        "SELECT original_language, origin_countries, certifications FROM metadata_cache
         WHERE source = 'tvdb'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    assert_eq!(asked(&sources, "tvdb"), ["series/76885"]);
    assert_eq!(row.0.as_deref(), Some("ja"), "jpn -> ja");
    assert_eq!(row.1, r#"["JP"]"#, "jpn -> JP");
    assert_eq!(row.2, r#"{"FR":"-12","US":"TV-14"}"#, "fra -> FR, usa -> US");
}

// ------------------------------------------------------------- resolution

#[tokio::test]
async fn anilist_finds_a_film_by_title_and_remembers_the_answer() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let resolved: (String, Option<String>) = sqlx::query_as(
        "SELECT local_key, external_id FROM source_identifiers WHERE source = 'anilist'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    // Keyed on the library's identity, so the film in a second Radarr is not searched again.
    assert_eq!(resolved.0, "tmdb:8392");
    assert_eq!(resolved.1.as_deref(), Some("523"));
    assert_eq!(asked(&sources, "anilist"), ["523"], "another work was described");

    let (genres, keywords, _, countries) = cached(&app, "anilist").await.expect("cached");
    assert_eq!(genres, r#"["Adventure","Slice of Life"]"#);
    assert_eq!(countries, r#"["JP"]"#);
    // Community tags make the keywords, minus the ones nobody agrees with.
    assert_eq!(keywords, r#"["Iyashikei","Rural"]"#);
}

/// A film is searched among films and a series among everything else, on
/// both sources that search: restricted to films, a TV anime is never found,
/// and unrestricted, the film is searched among series as well.
#[tokio::test]
async fn a_film_is_searched_among_films_and_a_series_among_the_rest() {
    for (source, films) in [("anilist", "MOVIE"), ("jikan", "movie")] {
        let sources = FakeSources::start().await;
        let app = TestApp::one_film_on(&sources, source).await;
        app.execute(&["INSERT INTO media (id, instance_id, arr_id, media_type, title, year,
                                          tvdb_id)
               VALUES ('m-2', 'inst-1', 20, 'series', 'Cowboy Bebop', 1998, 76885)"])
            .await;

        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();

        let resolved: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT local_key, external_id FROM source_identifiers WHERE source = ?
              ORDER BY local_key",
        )
        .bind(source)
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(
            resolved,
            [("tmdb:8392".into(), Some("523".into())), ("tvdb:76885".into(), Some("1".into()))],
            "{source}"
        );
        let mut formats: Vec<Option<String>> = sources
            .recorded()
            .formats
            .iter()
            .filter(|(asked, _)| *asked == source)
            .map(|(_, format)| format.clone())
            .collect();
        formats.sort();
        assert_eq!(formats, [None, Some(films.to_string())], "{source}");
    }
}

#[tokio::test]
async fn a_second_pass_does_not_search_again() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 1, "a resolution is permanent, not per-run");
}

/// AniList and MyAnimeList list anime alone: a title its Arr files under no
/// animated genre is searched there neither by a pass nor by a lookup, until
/// the setting asks for every title.
#[tokio::test]
async fn a_title_the_arr_calls_live_action_is_not_searched() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    app.execute(&[
        "UPDATE media SET genres = '[\"ANIMATION\",\"Family\"]'",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, imdb_id, genres)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Heat', 1995, 'tt0113277', '[\"Drama\"]')",
    ])
    .await;
    let searched = || -> Vec<String> {
        let recorded = sources.recorded();
        recorded
            .searches
            .iter()
            .filter(|(id, _)| *id == "anilist")
            .map(|(_, t)| t.clone())
            .collect()
    };

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    assert_eq!(searched(), ["My Neighbor Totoro"]);
    app.get("/api/v1/route?type=movie&imdb=tt0113277&enrich=true").await.assert_ok();
    assert_eq!(searched(), ["My Neighbor Totoro"], "a lookup searched a live-action title");

    app.store_setting("anime_search", "all").await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    assert_eq!(searched(), ["My Neighbor Totoro", "Heat"]);
}

/// A search the source refuses for this title alone, a 400 or a 422, is a
/// search that found nothing: remembered, not asked again at every pass.
#[tokio::test]
async fn a_search_the_source_refuses_is_not_repeated_next_pass() {
    let sources = FakeSources::failing(400).await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    for _ in 0..2 {
        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();
    }

    let searched = sources.recorded().paths.iter().filter(|path| *path == "/anilist").count();
    assert_eq!(searched, 1, "a refused search was asked again");
}

#[tokio::test]
async fn a_work_from_the_wrong_year_is_refused_and_the_refusal_is_remembered() {
    let sources = FakeSources::with_mismatched_year().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    // Same title, sixteen years apart: not the same work.
    let resolved: (Option<String>,) =
        sqlx::query_as("SELECT external_id FROM source_identifiers WHERE source = 'anilist'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(resolved.0, None);
    assert!(cached(&app, "anilist").await.is_none());

    // The miss is written down, so the next pass does not search again.
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 1);
}

/// A pass whose searches all come back empty has nothing left to fetch, and
/// its task still ends full: a finished task showing "1/2" reads as one that
/// stopped halfway.
#[tokio::test]
async fn a_pass_that_identifies_nothing_still_ends_its_progress() {
    let sources = FakeSources::with_mismatched_year().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Castle in the Sky', 1986, 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let progress: (String, i64, i64) = sqlx::query_as(
        "SELECT status, progress_current, progress_total FROM jobs WHERE kind = 'enrich'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(progress, ("success".to_string(), 2, 2));
}

/// Jikan leaves `year` null for a film and dates it by `aired`. Read from
/// `year`, every film whose library knows its year would be refused, and the
/// refusal remembered for good.
#[tokio::test]
async fn a_jikan_film_is_resolved_by_its_aired_year() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "jikan").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let resolved: Option<String> =
        sqlx::query_scalar("SELECT external_id FROM source_identifiers WHERE source = 'jikan'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(resolved.as_deref(), Some("523"));
}

/// The webhook enriches a new title before its rules run, from every source the row can
/// address: TheTVDB by its id, AniList by a search. TMDB alone would leave a series with no TMDB
/// id routed on nothing.
#[tokio::test]
async fn a_webhook_enrichment_asks_every_source_the_row_can_address() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "tvdb,anilist,arr").await;
    sqlx::query("UPDATE media SET media_type = 'series', tmdb_id = NULL, imdb_id = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let media = crate::api::media::load_media(&app.state, "m-1").await.unwrap();

    enrichment::enrich_one(&app.state, &media).await.unwrap();

    assert_eq!(asked(&sources, "tvdb"), [format!("series/{}", super::fake_sources::TVDB_ID)]);
    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 1);
    let resolved: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_identifiers WHERE source = 'anilist'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(resolved, 1, "the AniList search was not recorded");
    assert!(cached(&app, "tvdb").await.is_some(), "TheTVDB's answer was not stored");
}

/// An upgrade forgets the ids an Arr wrote as 0, and the one answer every
/// title without an id shared under the key `tmdb:0`: kept, it would go on
/// routing them all until their next sync.
#[tokio::test]
async fn an_upgrade_forgets_the_ids_written_as_zero_and_what_they_shared() {
    let pool = super::database_of_0_1_5().await;
    sqlx::query(super::AN_INSTANCE).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, tvdb_id)
         VALUES ('m-1', 'inst-1', 1, 'series', 'A', 0, 76885),
                ('m-2', 'inst-1', 2, 'series', 'B', 1396, 81189)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
         VALUES ('anilist', 'series', 'tmdb:0', '1'), ('anilist', 'series', 'tmdb:1396', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let ids: Vec<(String, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT id, tmdb_id, tvdb_id FROM media ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids, [("m-1".into(), None, Some(76885)), ("m-2".into(), Some(1396), Some(81189))]);
    let keys: Vec<String> = sqlx::query_scalar("SELECT local_key FROM source_identifiers")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(keys, ["tmdb:1396"]);
}
/// A work is often listed after the library holds it, so a search that found nothing is asked
/// again once it is old.
#[tokio::test]
async fn a_search_that_found_nothing_is_tried_again_once_it_is_old() {
    let sources = FakeSources::with_mismatched_year().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    sqlx::query("UPDATE source_identifiers SET resolved_at = datetime('now', '-41 days')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 2, "an old miss was never asked again");
}

/// GraphQL reports a failure as `data: null` beside an `errors` list, with a
/// 200. Read as an empty answer, the failure would be remembered as "nothing
/// found".
#[tokio::test]
async fn an_anilist_error_is_not_remembered_as_nothing_found() {
    let sources = FakeSources::with_graphql_errors().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let remembered: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_identifiers WHERE source = 'anilist'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(remembered, 0, "a failure was stored as an answer");
}

#[tokio::test]
async fn jikan_themes_and_demographics_become_keywords() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "jikan").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(asked(&sources, "jikan"), ["523"], "another work was described");
    let paths = sources.recorded().paths.clone();
    assert!(paths.contains(&"/jikan/anime/523/full".to_string()), "{paths:?}");
    let (genres, keywords, certification, _) = cached(&app, "jikan").await.expect("cached");
    assert_eq!(genres, r#"["Adventure"]"#);
    assert_eq!(keywords, r#"["iyashikei","kids"]"#);
    // "G - All Ages" keeps the code a rule names.
    assert_eq!(certification.as_deref(), Some("G"));
}

// ------------------------------------------------------------- all together

#[tokio::test]
async fn every_source_contributes_what_only_it_has() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,anilist,jikan,omdb,tvdb").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let media = super::TestApp::get(&app, "/api/v1/media/m-1").await;
    let media = media.assert_ok();
    let field_sources = &media["metadata"]["field_sources"];

    // The Arr row is empty here, so AniList, next in the order, wins the genres.
    assert_eq!(field_sources["genres"], "anilist");
    assert_eq!(field_sources["keywords"], "anilist");
    assert_eq!(field_sources["origin_countries"], "anilist");
    // Jikan rates first but on MyAnimeList's scale, and OMDb for the US, the default region.
    assert_eq!(field_sources["certification"], "omdb");
    // Neither anime source reports a language, and OMDb is the first that does.
    assert_eq!(field_sources["original_language"], "omdb");
}

/// Radarr and Sonarr report English for any original language they do not list (Cantonese,
/// Malay, Swahili), so their English is the one answer a later source may correct. Any other
/// language they report stands, and their English stands where no source knows better.
#[tokio::test]
async fn the_arrs_english_gives_way_to_a_source_that_knows_the_language() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,omdb").await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    for (order, arr_says, read, from) in [
        ("arr,omdb", "en", "ja", "omdb"),
        ("arr,omdb", "ko", "ko", "arr"),
        ("arr", "en", "en", "arr"),
    ] {
        app.store_setting("metadata_providers", order).await;
        // Genres too, so the Arr always answers and is listed once, its English offered last.
        sqlx::query("UPDATE media SET original_language = ?, genres = '[\"Animation\"]'")
            .bind(arr_says)
            .execute(&app.state.pool)
            .await
            .unwrap();

        let media = app.get("/api/v1/media/m-1").await;
        let metadata = &media.assert_ok()["metadata"];

        assert_eq!(metadata["original_language"], read, "{order}, the Arr saying {arr_says}");
        assert_eq!(metadata["field_sources"]["original_language"], from, "{order}, {arr_says}");
        let sources = metadata["sources"].as_array().unwrap();
        let named: Vec<&str> = sources.iter().filter_map(|s| s.as_str()).collect();
        assert_eq!(named.iter().filter(|s| **s == "arr").count(), 1, "{named:?}");
    }
}

#[tokio::test]
async fn the_health_page_probes_every_enabled_source() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,anilist,omdb,tvdb").await;

    let response = app.get("/api/v1/health").await;
    let health = response.assert_ok();
    let providers = health["metadata"]["providers"].as_array().unwrap();

    assert_eq!(providers.len(), 4);
    // The Arr is reached through the instance probes, not here.
    assert_eq!(providers[0]["connected"], serde_json::Value::Null);
    assert_eq!(providers[1]["id"], "anilist");
    assert_eq!(providers[1]["needs_key"], false);
    assert_eq!(providers[1]["connected"], true);
    assert_eq!(providers[2]["id"], "omdb");
    assert_eq!(providers[2]["connected"], true);
    assert_eq!(providers[3]["id"], "tvdb");
    assert_eq!(providers[3]["connected"], true);
}

/// Two callers at once share one probe, and a caller soon after reads it
/// again: each source is asked once, at the pace every other request to it
/// keeps. "Recheck" asks again.
#[tokio::test]
async fn one_probe_answers_every_caller_until_a_recheck() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,anilist").await;
    let asked = || sources.recorded().paths.iter().filter(|path| *path == "/anilist").count();

    let (first, second) = tokio::join!(app.get("/api/v1/health"), app.get("/api/v1/health"));
    first.assert_ok();
    second.assert_ok();
    app.get("/api/v1/health").await.assert_ok();
    assert_eq!(asked(), 1, "{:?}", sources.recorded().paths);

    app.get("/api/v1/health?fresh=true").await.assert_ok();
    assert_eq!(asked(), 2, "a recheck read the last probe again");
}

/// A source that does not answer says why, so a refused key is told from a
/// source that is down, and the log keeps what it answered.
#[tokio::test]
async fn a_failed_source_probe_says_why_and_is_logged() {
    use tracing_subscriber::layer::SubscriberExt;
    let log = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(log.clone()));
    let _logging = tracing::subscriber::set_default(subscriber);
    let sources = FakeSources::failing(500).await;
    let app = TestApp::one_film_on(&sources, "arr,anilist").await;

    let health = app.get("/api/v1/health").await;
    let anilist = health.assert_ok()["metadata"]["providers"][1].clone();

    assert_eq!(
        (anilist["connected"].as_bool(), anilist["reason"].as_str()),
        (Some(false), Some("unreachable"))
    );
    assert!(log.contents().contains("The probe of a metadata source failed"), "{}", log.contents());
}

/// Whether the health page reads TheTVDB as connected.
async fn tvdb_connected(app: &TestApp) -> serde_json::Value {
    let health = app.get("/api/v1/health?fresh=true").await.assert_ok().clone();
    let providers = health["metadata"]["providers"].as_array().unwrap().clone();
    providers.into_iter().find(|p| p["id"] == "tvdb").expect("tvdb is listed")["connected"].clone()
}

/// A revoked key is what the probe is for. Answered from the token the last
/// login left, it would read as connected until that token expires, a month on.
#[tokio::test]
async fn a_revoked_tvdb_key_is_reported_by_the_next_probe() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,tvdb").await;
    assert_eq!(tvdb_connected(&app).await, true, "the fixture's key does not work to begin with");

    sources.revoke_tvdb_key();

    assert_eq!(tvdb_connected(&app).await, false, "the revoked key still reads as connected");
}

/// A token belongs to the key that obtained it. Kept across a new key, it would
/// go on answering for the old one until it expires.
#[tokio::test]
async fn a_new_tvdb_key_logs_in_rather_than_reuse_the_old_token() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,tvdb").await;
    tvdb_connected(&app).await;

    app.save_setting("tvdb_api_key", "new-key").await.assert_ok();
    tvdb_connected(&app).await;

    assert_eq!(tvdb_logins(&sources), ["tvdb-key/1234", "new-key/1234"]);
}

/// Each login TheTVDB's stand-in received, as `key/pin`.
fn tvdb_logins(sources: &FakeSources) -> Vec<String> {
    let recorded = sources.recorded();
    recorded
        .credentials
        .iter()
        .filter(|(id, _)| *id == "tvdb")
        .map(|(_, credential)| credential.clone())
        .collect()
}

/// A PIN saved on the Sources screen is the one the login carries, over
/// `TVDB_SUBSCRIBER_PIN`: otherwise saving one looks done and changes nothing.
#[tokio::test]
async fn a_saved_tvdb_pin_reaches_the_login_over_the_environment() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,tvdb").await;

    app.save_setting("tvdb_subscriber_pin", "5678").await.assert_ok();
    assert_eq!(tvdb_connected(&app).await, true);

    assert_eq!(tvdb_logins(&sources), ["tvdb-key/5678"]);
}

/// With no PIN saved, or the saved one removed, `TVDB_SUBSCRIBER_PIN` answers:
/// a deployment that sets it keeps working untouched.
#[tokio::test]
async fn the_environment_tvdb_pin_answers_while_none_is_saved() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,tvdb").await;

    app.save_setting("tvdb_subscriber_pin", "5678").await.assert_ok();
    app.save_setting("tvdb_subscriber_pin", "").await.assert_ok();
    assert_eq!(tvdb_connected(&app).await, true);

    assert_eq!(tvdb_logins(&sources), ["tvdb-key/1234"]);
}

/// A token belongs to the PIN it was obtained with as much as to the key.
/// Kept across a new PIN, it would go on answering for the old one until it
/// expires.
#[tokio::test]
async fn a_new_tvdb_pin_logs_in_rather_than_reuse_the_old_token() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,tvdb").await;
    tvdb_connected(&app).await;

    app.save_setting("tvdb_subscriber_pin", "5678").await.assert_ok();
    tvdb_connected(&app).await;

    assert_eq!(tvdb_logins(&sources), ["tvdb-key/1234", "tvdb-key/5678"]);
}

#[tokio::test]
async fn a_source_without_its_key_is_reported_rather_than_probed() {
    let sources = FakeSources::start().await;
    let app = TestApp::new().await;

    let mut config = crate::config::Config::for_tests();
    config.omdb_base_url = sources.omdb_url();
    // No OMDb key.
    let app = TestApp::around(app.state.clone().with_config(config));

    app.store_setting("metadata_providers", "omdb").await;

    let response = app.get("/api/v1/health").await;
    let health = response.assert_ok();

    assert_eq!(health["metadata"]["providers"][0]["configured"], false);
    assert_eq!(health["metadata"]["providers"][0]["connected"], serde_json::Value::Null);
    assert!(warning_messages(health).iter().any(|w| w.contains("OMDb")));
    // A setting to look at, and no fault: a monitor reading the status stays quiet.
    assert_eq!(health["status"], "ok");
}

// ------------------------------------------------------------------- units

#[test]
fn the_local_key_prefers_the_most_stable_identifier_it_has() {
    assert_eq!(
        metadata::local_key_of(Some(8392), Some(1), Some("tt1"), "T", Some(1988)),
        "tmdb:8392"
    );
    assert_eq!(
        metadata::local_key_of(None, Some(76885), Some("tt1"), "T", Some(1988)),
        "tvdb:76885"
    );
    assert_eq!(metadata::local_key_of(None, None, Some("tt1"), "T", Some(1988)), "imdb:tt1");
    // Nothing but a title: still a key, and still shared between instances.
    assert_eq!(metadata::local_key_of(None, None, None, "Akira", Some(1988)), "title:akira|1988");
}

// ------------------------------------------------------- source outages

/// A library big enough that a doomed request per item would be obvious.
async fn library_of(sources: &FakeSources, order: &str, count: i64) -> TestApp {
    let app = TestApp::one_film_on(sources, order).await;

    for arr_id in 100..(100 + count) {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
             tvdb_id, imdb_id, monitored, has_files)
             VALUES (?, 'inst-1', ?, 'movie', ?, 1988, ?, ?, ?, 1, 1)",
        )
        .bind(format!("m-{arr_id}"))
        .bind(arr_id)
        .bind(format!("Film {arr_id}"))
        .bind(1000 + arr_id)
        .bind(2000 + arr_id)
        .bind(format!("tt{:07}", 3000 + arr_id))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    app
}

/// A work a source does not have is an answer like any other, cached so the next pass does not
/// ask again and a daily quota is kept. TheTVDB and OMDb are asked by the library's id, AniList
/// and Jikan by one an earlier search resolved.
#[tokio::test]
async fn a_work_a_source_does_not_have_is_not_asked_again_next_pass() {
    for source in ["tvdb", "omdb", "anilist", "jikan"] {
        let sources = FakeSources::start().await;
        let app = TestApp::one_film_on(&sources, source).await;
        if source == "tvdb" {
            app.execute(&[SERIES_ALONE]).await;
        }
        app.execute(&[
            "UPDATE media SET tvdb_id = 999, imdb_id = 'tt0999999'",
            "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
             VALUES ('anilist', 'movie', 'tmdb:8392', '999'), ('jikan', 'movie', 'tmdb:8392', '999')",
        ])
        .await;

        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();
        enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
            .await
            .unwrap();

        assert_eq!(asked(&sources, source).len(), 1, "{source}: {:?}", asked(&sources, source));
    }
}

/// A revoked TheTVDB key is refused at every login, and the pass stops at the breaker rather
/// than spending a login on every title. A login answering without a token is a refusal too:
/// no read goes out with an empty bearer.
#[tokio::test]
async fn a_tvdb_key_refused_at_login_costs_a_handful_of_attempts() {
    let revoked = FakeSources::start().await;
    revoked.revoke_tvdb_key();
    let app = library_of(&revoked, "tvdb", 20).await;
    app.execute(&[SERIES_ALONE]).await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let logins = revoked.recorded().paths.iter().filter(|path| *path == "/tvdb/login").count();
    assert!(logins <= 8, "{logins} refused logins for 21 titles");

    let tokenless = FakeSources::start().await;
    tokenless.answer_logins_without_a_token();
    let app = library_of(&tokenless, "tvdb", 3).await;
    app.execute(&[SERIES_ALONE]).await;
    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();
    let recorded = tokenless.recorded();
    assert!(recorded.paths.contains(&"/tvdb/login".to_string()), "the control: no login");
    let reads: Vec<&String> = recorded.paths.iter().filter(|p| p.ends_with("/extended")).collect();
    assert!(reads.is_empty(), "a read went out without a token: {reads:?}");
}

/// A `Retry-After` received while searching holds the searches after it
/// back, as one received while fetching holds the fetches: the search after
/// a 429 asking for a second of quiet leaves at least a second after it.
#[tokio::test]
async fn a_retry_after_received_while_searching_holds_the_next_search() {
    let sources = FakeSources::start().await;
    let app = library_of(&sources, "anilist", 1).await;
    // One search at a time, so the second leaves after the first is answered.
    let config = crate::config::Config { metadata_concurrency: 1, ..(*app.state.config).clone() };
    let app = TestApp::around(app.state.clone().with_config(config));
    sources.throttle_next_search(1);

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let arrivals = sources.recorded().searched_at.clone();
    assert!(arrivals.len() >= 2, "{} searches", arrivals.len());
    let gap = arrivals[1] - arrivals[0];
    assert!(gap >= std::time::Duration::from_secs(1), "the next search left after {gap:?}");
}

/// Jikan answers `504` to every request while MyAnimeList is down. A source refusing five times
/// in a row is not asked again that pass, not even for the titles a search identified before,
/// but the requests in flight at the fifth refusal still go out.
#[tokio::test]
async fn a_source_that_is_down_is_abandoned_rather_than_asked_once_per_item() {
    for source in ["jikan", "anilist", "omdb", "tvdb"] {
        let sources = FakeSources::failing(504).await;
        let app = library_of(&sources, source, 20).await;
        if source == "tvdb" {
            app.execute(&[SERIES_ALONE]).await;
        }
        // Half the films identified earlier, so a searching source has records to fetch.
        app.execute(&[
            "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
             SELECT searching.source, 'movie', 'tmdb:' || tmdb_id, tmdb_id
               FROM media, (SELECT 'anilist' AS source UNION ALL SELECT 'jikan') searching
              WHERE arr_id BETWEEN 100 AND 109",
        ])
        .await;
        let fetching = app.state.metadata_sources().await;
        let ceiling = 5 + fetching[0].concurrency(app.state.config.metadata_concurrency) - 1;

        let report =
            enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
                .await
                .unwrap();

        // The point is how it failed: a handful of requests, not one per item.
        assert_eq!(report.enriched, 0, "{source}");
        assert!(report.considered > 0, "{source} had no record to fetch: {report:?}");
        let attempts = sources.recorded().paths.len();
        assert!(
            attempts <= ceiling,
            "{attempts} requests went out to {source}, which was down (21 items in the library), \
             {ceiling} at most"
        );
    }
}

/// The breaker must not fire on an ordinary miss: a source answering "I do not
/// know that item" says nothing about the next one.
#[tokio::test]
async fn a_working_source_is_never_cut_off_by_the_breaker() {
    let sources = FakeSources::start().await;
    let app = library_of(&sources, "jikan", 20).await;

    let report = enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    assert_eq!(report.skipped, 0, "a healthy source was cut off: {report:?}");
    assert!(report.enriched > 0);
}

// ------------------------------------------------------------ pacing

/// AniList states its limit beside each answer, and the pace every request
/// to it waits on follows that limit rather than the one assumed.
#[tokio::test]
async fn the_pace_follows_the_limit_anilist_states() {
    use crate::services::rate_limit::RateLimiter;
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    let source = app.state.metadata_sources().await.into_iter().next().expect("AniList");
    let pace = RateLimiter::new(90, 1);

    let found = source.resolve("My Neighbor Totoro", Some(1988), "movie").await;
    source.paced_after(&pace, &found).await;

    tokio::time::pause();
    let start = tokio::time::Instant::now();
    pace.acquire().await;
    pace.acquire().await;
    let per_request = 60 / u64::from(super::fake_sources::ANILIST_LIMIT);
    assert_eq!(start.elapsed().as_secs(), per_request, "not paced at the limit AniList stated");
}

#[tokio::test]
async fn the_public_endpoints_are_paced_and_a_mirror_is_not() {
    let sources = FakeSources::start().await;

    // The fake is what a mirror or a proxy looks like, and no published limit applies to it.
    let app = TestApp::one_film_on(&sources, "anilist,jikan,omdb").await;
    for source in app.state.metadata_sources().await {
        assert!(
            source.rate().is_none(),
            "{} was paced against a base URL the operator changed",
            source.id()
        );
    }

    // Pointed at the real APIs, the published ceilings apply.
    let mut config = crate::config::Config::for_tests();
    config.anilist_base_url = crate::integrations::anilist::DEFAULT_BASE_URL.into();
    config.jikan_base_url = crate::integrations::jikan::DEFAULT_BASE_URL.into();
    config.omdb_base_url = crate::integrations::omdb::DEFAULT_BASE_URL.into();
    config.omdb_api_key = Some("omdb-key".into());
    let app = TestApp::around(AppState::for_tests().await.with_config(config));
    app.store_setting("metadata_providers", "anilist,jikan,omdb").await;

    let sources = app.state.metadata_sources().await;
    let rates: Vec<(&str, Option<(u32, u32)>)> =
        sources.iter().map(|source| (source.id(), source.rate())).collect();
    assert_eq!(
        rates,
        vec![("anilist", Some((30, 2))), ("jikan", Some((60, 3))), ("omdb", Some((300, 10)))]
    );

    // A concurrency setting is a ceiling: Jikan keeps two in flight whatever it says.
    let concurrency: Vec<(&str, usize)> =
        sources.iter().map(|source| (source.id(), source.concurrency(8))).collect();
    assert_eq!(concurrency, vec![("anilist", 8), ("jikan", 2), ("omdb", 8)]);
}

/// A verdict must not outlive its subject. A source switched off is not probed, and nothing
/// expires `probe_results`, so the probe prunes what it no longer looks at, or the last failure
/// is reported on every `/status` poll for ever.
#[tokio::test]
async fn a_source_switched_off_stops_being_reported_as_unreachable() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,omdb").await;
    // A port nothing listens on, which is how a source that stopped answering behaves.
    let mut config = (*app.state.config).clone();
    config.omdb_base_url = "http://127.0.0.1:1".to_string();
    let app = TestApp::around(app.state.clone().with_config(config));

    let unreachable =
        app.state.localizer().await.translate("WarnProviderUnreachable", &[("provider", "OMDb")]);
    app.get("/api/v1/health").await.assert_ok();
    let warned = warning_messages(app.get("/api/v1/status").await.assert_ok());
    assert!(
        warned.contains(&unreachable),
        "the probe recorded nothing about a source that failed: {warned:?}"
    );

    // Switched off, it is no longer probed, and no longer has anything to say.
    sqlx::query("UPDATE settings SET value = 'arr' WHERE key = 'metadata_providers'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    app.get("/api/v1/health").await.assert_ok();
    let after = warning_messages(app.get("/api/v1/status").await.assert_ok());
    assert!(
        !after.contains(&unreachable),
        "a source nobody probes any more is still reported: {after:?}"
    );
}

/// A probe names what it found: a refused key, a source asking for fewer requests and one that
/// does not answer each call for something else of the operator. A source taking no key that
/// refuses is one not answering, with no key to check.
#[tokio::test]
async fn a_probe_names_why_a_source_did_not_answer() {
    for (status, order, verdict) in [
        (429, "arr,jikan", "source_rate_limited"),
        (429, "arr,anilist", "source_rate_limited"),
        (401, "arr,tvdb", "source_key_refused"),
        (401, "arr,omdb", "source_key_refused"),
        (401, "arr,anilist", "source_unreachable"),
        (503, "arr,anilist", "source_unreachable"),
    ] {
        let sources = FakeSources::failing(status).await;
        let app = TestApp::one_film_on(&sources, order).await;
        assert_eq!(probe_verdicts(&app).await, [verdict], "{order} answering {status}");
    }
}

/// A probe keeps to its source's pace: sent while the source has asked for
/// quiet, it earns another refusal and reports the source as failing.
#[tokio::test]
async fn a_probe_waits_while_its_source_is_held_back() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,jikan").await;
    let jikan = app.state.metadata_sources().await.into_iter().find(|s| s.id() == "jikan");
    let pace = app.state.paces.of(&jikan.expect("jikan is a source"));
    pace.penalise(std::time::Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    assert!(probe_verdicts(&app).await.is_empty(), "the held source failed its probe");
    assert!(started.elapsed() >= std::time::Duration::from_millis(250), "the probe did not wait");
}

/// A pass counts again from zero at each source and each stage, so the task names the one its
/// count measures, and a stage starts on no count until it has one. Only the reading stage, held
/// before its first answer, has a count to check: a search writes its own at once.
#[tokio::test]
async fn a_pass_names_the_source_and_stage_its_count_measures() {
    for (order, id, stage, count) in [
        ("arr,jikan", "jikan", "JobDetailIdentifying", None),
        ("arr,jikan,omdb", "omdb", "JobDetailFetching", Some((0, 0))),
    ] {
        let sources = FakeSources::start().await;
        let app = TestApp::one_film_on(&sources, order).await;
        let source = app.state.metadata_sources().await.into_iter().find(|s| s.id() == id);
        let pace = app.state.paces.of(&source.expect("the source is configured"));
        pace.penalise(std::time::Duration::from_secs(1)).await;

        let state = app.state.clone();
        let pass = tokio::spawn(async move {
            enrichment::enrich_all_media(&state, &crate::jobs::Attribution::manual(None)).await
        });
        let name = metadata::info(id).unwrap().display_name;
        let (named, current, total) = loop {
            let running: Option<(String, i64, i64)> = sqlx::query_as(
                "SELECT detail_key, progress_current, progress_total FROM jobs
                  WHERE status = 'running' AND detail_params = ?",
            )
            .bind(serde_json::json!({ "source": name }).to_string())
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
            if let Some(named) = running {
                break named;
            }
            assert!(!pass.is_finished(), "{order}: the pass never named {name}");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(named, stage, "{order}");
        if let Some(count) = count {
            assert_eq!((current, total), count, "{order}: the count of the stage before");
        }
        pass.await.unwrap().unwrap();
    }
}

/// A searched source's answer ages as any other: past its lifetime it is read
/// again by the id found, and the title is not searched again.
#[tokio::test]
async fn a_searched_answer_past_its_lifetime_is_read_again() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,jikan").await;
    let by = crate::jobs::Attribution::manual(None);
    let pass = || enrichment::enrich_all_media(&app.state, &by);
    pass().await.unwrap();
    app.execute(&["UPDATE metadata_cache SET cached_at = datetime('now', '-8 days')"]).await;

    pass().await.unwrap();

    let recorded = sources.recorded();
    let read = recorded.details.iter().filter(|(source, _)| *source == "jikan").count();
    let searched = recorded.searches.iter().filter(|(source, _)| *source == "jikan").count();
    assert_eq!((read, searched), (2, 1));
}

/// A finished pass counts the whole pass, not its last stage: TheTVDB, last
/// here, has nothing to read about a film, and the task would end on no count
/// at all after OMDb read one.
#[tokio::test]
async fn a_finished_pass_counts_the_whole_pass() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,omdb,tvdb").await;

    enrichment::enrich_all_media(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let progress: (i64, i64) =
        sqlx::query_as("SELECT progress_current, progress_total FROM jobs WHERE kind = 'enrich'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(progress, (1, 1));
}

/// An upgrade keeps due an answer whose expiry had passed: the expiry goes,
/// and with it the only mark that it was. A TMDB answer is due too, for the score it gives.
#[tokio::test]
async fn an_upgrade_keeps_due_the_answers_whose_time_had_come() {
    let pool = super::database_of_0_1_5().await;
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, expires_at)
         VALUES ('jikan', '1', 'series', datetime('now', '-1 day')),
                ('tmdb', '3', 'movie', '2099-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let due: Vec<(String, bool)> =
        sqlx::query_as("SELECT source, stale FROM metadata_cache ORDER BY source")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(due, [("jikan".to_string(), true), ("tmdb".to_string(), true)]);
}

/// An upgrade has OMDb and TheTVDB asked again, for names and codes read into every ISO language
/// and country, and TMDB for the score it gives.
#[tokio::test]
async fn an_upgrade_asks_omdb_and_thetvdb_again_for_the_full_vocabularies() {
    let pool = super::database_of_0_1_5().await;
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, expires_at)
         VALUES ('omdb', 'tt1', 'movie', '2099-01-01 00:00:00'),
                ('tvdb', '2', 'series', '2099-01-01 00:00:00'),
                ('tmdb', '3', 'movie', '2099-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let due: Vec<String> =
        sqlx::query_scalar("SELECT source FROM metadata_cache WHERE stale ORDER BY source")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(due, ["omdb", "tmdb", "tvdb"]);
}

/// An upgrade reads each cached status into the words the Arrs use, as a new
/// answer is read, and drops the posters nothing reads.
#[tokio::test]
async fn an_upgrade_reads_cached_statuses_as_a_new_answer_would() {
    let pool = super::database_of_0_1_5().await;
    let statuses = [
        "Released",
        "Ended",
        "FINISHED",
        "Finished Airing",
        "RELEASING",
        "Currently Airing",
        "Returning Series",
        "Continuing",
        "HIATUS",
        "Planned",
        "In Production",
        "Post Production",
        "Pilot",
        "NOT_YET_RELEASED",
        "Not yet aired",
        "Upcoming",
        "Rumored",
        "Canceled",
        "CANCELLED",
        "Something else",
    ];
    // From 1: the same upgrade drops the answers cached under an id of 0.
    for (index, raw) in statuses.iter().enumerate() {
        for kind in ["movie", "series"] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type, status, poster_path,
                                             expires_at)
                 VALUES ('tmdb', ?, ?, ?, '/p.jpg', '2099-01-01 00:00:00')",
            )
            .bind((index + 1).to_string())
            .bind(kind)
            .bind(raw)
            .execute(&pool)
            .await
            .unwrap();
        }
    }

    crate::db::run_migrations(&pool).await.unwrap();

    for (index, raw) in statuses.iter().enumerate() {
        for kind in ["movie", "series"] {
            let stored: Option<String> = sqlx::query_scalar(
                "SELECT status FROM metadata_cache WHERE external_id = ? AND media_type = ?",
            )
            .bind((index + 1).to_string())
            .bind(kind)
            .fetch_one(&pool)
            .await
            .unwrap();
            let read = crate::integrations::status::read(raw, kind);
            assert_eq!(stored.as_deref(), read, "{raw} for a {kind}");
        }
    }
    let posters: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('metadata_cache') WHERE name = 'poster_path'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(posters, 0);
}
