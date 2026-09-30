//! AniList, Jikan, OMDb and TheTVDB, each through its own client.
//!
//! Two of them, AniList and Jikan, know none of Routarr's identifiers, so
//! they have to *find* an item before they can describe it. That search is the
//! only place in the application where a source can attach the wrong work to a
//! media item, and a wrong genre routes a film into the wrong folder. Most of
//! what follows defends that seam.

use crate::services::{enrichment, metadata};
use crate::state::AppState;

use super::fake_sources::FakeSources;
use super::{TestApp, warning_messages};

/// A one-film library pointed at every fake source.
///
/// `title`/`year` are what the resolution has to work with, and `imdb`/`tvdb`
/// are what the directly-addressed sources read.
async fn library(sources: &FakeSources, order: &str) -> TestApp {
    let app = TestApp::new().await;

    let mut config = crate::config::Config::for_tests();
    config.anilist_base_url = sources.anilist_url();
    config.jikan_base_url = sources.jikan_url();
    config.omdb_base_url = sources.omdb_url();
    config.omdb_api_key = Some("omdb-key".into());
    config.tvdb_base_url = sources.tvdb_url();
    config.tvdb_api_key = Some("tvdb-key".into());
    config.tvdb_pin = Some("1234".into());

    let app = TestApp::around(app.state.clone().with_config(config));

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Arr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id, tvdb_id,
         imdb_id, monitored, has_files)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'My Neighbor Totoro', 1988, 8392, 76885,
                 'tt0096283', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    app.store_setting("metadata_providers", order).await;

    app
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
    let app = library(&sources, "omdb").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

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
    let app = library(&sources, "omdb").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let language: Option<String> =
        sqlx::query_scalar("SELECT original_language FROM metadata_cache WHERE source = 'omdb'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    // "Japanese, English": the first is the original one, as `ja`.
    assert_eq!(language.as_deref(), Some("ja"));
}

#[tokio::test]
async fn the_tvdb_token_survives_between_passes_and_pages() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "tvdb").await;

    // Two enrichment passes and a health probe: three separate occasions on
    // which the client is rebuilt. The token lives on the state, not the
    // client, so exactly one login is spent on all three, and TheTVDB counts
    // them.
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    let _ = app.get("/api/v1/health").await;
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

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

/// TheTVDB documents a month's validity. A token kept past it answers 401 on
/// every read, one per pending title per pass, `failed` climbing, and nothing
/// naming the cause.
#[tokio::test]
async fn an_expired_tvdb_token_is_renewed_once_and_the_read_retried() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "tvdb").await;
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    sources.expire_tvdb_token();
    // A second title to read, or the next pass has nothing to say.
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tvdb_id, monitored,
         has_files)
         VALUES ('m-2', 'inst-1', 11, 'series', 'Cowboy Bebop', 1998, 76885, 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let logins = sources.recorded().paths.iter().filter(|path| *path == "/tvdb/login").count();
    assert_eq!(logins, 2, "the expired token was not renewed");
    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache WHERE source = 'tvdb'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(cached, 2, "the read behind the expired token was not retried");
}

#[tokio::test]
async fn thetvdb_three_letter_codes_become_the_ones_rules_are_written_against() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "tvdb").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let row: (Option<String>, String, Option<String>) = sqlx::query_as(
        "SELECT original_language, origin_countries, certification FROM metadata_cache
         WHERE source = 'tvdb'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    assert_eq!(row.0.as_deref(), Some("ja"), "jpn -> ja");
    assert_eq!(row.1, r#"["JP"]"#, "jpn -> JP");
    // Two ratings offered: the configured region order decides, and the default
    // is US.
    assert_eq!(row.2.as_deref(), Some("TV-14"));
}

// ------------------------------------------------------------- resolution

#[tokio::test]
async fn anilist_finds_a_film_by_title_and_remembers_the_answer() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let resolved: (String, Option<String>) = sqlx::query_as(
        "SELECT local_key, external_id FROM source_identifiers WHERE source = 'anilist'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    // Keyed on the identity the library already has, so the same film in a
    // second Radarr is not searched for twice.
    assert_eq!(resolved.0, "tmdb:8392");
    assert_eq!(resolved.1.as_deref(), Some("523"));

    let (genres, keywords, _, countries) = cached(&app, "anilist").await.expect("cached");
    assert_eq!(genres, r#"["Adventure","Slice of Life"]"#);
    assert_eq!(countries, r#"["JP"]"#);
    // Community tags make the keywords, minus the ones nobody agrees with.
    assert_eq!(keywords, r#"["Iyashikei","Rural"]"#);
}

#[tokio::test]
async fn a_second_pass_does_not_search_again() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 1, "a resolution is permanent, not per-run");
}

#[tokio::test]
async fn a_work_from_the_wrong_year_is_refused_and_the_refusal_is_remembered() {
    let sources = FakeSources::with_mismatched_year().await;
    let app = library(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    // Same title, sixteen years apart: not the same work. Better no metadata
    // than another film's genres.
    let resolved: (Option<String>,) =
        sqlx::query_as("SELECT external_id FROM source_identifiers WHERE source = 'anilist'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(resolved.0, None);
    assert!(cached(&app, "anilist").await.is_none());

    // And the "found nothing" is written down, so the next pass does not search
    // the whole library again for ever.
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();
    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 1);
}

/// Jikan leaves `year` null for a film and dates it by `aired`. Read from
/// `year`, every film whose library knows its year would be refused, and the
/// refusal remembered for good.
#[tokio::test]
async fn a_jikan_film_is_resolved_by_its_aired_year() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "jikan").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let resolved: Option<String> =
        sqlx::query_scalar("SELECT external_id FROM source_identifiers WHERE source = 'jikan'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(resolved.as_deref(), Some("523"));
}

/// An upgrade forgets the films Jikan "found nothing" for, and nothing else. A
/// database through migration 004 holds those answers as read from `year`,
/// which Jikan leaves null for a film, so none of them is Jikan's.
#[tokio::test]
async fn an_upgrade_searches_jikan_again_for_the_films_it_misread() {
    let pool = super::database_through("004_orphaned_proposals").await;
    for (source, media_type, key, external) in [
        ("jikan", "movie", "tmdb:8392", None),
        ("jikan", "movie", "tmdb:149", Some("47")),
        ("jikan", "series", "tvdb:76885", None),
        ("anilist", "movie", "tmdb:8392", None),
    ] {
        sqlx::query(
            "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
             VALUES (?, ?, ?, ?)",
        )
        .bind(source)
        .bind(media_type)
        .bind(key)
        .bind(external)
        .execute(&pool)
        .await
        .unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    let left: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT source, media_type, local_key FROM source_identifiers
         ORDER BY source, media_type, local_key",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let left: Vec<(&str, &str, &str)> =
        left.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
    assert_eq!(
        left,
        [
            ("anilist", "movie", "tmdb:8392"),
            ("jikan", "movie", "tmdb:149"),
            ("jikan", "series", "tvdb:76885"),
        ]
    );
}

/// A work is often listed after the library holds it: a film indexed before
/// its release is not on AniList yet. A search that found nothing is asked
/// again once it is old, not kept for good.
#[tokio::test]
async fn a_search_that_found_nothing_is_tried_again_once_it_is_old() {
    let sources = FakeSources::with_mismatched_year().await;
    let app = library(&sources, "anilist").await;
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    sqlx::query("UPDATE source_identifiers SET resolved_at = datetime('now', '-31 days')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let searches = sources.recorded().searches.iter().filter(|(id, _)| *id == "anilist").count();
    assert_eq!(searches, 2, "an old miss was never asked again");
}

/// GraphQL reports a failure as `data: null` beside an `errors` list, with a
/// 200. Read as an empty answer, the failure would be remembered as "nothing
/// found".
#[tokio::test]
async fn an_anilist_error_is_not_remembered_as_nothing_found() {
    let sources = FakeSources::with_graphql_errors().await;
    let app = library(&sources, "anilist").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

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
    let app = library(&sources, "jikan").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let (genres, keywords, certification, _) = cached(&app, "jikan").await.expect("cached");
    assert_eq!(genres, r#"["Adventure"]"#);
    // The vocabulary an anime library is actually sorted by, and which TMDb
    // does not express at all.
    assert_eq!(keywords, r#"["iyashikei","kids"]"#);
    // "G - All Ages" keeps only what a rule can name.
    assert_eq!(certification.as_deref(), Some("G - All Ages"));
}

// ------------------------------------------------------------- all together

#[tokio::test]
async fn every_source_contributes_what_only_it_has() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "arr,anilist,jikan,omdb,tvdb").await;

    enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    let media = super::TestApp::get(&app, "/api/v1/media/m-1").await;
    let media = media.assert_ok();
    let field_sources = &media["metadata"]["field_sources"];

    // The Arr row is empty here, so AniList, the next in the order, wins the
    // genres, and each later source only fills what is still missing.
    assert_eq!(field_sources["genres"], "anilist");
    assert_eq!(field_sources["keywords"], "anilist");
    assert_eq!(field_sources["origin_countries"], "anilist");
    // AniList has no certification at all, and Jikan is the first that does.
    assert_eq!(field_sources["certification"], "jikan");
    // Neither anime source reports a language, and OMDb is the first that does.
    assert_eq!(field_sources["original_language"], "omdb");
}

#[tokio::test]
async fn the_health_page_probes_every_enabled_source() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "arr,anilist,omdb,tvdb").await;

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

/// Whether the health page reads TheTVDB as connected.
async fn tvdb_connected(app: &TestApp) -> serde_json::Value {
    let health = app.get("/api/v1/health").await.assert_ok().clone();
    let providers = health["metadata"]["providers"].as_array().unwrap().clone();
    providers.into_iter().find(|p| p["id"] == "tvdb").expect("tvdb is listed")["connected"].clone()
}

/// A revoked key is what the probe is for. Answered from the token the last
/// login left, it would read as connected until that token expires, a month on.
#[tokio::test]
async fn a_revoked_tvdb_key_is_reported_by_the_next_probe() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "arr,tvdb").await;
    assert_eq!(tvdb_connected(&app).await, true, "the fixture's key does not work to begin with");

    sources.revoke_tvdb_key();

    assert_eq!(tvdb_connected(&app).await, false, "the revoked key still reads as connected");
}

/// A token belongs to the key that obtained it. Kept across a new key, it would
/// go on answering for the old one until it expires.
#[tokio::test]
async fn a_new_tvdb_key_logs_in_rather_than_reuse_the_old_token() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "arr,tvdb").await;
    tvdb_connected(&app).await;

    app.put("/api/v1/settings", serde_json::json!({ "settings": { "tvdb_api_key": "new-key" } }))
        .await
        .assert_ok();
    tvdb_connected(&app).await;

    let logins: Vec<String> = sources
        .recorded()
        .credentials
        .iter()
        .filter(|(id, _)| *id == "tvdb")
        .map(|(_, credential)| credential.clone())
        .collect();
    assert_eq!(logins, ["tvdb-key/1234", "new-key/1234"]);
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
}

// ------------------------------------------------------------------- units

#[test]
fn a_title_matches_whatever_punctuation_and_case_it_is_written_in() {
    assert_eq!(
        crate::services::rule_engine::normalise_value("My Neighbor Totoro!"),
        "my neighbor totoro"
    );
    assert_eq!(crate::services::rule_engine::normalise_value("  ONE  PIECE  "), "one piece");
    assert_eq!(
        crate::services::rule_engine::normalise_value("Fullmetal Alchemist: Brotherhood"),
        "fullmetal alchemist brotherhood"
    );
}

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
    let app = library(sources, order).await;

    for arr_id in 100..(100 + count) {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
             monitored, has_files)
             VALUES (?, 'inst-1', ?, 'movie', ?, 1988, ?, 1, 1)",
        )
        .bind(format!("m-{arr_id}"))
        .bind(arr_id)
        .bind(format!("Film {arr_id}"))
        .bind(1000 + arr_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    app
}

/// Jikan answers `504` for *every* request whenever MyAnimeList is down.
/// Without a breaker, a five-thousand-title library issues five thousand doomed
/// requests, logs five thousand warnings, and repeats the whole thing on the
/// next pass. A source that has refused five times in one pass is down, so
/// Routarr stops asking.
#[tokio::test]
async fn a_source_that_is_down_is_abandoned_rather_than_asked_once_per_item() {
    let sources = FakeSources::failing(504).await;
    let app = library_of(&sources, "jikan", 20).await;

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    // Nothing was enriched, which is expected, but the point is *how* it
    // failed: the requests that went out are a handful, not one per item. For a
    // searching source the storm happens in the resolution stage, so that is
    // where the count is taken.
    assert_eq!(report.enriched, 0);
    let attempts = sources.recorded().paths.len();
    assert!(
        attempts <= 12,
        "{attempts} requests went out to a source that was down (20 items in the library)"
    );
}

/// The breaker must not fire on an ordinary miss: a source answering "I do not
/// know that item" says nothing about the next one.
#[tokio::test]
async fn a_working_source_is_never_cut_off_by_the_breaker() {
    let sources = FakeSources::start().await;
    let app = library_of(&sources, "jikan", 20).await;

    let report = enrichment::enrich_all_media(&app.state, "manual").await.unwrap();

    assert_eq!(report.skipped, 0, "a healthy source was cut off: {report:?}");
    assert!(report.enriched > 0);
}

// ------------------------------------------------------------ pacing

#[tokio::test]
async fn the_public_endpoints_are_paced_and_a_mirror_is_not() {
    let sources = FakeSources::start().await;

    // Pointed at the fake, which is what a mirror or a proxy looks like, no
    // published limit applies, and pacing it would buy a delay for nothing.
    // This is also why the test suite does not spend a second per request.
    let app = library(&sources, "anilist,jikan,omdb").await;
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
    let app = TestApp::around(AppState::for_tests().await.with_config(config));
    app.store_setting("metadata_providers", "anilist,jikan").await;

    let rates: Vec<(&str, Option<(u32, u32)>)> = app
        .state
        .metadata_sources()
        .await
        .iter()
        .map(|source| (source.id(), source.rate()))
        .collect();

    assert_eq!(rates, vec![("anilist", Some((90, 5))), ("jikan", Some((60, 3)))]);
}

/// A verdict must not outlive its subject.
///
/// `record_probe` writes nothing for a source it did not probe, and one
/// switched off is not probed at all. Nothing expires the table either, so the
/// probe prunes what it no longer looks at, or the source's last failure is
/// reported for ever, on every `/status` poll, with no screen able to clear it.
#[tokio::test]
async fn a_source_switched_off_stops_being_reported_as_unreachable() {
    let sources = FakeSources::start().await;
    let app = library(&sources, "arr,omdb").await;
    // A port nothing listens on, which is how a source that has stopped
    // answering behaves.
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
