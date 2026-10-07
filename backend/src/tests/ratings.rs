//! Which rating a rule reads when several sources rate one title, and how the
//! interface names the ratings and the countries a library holds.
//!
//! Each rating belongs to a country's system: TMDB and TheTVDB rate for many
//! countries, of which the certification regions pick one, OMDb rates for the
//! United States, a Radarr for the country its metadata settings name, a
//! Sonarr for the United States, and MyAnimeList has a system of its own. The
//! regions decide among them.

use super::TestApp;
use super::fake_arr::FakeArr;
use serde_json::json;

/// The seeded library with the Arr rating Totoro `arr` in its instance's
/// `country`, and TMDB rating it `tmdb` in `tmdb_country`, under `regions`.
async fn rated(
    arr: Option<&str>,
    country: &str,
    tmdb: Option<&str>,
    tmdb_country: &str,
    regions: &str,
) -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET certification = ? WHERE id = 'm-1'")
        .bind(arr)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE instances SET certification_country = ? WHERE id = 'inst-1'")
        .bind(country)
        .execute(&app.state.pool)
        .await
        .unwrap();
    tmdb_rates(&app, tmdb.map(|rating| json!({ tmdb_country: rating })).unwrap_or(json!({}))).await;
    app.store_setting("certification_regions", regions).await;
    app
}

/// TMDB's answer for Totoro, rating it as `by_country` says, as the
/// enrichment stores it.
async fn tmdb_rates(app: &TestApp, by_country: serde_json::Value) {
    sqlx::query(
        "UPDATE metadata_cache SET certification = NULL, certifications = ?
          WHERE source = 'tmdb' AND external_id = '8392'",
    )
    .bind(by_country.to_string())
    .execute(&app.state.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_rating_of_the_first_region_wins_whatever_the_order_of_the_sources() {
    // The Arr comes first in the order of the sources, and rates for the US.
    let app = rated(Some("PG"), "US", Some("U"), "FR", "FR,US").await;
    app.seed_rule_on(json!({ "type": "certification_in", "value": ["U"] })).await;
    assert_eq!(app.decided_category().await, "anime", "France comes first among the regions");
}

#[tokio::test]
async fn the_arrs_rating_stands_when_it_alone_rates_the_title() {
    let app = rated(Some("PG"), "US", None, "FR", "FR").await;
    app.seed_rule_on(json!({ "type": "certification_in", "value": ["PG"] })).await;
    assert_eq!(app.decided_category().await, "anime", "a rating outside the regions beats none");
}

#[tokio::test]
async fn ratings_outside_the_regions_follow_the_order_of_the_sources() {
    // The Arr rates for the US and MyAnimeList in its own system, both
    // outside France, and TMDB rates for no region.
    let app = rated_by_myanimelist("R+", "PG").await;
    app.execute(&[
        "UPDATE media SET certification = 'PG-13' WHERE id = 'm-1'",
        "UPDATE instances SET certification_country = 'US'",
    ])
    .await;
    app.store_setting("certification_regions", "FR").await;
    app.seed_rule_on(json!({ "type": "certification_in", "value": ["PG-13"] })).await;
    assert_eq!(app.decided_category().await, "anime", "the Arr is listed first");

    app.store_setting("metadata_providers", "jikan,arr,tmdb").await;
    assert_ne!(app.decided_category().await, "anime", "MyAnimeList is listed first");
}

/// TMDB and TheTVDB rate a title for many countries, and the regions pick one
/// each time the answer is read: a change of regions holds from the next
/// simulation, with nothing asked again.
#[tokio::test]
async fn a_region_change_reaches_the_cached_tmdb_rating() {
    let app = rated(None, "US", None, "US", "FR").await;
    tmdb_rates(&app, json!({ "FR": "12", "US": "PG-13" })).await;
    app.seed_rule_on(json!({ "type": "certification_in", "value": ["PG-13"] })).await;
    assert_ne!(app.decided_category().await, "anime", "the French rating is read");
    assert_eq!(facet_values(&app).await, ["12"]);

    app.put("/api/v1/settings", json!({ "settings": { "certification_regions": "US, FR" } }))
        .await
        .assert_ok();

    assert_eq!(app.decided_category().await, "anime", "the US now comes first");
    assert_eq!(facet_values(&app).await, ["PG-13"], "the facets read the same rating");
    let explained = app.get("/api/v1/media/m-1/explain").await;
    let metadata = &explained.assert_ok()["metadata"];
    assert_eq!(metadata["certification"], "PG-13");
    assert_eq!(metadata["certification_scale"], "US");
}

/// A blank rating claims nothing, as the merge of the sources treats it: the
/// Arr's empty one, first in the order and in the region, leaves TMDB's.
#[tokio::test]
async fn a_blank_rating_does_not_erase_a_real_one() {
    let app = rated(Some(""), "US", Some("PG"), "US", "US").await;
    app.seed_rule_on(json!({ "type": "certification_in", "value": ["PG"] })).await;
    assert_eq!(app.decided_category().await, "anime");
}

#[tokio::test]
async fn a_sync_reads_the_country_a_radarr_rates_for_and_a_sonarr_rates_for_the_us() {
    let arr = FakeArr::start().await;
    arr.rate_for("gb");
    let app = TestApp::synced_from("radarr", &arr).await;
    let country: Option<String> =
        sqlx::query_scalar("SELECT certification_country FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(country.as_deref(), Some("GB"));

    let app = TestApp::synced_from("sonarr", &arr).await;
    let country: Option<String> =
        sqlx::query_scalar("SELECT certification_country FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(country.as_deref(), Some("US"));
}

#[tokio::test]
async fn an_arr_rating_for_a_country_outside_the_regions_is_reported() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.store_setting("certification_regions", "FR,BE").await;

    let status = app.get("/api/v1/status").await;
    let warning = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|warning| warning["code"] == "certification_country_outside_regions")
        .cloned()
        .expect("the Arr's country is outside the regions");
    let message = warning["message"].as_str().unwrap();
    assert!(message.contains("US") && message.contains("FR, BE"), "{message}");

    app.store_setting("certification_regions", "FR,US").await;
    let status = app.get("/api/v1/status").await;
    let codes: Vec<&str> = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning["code"].as_str())
        .collect();
    assert!(!codes.contains(&"certification_country_outside_regions"), "{codes:?}");
}

#[tokio::test]
async fn a_retired_country_is_named_in_the_readers_language() {
    let app = TestApp::new().await;
    app.put("/api/v1/settings", json!({ "settings": { "ui_language": "fr" } })).await.assert_ok();
    let facets = app.get("/api/v1/media/facets").await;
    let countries =
        facets.assert_ok()["vocabularies"]["origin_countries"].as_array().unwrap().clone();
    let label = |code: &str| {
        countries
            .iter()
            .find(|facet| facet["value"] == code)
            .and_then(|facet| facet["label"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(label("XG"), "Allemagne de l'Est (XG)");
    assert_eq!(label("SU"), "Union soviétique (SU)");
    assert_eq!(label("XC"), "Tchécoslovaquie (XC)");
}

/// A library holding one rating MyAnimeList gave, `code`, and the US rating
/// `us` TMDB gave.
async fn rated_by_myanimelist(code: &str, us: &str) -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
         VALUES ('jikan', 'movie', 'tmdb:8392', '523')",
        "UPDATE settings SET value = 'arr,tmdb,jikan' WHERE key = 'metadata_providers'",
    ])
    .await;
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         origin_countries, certification, certification_scale, expires_at)
         VALUES ('jikan', '523', 'movie', '[]', '[]', '[]', ?, 'MAL', '2099-01-01')",
    )
    .bind(code)
    .execute(&app.state.pool)
    .await
    .unwrap();
    tmdb_rates(&app, json!({ "US": us })).await;
    app
}

async fn facet_values(app: &TestApp) -> Vec<String> {
    let facets = app.get("/api/v1/media/facets").await;
    let certifications = facets.assert_ok()["certifications"].as_array().unwrap().clone();
    certifications.iter().map(|facet| facet["value"].as_str().unwrap().to_string()).collect()
}

async fn certification_facet(app: &TestApp, code: &str) -> serde_json::Value {
    let facets = app.get("/api/v1/media/facets").await;
    facets.assert_ok()["certifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|facet| facet["value"] == code)
        .cloned()
        .unwrap_or_else(|| panic!("no facet {code}: {:?}", facets.json))
}

#[tokio::test]
async fn a_myanimelist_rating_shows_its_words_and_joins_its_age() {
    let app = rated_by_myanimelist("R+", "PG").await;
    let facet = certification_facet(&app, "R+").await;
    assert_eq!(facet["label"], "R+ (mild nudity)");
    assert_eq!(facet["group"], "17 and over");
}

#[tokio::test]
async fn an_r_is_named_once_every_system_giving_it_agrees_on_its_age() {
    // MyAnimeList's R and the MPA's are both seventeen and over.
    let app = rated_by_myanimelist("R", "R").await;
    let facet = certification_facet(&app, "R").await;
    assert_eq!(facet["group"], "17 and over", "{facet}");
}

/// An upgrade gives every cached rating the system it was issued under where
/// the source settles it, and has TMDB and TheTVDB asked again for theirs,
/// which they rated for a region they did not name.
#[tokio::test]
async fn an_upgrade_gives_each_cached_rating_its_system() {
    let pool = super::database_through("014_routing_generation").await;
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, certification, expires_at)
         VALUES ('omdb', 'tt1', 'movie', 'PG-13', '2099-01-01 00:00:00'),
                ('omdb', 'tt2', 'movie', NULL, '2099-01-01 00:00:00'),
                ('jikan', '5', 'series', 'R+', '2099-01-01 00:00:00'),
                ('tmdb', '8', 'movie', 'PG', '2099-01-01 00:00:00'),
                ('tvdb', '9', 'series', NULL, '2099-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations_through(&pool, "015_certification_scale").await.unwrap();

    let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT source || ':' || external_id, certification_scale, expires_at <= datetime('now')
           FROM metadata_cache ORDER BY source, external_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let expected = [
        ("jikan:5", Some("MAL"), false),
        ("omdb:tt1", Some("US"), false),
        ("omdb:tt2", None, false),
        ("tmdb:8", None, true),
        ("tvdb:9", None, false),
    ];
    let rows: Vec<(&str, Option<&str>, bool)> =
        rows.iter().map(|(key, scale, due)| (key.as_str(), scale.as_deref(), *due)).collect();
    assert_eq!(rows, expected);
}

/// An upgrade moves the one rating each TMDB and TheTVDB answer kept under
/// its country, and has them asked again for every country's.
#[tokio::test]
async fn an_upgrade_keeps_each_cached_rating_under_its_country() {
    let pool = super::database_through("031_zero_ids").await;
    sqlx::query(
        "INSERT INTO metadata_cache
            (source, external_id, media_type, certification, certification_scale, expires_at)
         VALUES ('tmdb', '1', 'movie', 'PG', 'US', '2099-01-01 00:00:00'),
                ('tvdb', '2', 'series', '-12', 'FR', '2099-01-01 00:00:00'),
                ('tmdb', '3', 'movie', 'G', NULL, '2099-01-01 00:00:00'),
                ('tmdb', '4', 'movie', NULL, NULL, '2099-01-01 00:00:00'),
                ('omdb', 'tt5', 'movie', 'PG-13', 'US', '2099-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations_through(&pool, "032_every_regions_rating").await.unwrap();

    type Row = (String, String, Option<String>, Option<String>, bool);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT source || ':' || external_id, certifications, certification,
                certification_scale, expires_at <= datetime('now')
           FROM metadata_cache ORDER BY source, external_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let us = (Some("PG-13".to_string()), Some("US".to_string()));
    let expected: Vec<Row> = vec![
        ("omdb:tt5".into(), "{}".into(), us.0, us.1, false),
        ("tmdb:1".into(), r#"{"US":"PG"}"#.into(), None, None, true),
        ("tmdb:3".into(), "{}".into(), None, None, true),
        ("tmdb:4".into(), "{}".into(), None, None, false),
        ("tvdb:2".into(), r#"{"FR":"-12"}"#.into(), None, None, true),
    ];
    assert_eq!(rows, expected);
}
