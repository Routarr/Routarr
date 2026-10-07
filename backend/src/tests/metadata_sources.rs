//! Several metadata sources, ordered by priority.
//!
//! The two properties worth defending: a library with **no TMDB key at all**
//! still routes on genre, language and certification, because Radarr and Sonarr
//! carry those in the payload the sync already reads. And when two sources
//! disagree, the order the user set decides, field by field, with the loser
//! still filling in what the winner had nothing to say about.

use crate::services::maintenance;
use sqlx::AssertSqlSafe;

use super::fake_arr::FakeArr;
use super::{AN_INSTANCE, TestApp, database_through, warning_messages};

async fn set_order(app: &TestApp, order: &str) {
    app.store_setting("metadata_providers", order).await;
}

/// Cache a TMDB answer for the fake Radarr's movie, deliberately different from
/// what the Arr says, so which one wins is observable.
async fn cache_tmdb(app: &TestApp, genres: &str) {
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, expires_at)
         VALUES ('tmdb', '8392', 'movie', ?, '[\"anime\"]', 'en', '[\"US\"]', '2099-01-01')",
    )
    .bind(genres)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

// ------------------------------------------------------- the Arr as a source

#[tokio::test]
async fn sync_stores_the_metadata_radarr_already_carries() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    let row: (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT genres, original_language, certification FROM media")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    assert_eq!(row.0.as_deref(), Some(r#"["Animation","Family"]"#));
    // Radarr answers "Japanese", and a rule is written against `ja`.
    assert_eq!(row.1.as_deref(), Some("ja"));
    assert_eq!(row.2.as_deref(), Some("G"));
}

#[tokio::test]
async fn a_genre_rule_matches_with_no_tmdb_key_configured() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    // `AppState::for_tests` configures no TMDB key, and nothing was enriched.
    assert!(app.state.config.tmdb_api_key.is_none());
    assert_eq!(app.decided_category().await, "anime");
}

#[tokio::test]
async fn an_original_language_rule_matches_the_code_not_the_arrs_wording() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "original_language", "value": ["ja"] })).await;

    assert_eq!(app.decided_category().await, "anime");
}

#[tokio::test]
async fn a_keyword_rule_cannot_match_from_the_arr_alone() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    set_order(&app, "arr").await;
    app.seed_rule_on(serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;

    // The one thing no Arr reports. Silence, not a wrong match.
    assert_eq!(app.decided_category().await, "standard");
}

// ------------------------------------------------------------ priority order

#[tokio::test]
async fn the_first_source_in_the_order_wins_the_field() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(app.decided_category().await, "anime");
}

#[tokio::test]
async fn reordering_the_sources_changes_the_decision() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "tmdb,arr").await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    // Same library, same rule, same data: only the order moved.
    assert_eq!(app.decided_category().await, "standard");
}

#[tokio::test]
async fn a_lower_source_still_fills_what_the_higher_one_lacks() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    // Radarr wins the genres above. Keywords exist only in TMDB's answer and
    // must still be reachable.
    app.seed_rule_on(serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;

    assert_eq!(app.decided_category().await, "anime");
}

/// The Arr's own metadata is always read. A list saved without it, by an older
/// version or by hand in the database, must not leave a library whose genre
/// rules silently stop matching, and the API refuses to save one.
#[tokio::test]
async fn the_arr_source_cannot_be_turned_off() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    set_order(&app, "").await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(app.decided_category().await, "anime");

    for without_arr in ["tmdb", ""] {
        let refused = app
            .put(
                "/api/v1/settings",
                serde_json::json!({ "settings": { "metadata_providers": without_arr } }),
            )
            .await;
        refused.assert_status(axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn a_source_from_a_newer_build_is_ignored_rather_than_fatal() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    // A database written by a build that knew a source this one does not.
    set_order(&app, "trakt,arr").await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(app.decided_category().await, "anime");
}

// ------------------------------------------------------------ explainability

#[tokio::test]
async fn the_explanation_names_the_source_that_answered() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    app.seed_rule_on(serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    let response = app.get("/api/v1/media/m-inst-1-10/explain").await;
    let explanation = response.assert_ok();
    let condition = &explanation["rule_traces"][0]["conditions"][0];

    assert_eq!(condition["matched"], true);
    // Two sources hold genres and they disagree. Without this the explanation
    // cannot be checked against either of them.
    assert_eq!(condition["source"], "arr");
}

#[tokio::test]
async fn the_media_page_lists_the_sources_that_contributed() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;

    let response = app.get("/api/v1/media/m-inst-1-10").await;
    let media = response.assert_ok();
    let sources = media["metadata"]["sources"].as_array().unwrap();

    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0], "arr");
    assert_eq!(media["metadata"]["field_sources"]["genres"], "arr");
    assert_eq!(media["metadata"]["field_sources"]["keywords"], "tmdb");
}

// ---------------------------------------------------------------- catalogue

#[tokio::test]
async fn the_provider_catalogue_reports_what_each_source_needs() {
    let app = TestApp::new().await;
    let raw = app.get("/api/v1/metadata/providers").await;
    let response = raw.assert_ok();

    let arr = &response["providers"][0];
    assert_eq!(arr["id"], "arr");
    assert_eq!(arr["needs_key"], false);
    // Usable with no configuration at all: the whole point of it.
    assert_eq!(arr["configured"], true);

    let tmdb = &response["providers"][1];
    assert_eq!(tmdb["id"], "tmdb");
    assert_eq!(tmdb["needs_key"], true);
    assert_eq!(tmdb["configured"], false);
    // Named, so the interface can say what to set rather than "a key is
    // missing", which nobody can act on.
    assert_eq!(tmdb["key_env"], "TMDB_API_KEY");
    assert_eq!(tmdb["media_types"], serde_json::json!(["movie", "series"]));

    // Radarr carries no TheTVDB id, so TheTVDB answers for series alone.
    let providers = response["providers"].as_array().unwrap();
    let tvdb = providers.iter().find(|provider| provider["id"] == "tvdb").unwrap();
    assert_eq!(tvdb["media_types"], serde_json::json!(["series"]));

    assert_eq!(response["order"][0], "arr");
}

#[tokio::test]
async fn a_condition_no_enabled_source_can_answer_is_reported_as_unavailable() {
    let app = TestApp::new().await;
    set_order(&app, "arr").await;

    let response = app.get("/api/v1/rules/conditions").await;
    let catalog = response.assert_ok();
    let conditions = catalog["conditions"].as_array().unwrap();
    let find = |kind: &str| {
        conditions.iter().find(|c| c["type"] == kind).expect("condition in catalogue").clone()
    };

    assert_eq!(find("genre_contains")["available"], true);
    assert_eq!(find("keyword_contains")["available"], false);
    assert_eq!(find("origin_country")["available"], false);
    assert_eq!(find("title_contains")["available"], true);
}

/// One cached answer, in the namespace the source addresses by.
async fn cache_row(app: &TestApp, source: &str, external_id: &str, genres: &str) {
    sqlx::query(
        "INSERT INTO metadata_cache
            (source, external_id, media_type, genres, keywords, original_language,
             origin_countries, certification, status, overview, poster_path,
             cached_at, expires_at)
         VALUES (?, ?, 'movie', ?, '[]', NULL, '[]', NULL, NULL, NULL, NULL,
                 datetime('now'), datetime('now', '+7 days'))",
    )
    .bind(source)
    .bind(external_id)
    .bind(genres)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

/// What the library column says about the first film.
async fn listed_has_metadata(app: &TestApp) -> bool {
    app.get("/api/v1/media").await.assert_ok()["data"][0]["has_metadata"] == true
}

/// A source switched off stops speaking for an item.
///
/// The engine reads the enabled order. A predicate filtering on nothing would
/// let a row cached by a source the operator disabled yesterday make the column
/// promise metadata, while the engine treats the same item as undescribed.
#[tokio::test]
async fn a_disabled_source_no_longer_answers_for_an_item() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    sqlx::query("UPDATE media SET genres = '[]', original_language = NULL, certification = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let tmdb_id: i64 = sqlx::query_scalar("SELECT tmdb_id FROM media ORDER BY id LIMIT 1")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    cache_row(&app, "tmdb", &tmdb_id.to_string(), r#"["Animation"]"#).await;

    set_order(&app, "arr,tmdb").await;
    assert!(listed_has_metadata(&app).await, "the cached answer should count while tmdb is on");

    // Another fetched source in its place, so the cache clause is still built:
    // reduced to `arr` alone it is not emitted at all, and the check would pass
    // whether or not the filter exists.
    set_order(&app, "arr,anilist").await;
    assert!(!listed_has_metadata(&app).await, "a source switched off still spoke for the item");
}

/// A series TheTVDB describes counts, though it carries no TMDB id.
///
/// A predicate looking only in the `tmdb_id` namespace would read a series
/// enriched by a source that addresses by `tvdb_id` as undescribed for ever, on
/// the one screen somebody opens to find out why a rule matches nothing.
#[tokio::test]
async fn a_series_known_only_to_thetvdb_is_not_undescribed() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("sonarr", &arr).await;
    sqlx::query(
        "UPDATE media SET genres = '[]', original_language = NULL, certification = NULL,
                          tmdb_id = NULL, tvdb_id = 4242",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO metadata_cache
            (source, external_id, media_type, genres, keywords, original_language,
             origin_countries, certification, status, overview, poster_path,
             cached_at, expires_at)
         VALUES ('tvdb', '4242', 'series', '[\"Animation\"]', '[]', NULL, '[]', NULL,
                 NULL, NULL, NULL, datetime('now'), datetime('now', '+7 days'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    set_order(&app, "arr,tvdb").await;

    assert!(
        listed_has_metadata(&app).await,
        "a TheTVDB answer was invisible for want of a TMDB id"
    );
}

/// A title AniList describes counts as the engine counts it. AniList is
/// found by search, and its answer is cached under its own id, which only
/// what the search resolved the title to leads to.
#[tokio::test]
async fn a_title_known_through_anilist_has_metadata_in_the_library() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.execute(&[
        "UPDATE media SET genres = '[]', original_language = NULL, certification = NULL",
    ])
    .await;
    let (id, tmdb_id): (String, i64) =
        sqlx::query_as("SELECT id, tmdb_id FROM media ORDER BY id LIMIT 1")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
         VALUES ('anilist', 'movie', ?, '21')",
    )
    .bind(format!("tmdb:{tmdb_id}"))
    .execute(&app.state.pool)
    .await
    .unwrap();
    cache_row(&app, "anilist", "21", r#"["Animation"]"#).await;
    set_order(&app, "arr,anilist").await;

    let explained = app.get(&format!("/api/v1/media/{id}/explain")).await.assert_ok().clone();
    assert!(!explained["metadata"].is_null(), "the engine reads no answer: {explained}");
    assert!(listed_has_metadata(&app).await, "the library disagrees with the engine");
}

/// An id is an answer in its own namespace only: TheTVDB's series 1399 says
/// nothing about the series whose TMDB id is 1399.
#[tokio::test]
async fn an_id_from_another_namespace_is_not_metadata() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("sonarr", &arr).await;
    app.execute(&[
        "UPDATE media SET genres = '[]', original_language = NULL, certification = NULL,
                                    tmdb_id = 1399, tvdb_id = 81189",
    ])
    .await;
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
                                     origin_countries, cached_at, expires_at)
         VALUES ('tvdb', '1399', 'series', '[\"Animation\"]', '[]', '[]', datetime('now'),
                 datetime('now', '+7 days'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    set_order(&app, "arr,tvdb").await;

    assert!(!listed_has_metadata(&app).await, "another series' answer counted");
}

/// The column, the diagnostics count and the warning beside it are one question.
///
/// Three spellings of it is how a badge ends up contradicting the number above
/// it.
#[tokio::test]
async fn the_three_metadata_counters_agree_on_one_library() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    // The synced film described by the Arr, two more by nothing at all.
    app.execute(&["INSERT INTO media (id, instance_id, arr_id, media_type, title, genres)
                   VALUES ('m-bare-1', 'inst-1', 501, 'movie', 'Bare One', '[]'),
                          ('m-bare-2', 'inst-1', 502, 'movie', 'Bare Two', NULL)"])
        .await;
    set_order(&app, "arr").await;

    let listed = app.get("/api/v1/media?per_page=100").await.assert_ok().clone();
    let without =
        listed["data"].as_array().unwrap().iter().filter(|m| m["has_metadata"] == false).count()
            as i64;
    assert_eq!(without, 2, "the column: {listed}");

    let health = app.get("/api/v1/health?probe=false").await.assert_ok().clone();
    assert_eq!(
        health["metadata"]["media_missing_metadata"], without,
        "the diagnostics count and the library column disagree"
    );
    let warning = app.state.localizer().await.translate("WarnMissingMetadata", &[("count", "2")]);
    assert!(warning_messages(&health).contains(&warning), "{health}");
}

/// A cached row is not the same as a cached *answer*.
///
/// One holding only a synopsis is unreadable by every condition, so the engine
/// and the list must both say the item is unknown. Counting it would have the
/// column promise metadata about an item no rule can touch.
#[tokio::test]
async fn a_cached_synopsis_alone_is_not_metadata_to_either_of_them() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    // Nothing the Arr can contribute, so only the cache is left to answer.
    sqlx::query("UPDATE media SET genres = '[]', original_language = NULL, certification = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let tmdb_id: i64 = sqlx::query_scalar("SELECT tmdb_id FROM media ORDER BY id LIMIT 1")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO metadata_cache
            (source, external_id, media_type, genres, keywords, original_language,
             origin_countries, certification, status, overview, poster_path,
             cached_at, expires_at)
         VALUES ('tmdb', ?, 'movie', '[]', '[]', NULL, '[]', NULL, NULL,
                 'A synopsis, and nothing a rule can read.', NULL,
                 datetime('now'), datetime('now', '+7 days'))",
    )
    .bind(tmdb_id.to_string())
    .execute(&app.state.pool)
    .await
    .unwrap();
    set_order(&app, "arr,tmdb").await;

    let listed = app.get("/api/v1/media").await;
    let row = listed.assert_ok()["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tmdb_id"] == tmdb_id)
        .expect("the seeded film")
        .clone();
    assert_eq!(row["has_metadata"], false, "the list promised metadata no rule can read");

    // And the engine agrees, which is the whole invariant.
    let explained =
        app.get(&format!("/api/v1/media/{}/explain", row["id"].as_str().unwrap())).await;
    assert!(
        explained.assert_ok()["metadata"].is_null(),
        "the engine and the list disagree: {}",
        explained.json
    );
}

/// One described field is enough, whichever it is and whoever supplied it:
/// the engine reads every field `MetadataField` names, so the list, the
/// diagnostics and the rule builder's count must too, or each calls "missing"
/// an item a rule matches.
#[tokio::test]
async fn any_field_a_rule_reads_describes_the_item_to_every_counter() {
    let cases = [
        ("an Arr genre", "genres = '[\"Drama\"]'", None),
        ("an Arr language", "original_language = 'ja'", None),
        ("an Arr certification", "certification = 'PG'", None),
        ("cached keywords", "genres = '[]'", Some("keywords = '[\"kaiju\"]'")),
        ("a cached rating", "genres = '[]'", Some("certifications = '{\"US\":\"PG\"}'")),
    ];
    for (case, arr_field, cached) in cases {
        let app = one_undescribed_film().await;
        sqlx::query(AssertSqlSafe(format!("UPDATE media SET {arr_field}")))
            .execute(&app.state.pool)
            .await
            .unwrap();
        if let Some(cached) = cached {
            cache_for_the_film(&app, cached).await;
        }
        assert_eq!(described(&app).await, [true; 4], "{case}: engine, list, diagnostics, builder");
    }
}

/// A rating for a country outside every certification region is one the
/// engine never reads, and so describes nothing to any counter, until the
/// regions name the country.
#[tokio::test]
async fn a_cached_rating_outside_the_regions_describes_nothing_to_any_counter() {
    let app = one_undescribed_film().await;
    cache_for_the_film(&app, "certifications = '{\"DE\":\"16\"}'").await;
    app.store_setting("certification_regions", "FR,US").await;
    assert_eq!(described(&app).await, [false; 4], "engine, list, diagnostics, builder");

    app.store_setting("certification_regions", "FR,DE").await;
    assert_eq!(described(&app).await, [true; 4], "engine, list, diagnostics, builder");
}

/// A library of one film its Arr describes in nothing, read from TMDB after
/// the Arr.
async fn one_undescribed_film() -> TestApp {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.execute(&[
        "DELETE FROM media WHERE id != (SELECT id FROM media ORDER BY id LIMIT 1)",
        "UPDATE media SET genres = '[]', original_language = NULL, certification = NULL",
    ])
    .await;
    set_order(&app, "arr,tmdb").await;
    app
}

/// A TMDB answer for the film that holds `field` alone.
async fn cache_for_the_film(app: &TestApp, field: &str) {
    app.execute(&["INSERT INTO metadata_cache (source, external_id, media_type, expires_at)
                   SELECT 'tmdb', CAST(tmdb_id AS TEXT), 'movie', datetime('now', '+7 days')
                     FROM media"])
        .await;
    sqlx::query(AssertSqlSafe(format!("UPDATE metadata_cache SET {field}")))
        .execute(&app.state.pool)
        .await
        .unwrap();
}

/// Whether the film is described to the engine, the library column, the
/// diagnostics and the rule builder's count, in that order.
async fn described(app: &TestApp) -> [bool; 4] {
    let listed = app.get("/api/v1/media").await.assert_ok()["data"][0].clone();
    let explained =
        app.get(&format!("/api/v1/media/{}/explain", listed["id"].as_str().unwrap())).await;
    let health = app.get("/api/v1/health?probe=false").await.assert_ok().clone();
    let facets = app.get("/api/v1/media/facets").await.assert_ok().clone();
    [
        !explained.assert_ok()["metadata"].is_null(),
        listed["has_metadata"] == true,
        health["metadata"]["media_missing_metadata"] == 0,
        facets["without_metadata"] == 0,
    ]
}

// ------------------------------------------------------------ shipped order

async fn stored_order(app: &TestApp) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = 'metadata_providers'")
        .fetch_optional(&app.state.pool)
        .await
        .unwrap()
}

async fn warnings(app: &TestApp) -> Vec<String> {
    warning_messages(app.get("/api/v1/health").await.assert_ok())
}

async fn with_tmdb_key_in_the_environment() -> TestApp {
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    TestApp::around(crate::state::AppState::for_tests().await.with_config(config))
}

/// Listed without a key, TMDB answers nothing and the diagnostics say so, which
/// an installation nobody has configured yet reads as a fault of its own.
#[tokio::test]
async fn a_fresh_install_lists_the_arr_alone_and_raises_no_key_warning() {
    let app = TestApp::new().await;

    let catalogue = app.get("/api/v1/metadata/providers").await;
    assert_eq!(catalogue.assert_ok()["order"], serde_json::json!(["arr"]));
    let warnings = warnings(&app).await;
    assert!(!warnings.iter().any(|w| w.contains("TMDB")), "{warnings:?}");
}

/// The Compose file offers `TMDB_API_KEY` as the way to turn TMDB on, and a
/// start is what reads it.
#[tokio::test]
async fn a_start_lists_tmdb_when_its_key_is_in_the_environment() {
    let app = with_tmdb_key_in_the_environment().await;

    maintenance::converge(&app.state).await.unwrap();

    let catalogue = app.get("/api/v1/metadata/providers").await;
    assert_eq!(catalogue.assert_ok()["order"], serde_json::json!(["arr", "tmdb"]));
}

/// Any save stores the source list the screen holds, so a key set in the
/// environment afterwards finds a list the start leaves alone. Said, rather
/// than read and never used.
#[tokio::test]
async fn a_tmdb_key_that_arrives_after_a_save_is_reported() {
    let app = with_tmdb_key_in_the_environment().await;
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "metadata_providers": "arr" } }))
        .await
        .assert_ok();

    maintenance::converge(&app.state).await.unwrap();

    let expected = app.state.localizer().await.translate(
        "WarnProviderKeyUnlisted",
        &[("provider", "TMDB"), ("variable", "TMDB_API_KEY")],
    );
    let warnings = warnings(&app).await;
    assert!(warnings.contains(&expected), "{warnings:?}");
}

/// A key in the environment for a listed source is the ordinary case.
#[tokio::test]
async fn a_tmdb_key_for_a_listed_source_is_not_reported() {
    let app = with_tmdb_key_in_the_environment().await;
    app.list_tmdb().await;

    let warnings = warnings(&app).await;
    assert!(!warnings.iter().any(|w| w.contains("TMDB_API_KEY")), "{warnings:?}");
}

/// Taking TMDB out is a choice, and the environment must not undo it.
#[tokio::test]
async fn a_chosen_order_is_never_changed_by_the_environment() {
    let app = with_tmdb_key_in_the_environment().await;
    set_order(&app, "arr").await;

    assert!(!maintenance::converge_metadata_sources(&app.state).await.unwrap());
    assert_eq!(stored_order(&app).await.as_deref(), Some("arr"));
}

#[tokio::test]
async fn no_key_in_the_environment_leaves_the_order_unchosen() {
    let app = TestApp::new().await;

    assert!(!maintenance::converge_metadata_sources(&app.state).await.unwrap());
    assert_eq!(stored_order(&app).await, None);
}

/// The source list a first-release database holds once a start upgraded it,
/// after `seed` prepared it. The initial schema seeds `arr,tmdb`.
async fn upgraded(seed: &[&'static str]) -> Option<String> {
    let pool = database_through("001_initial_schema").await;
    for statement in seed {
        sqlx::query(*statement).execute(&pool).await.unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    sqlx::query_scalar("SELECT value FROM settings WHERE key = 'metadata_providers'")
        .fetch_optional(&pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_database_that_routes_nothing_yet_drops_the_seeded_tmdb() {
    assert_eq!(upgraded(&[]).await, None);
}

/// Its rules may rely on what only TMDB answers.
#[tokio::test]
async fn an_installation_with_an_instance_keeps_tmdb_across_the_upgrade() {
    assert_eq!(upgraded(&[AN_INSTANCE]).await.as_deref(), Some("arr,tmdb"));
}

#[tokio::test]
async fn a_tmdb_key_stored_in_the_interface_keeps_tmdb_across_the_upgrade() {
    let key = "INSERT INTO settings (key, value) VALUES ('tmdb_api_key', 'sealed')";

    assert_eq!(upgraded(&[key]).await.as_deref(), Some("arr,tmdb"));
}

#[tokio::test]
async fn a_source_list_someone_chose_comes_through_the_upgrade_unchanged() {
    let chosen = "UPDATE settings SET value = 'arr,anilist' WHERE key = 'metadata_providers'";

    assert_eq!(upgraded(&[chosen]).await.as_deref(), Some("arr,anilist"));
}
