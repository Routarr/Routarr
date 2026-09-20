//! Several metadata sources, ordered by priority.
//!
//! The two properties worth defending: a library with **no TMDb key at all**
//! still routes on genre, language and certification, because Radarr and Sonarr
//! carry those in the payload the sync already reads; and when two sources
//! disagree, the order the user set decides, field by field, with the loser
//! still filling in what the winner had nothing to say about.

use crate::services::routing::{self, SimulationOptions};
use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

async fn synced(kind: &str, arr: &FakeArr) -> TestApp {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", kind, &arr.base_url).await;
    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();
    app
}

async fn seed_rule(app: &TestApp, condition: serde_json::Value) {
    sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-anime', 'anime')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('r-1', 'Source rule', 10, 1, 'both', ?, 'anime', 'all')",
    )
    .bind(serde_json::json!([condition]).to_string())
    .execute(&app.state.pool)
    .await
    .unwrap();
}

async fn set_order(app: &TestApp, order: &str) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('metadata_providers', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(order)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

async fn decided_category(app: &TestApp) -> String {
    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: false, ..Default::default() },
    )
    .await
    .unwrap();
    result.decisions[0].target_category.clone()
}

/// Cache a TMDb answer for the fake Radarr's movie, deliberately different from
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
    let app = synced("radarr", &arr).await;

    let row: (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT genres, original_language, certification FROM media")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    assert_eq!(row.0.as_deref(), Some(r#"["Animation","Family"]"#));
    // Radarr answers "Japanese"; a rule is written against `ja`.
    assert_eq!(row.1.as_deref(), Some("ja"));
    assert_eq!(row.2.as_deref(), Some("G"));
}

#[tokio::test]
async fn a_genre_rule_matches_with_no_tmdb_key_configured() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    // `AppState::for_tests` configures no TMDb key, and nothing was enriched.
    assert!(app.state.config.tmdb_api_key.is_none());
    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn an_original_language_rule_matches_the_code_not_the_arrs_wording() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "original_language", "value": ["ja"] })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn a_keyword_rule_cannot_match_from_the_arr_alone() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    set_order(&app, "arr").await;
    seed_rule(&app, serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;

    // The one thing no Arr reports. Silence, not a wrong match.
    assert_eq!(decided_category(&app).await, "standard");
}

// ------------------------------------------------------------ priority order

#[tokio::test]
async fn the_first_source_in_the_order_wins_the_field() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn reordering_the_sources_changes_the_decision() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "tmdb,arr").await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    // Same library, same rule, same data: only the order moved.
    assert_eq!(decided_category(&app).await, "standard");
}

#[tokio::test]
async fn a_lower_source_still_fills_what_the_higher_one_lacks() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    // Radarr wins the genres above; keywords exist only in TMDb's answer and
    // must still be reachable.
    seed_rule(&app, serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

/// The Arr's own metadata is always read. A list saved without it — by an
/// older version, or by hand in the database — must not leave a library whose
/// genre rules silently stop matching, and the API refuses to save one.
#[tokio::test]
async fn the_arr_source_cannot_be_turned_off() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    set_order(&app, "").await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(decided_category(&app).await, "anime");

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
    let app = synced("radarr", &arr).await;
    // A database written by a build that knew a source this one does not.
    set_order(&app, "trakt,arr").await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

// ------------------------------------------------------------ explainability

#[tokio::test]
async fn the_explanation_names_the_source_that_answered() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    cache_tmdb(&app, r#"["Documentary"]"#).await;
    set_order(&app, "arr,tmdb").await;
    seed_rule(&app, serde_json::json!({ "type": "genre_contains", "value": ["Animation"] })).await;

    let response = app.get("/api/v1/media/m-inst-1-10/explain").await;
    let explanation = response.assert_ok();
    let condition = &explanation["rule_traces"][0]["conditions"][0];

    assert_eq!(condition["matched"], true);
    // Two sources hold genres and they disagree; without this the explanation
    // cannot be checked against either of them.
    assert_eq!(condition["source"], "arr");
}

#[tokio::test]
async fn the_media_page_lists_the_sources_that_contributed() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
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
    // Usable with no configuration at all — the whole point of it.
    assert_eq!(arr["configured"], true);

    let tmdb = &response["providers"][1];
    assert_eq!(tmdb["id"], "tmdb");
    assert_eq!(tmdb["needs_key"], true);
    assert_eq!(tmdb["configured"], false);
    // Named, so the interface can say what to set rather than "a key is
    // missing" — which nobody can act on.
    assert_eq!(tmdb["key_env"], "TMDB_API_KEY");

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

/// A cached row is not the same as a cached *answer*.
///
/// One holding only a synopsis is unreadable by every condition, so the engine
/// and the list must both say the item is unknown. Counting it made the column
/// promise metadata about an item no rule could ever touch — and the moment the
/// library pass stopped loading the synopsis, the two started saying opposite
/// things about the same item.
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
/// The predicate filtered on nothing, so a row cached by a source the operator
/// disabled yesterday still made the column promise metadata — while the engine,
/// which reads the enabled order, treated the same item as undescribed.
#[tokio::test]
async fn a_disabled_source_no_longer_answers_for_an_item() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
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

/// A series TheTVDB describes counts, though it carries no TMDb id.
///
/// The predicate looked only in the `tmdb_id` namespace, so a series enriched by
/// a source that addresses by `tvdb_id` read as undescribed for ever — on the
/// one screen somebody opens to find out why a rule matches nothing.
#[tokio::test]
async fn a_series_known_only_to_thetvdb_is_not_undescribed() {
    let arr = FakeArr::start().await;
    let app = synced("sonarr", &arr).await;
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
        "a TheTVDB answer was invisible for want of a TMDb id"
    );
}

/// The column, the diagnostics count and the warning beside it are one question.
///
/// Three spellings of it is how a badge ends up contradicting the number above
/// it, and each of the three had its own.
#[tokio::test]
async fn the_three_metadata_counters_agree_on_one_library() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    // Half described by the Arr, half by nothing at all.
    sqlx::query(
        "UPDATE media SET genres = '[]' WHERE id != (SELECT id FROM media ORDER BY id LIMIT 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    set_order(&app, "arr").await;

    let listed = app.get("/api/v1/media?per_page=100").await.assert_ok().clone();
    let without =
        listed["data"].as_array().unwrap().iter().filter(|m| m["has_metadata"] == false).count()
            as i64;

    let health = app.get("/api/v1/health?probe=false").await.assert_ok().clone();
    assert_eq!(
        health["metadata"]["media_missing_metadata"], without,
        "the diagnostics count and the library column disagree"
    );

    let warning = health["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|w| w.as_str().filter(|w| w.contains("no metadata") || w.contains("Metadata")))
        .map(str::to_string);
    if without > 0 {
        let warning = warning.expect("a library with undescribed items warns about them");
        assert!(
            warning.contains(&without.to_string()),
            "the warning counts differently from the column: {warning}"
        );
    }
}

#[tokio::test]
async fn a_cached_synopsis_alone_is_not_metadata_to_either_of_them() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;

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
