//! `GET /route`: where a title another service names would go, whether the
//! library holds it or only an Arr knows it, with nothing stored.

use axum::http::StatusCode;
use serde_json::Value;

use super::TestApp;
use super::fake_arr::FakeArr;
use super::fake_tmdb::FakeTmdb;
use crate::jobs::Attribution;

/// A Radarr synced into the library, Totoro in `/movies/standard`, the anime
/// rule, and `anime` and `kids` mapped onto the fake's folders.
async fn radarr_library(app: &TestApp, arr: &FakeArr) {
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    crate::services::sync::sync_instance(&app.state, "inst-1", &Attribution::manual(None))
        .await
        .unwrap();
    app.seed_anime_rule().await;
    for statement in [
        "INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime'), ('cat-kids', 'kids')",
        "UPDATE root_folders SET category = 'anime' WHERE path = '/movies/anime'",
        "UPDATE root_folders SET category = 'kids' WHERE path = '/movies/kids'",
        "UPDATE root_folders SET category = 'standard' WHERE path = '/movies/standard'",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }
}

async fn count(app: &TestApp, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

fn only(answer: &Value) -> &Value {
    let answers = answer["answers"].as_array().expect("answers");
    assert_eq!(answers.len(), 1, "{answer}");
    &answers[0]
}

#[tokio::test]
async fn a_title_the_library_holds_goes_where_the_simulation_sends_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    // Tried after the winner, so what it reads cannot change the answer.
    app.seed_rule_on(serde_json::json!({ "type": "origin_country", "value": ["JP"] })).await;
    sqlx::query("UPDATE rules SET priority = 50 WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let placed = app.get("/api/v1/route?type=movie&tmdb=8392").await;
    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "library");
    assert_eq!(answer["category"], "anime");
    assert_eq!(answer["root_folder"]["path"], "/movies/anime");
    assert_eq!(answer["root_folder"]["origin"], "arr");
    assert_eq!(answer["current_root_folder"], "/movies/standard");
    assert_eq!(answer["action"], "move");
    assert_eq!(answer["rule"], "Anime");
    assert_eq!(answer["unanswered_fields"], serde_json::json!([]));

    // The explanation and the simulation give the same answer for the title.
    let media_id = answer["media_id"].as_str().unwrap();
    let explained = app.get(&format!("/api/v1/media/{media_id}/explain")).await;
    assert_eq!(explained.assert_ok()["target_category"], answer["category"]);
}

#[tokio::test]
async fn a_title_only_the_arr_knows_is_looked_up_and_placed_without_being_stored() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    let (media, decisions) = (count(&app, "media").await, count(&app, "decisions").await);

    for query in ["tmdb=129", "imdb=tt0245429"] {
        let placed = app.get(&format!("/api/v1/route?type=movie&{query}")).await;
        let answer = only(placed.assert_ok());
        assert_eq!(answer["source"], "lookup", "{query}");
        assert_eq!(answer["media_id"], Value::Null);
        assert_eq!(answer["title"], "Spirited Away");
        assert_eq!(answer["category"], "anime");
        assert_eq!(answer["root_folder"]["path"], "/movies/anime");
        assert_eq!(answer["action"], "add", "a title the Arr does not hold is added, not moved");
    }
    assert_eq!(count(&app, "media").await, media, "a looked-up title was stored");
    assert_eq!(count(&app, "decisions").await, decisions, "a placement was stored");
    assert!(arr.recorded().reads.iter().any(|read| read == "/api/v3/movie/lookup/129"));
}

/// A title nobody knows is not found, and one no instance of its kind can
/// hold is not found either.
#[tokio::test]
async fn a_title_no_instance_knows_is_not_found() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;

    let missing = app.get("/api/v1/route?type=movie&tmdb=999999").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let no_sonarr = app.get("/api/v1/route?type=series&tvdb=76885").await;
    assert_eq!(no_sonarr.status, StatusCode::NOT_FOUND);
    let refused = app.get("/api/v1/route?type=movie").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

/// One Arr that cannot be asked is said, and the other still answers.
#[tokio::test]
async fn an_instance_that_fails_is_reported_and_stops_none_of_the_others() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-down', 'Radarr 4K', 'radarr', 'http://127.0.0.1:1', ?, 1, 'tok-down')",
    )
    .bind(app.state.secrets.seal("arr-key").unwrap())
    .execute(&app.state.pool)
    .await
    .unwrap();

    let placed = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let body = placed.assert_ok();
    assert_eq!(only(body)["instance_id"], "inst-1");
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{body}");
    assert_eq!(errors[0]["instance_id"], "inst-down");

    let narrowed = app.get("/api/v1/route?type=movie&tmdb=129&instance=inst-1").await;
    assert!(narrowed.assert_ok()["errors"].as_array().unwrap().is_empty());
}

/// The tags a requester would add the title with reach a rule reading them.
#[tokio::test]
async fn the_tags_a_new_title_would_carry_reach_the_rules() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "tag_in", "value": ["4k"] })).await;
    sqlx::query("UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let plain = app.get("/api/v1/route?type=movie&tmdb=129").await;
    assert_eq!(only(plain.assert_ok())["category"], "anime");
    let tagged = app.get("/api/v1/route?type=movie&tmdb=129&tags=hdr,%204k").await;
    assert_eq!(only(tagged.assert_ok())["category"], "kids");
}

/// A rule tried before the winner reads a field nobody answered, so the
/// placement could change once a source does. `enrich` asks the source now,
/// and stores nothing it hears.
#[tokio::test]
async fn enrich_asks_a_source_for_what_the_rules_read_and_stores_nothing() {
    let arr = FakeArr::start().await;
    let tmdb = FakeTmdb::start().await;
    let app = TestApp::new().await;
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    let app = TestApp::around(app.state.clone().with_config(config));
    crate::services::maintenance::converge_metadata_sources(&app.state).await.unwrap();
    radarr_library(&app, &arr).await;
    // Tried before the anime rule, and only TMDb answers keywords.
    app.seed_rule_on(serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;
    sqlx::query("UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let cached = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let answer = only(cached.assert_ok());
    assert_eq!(answer["category"], "anime");
    assert_eq!(answer["unanswered_fields"], serde_json::json!(["keywords"]));

    let enriched = app.get("/api/v1/route?type=movie&tmdb=129&enrich=true").await;
    let answer = only(enriched.assert_ok());
    assert_eq!(answer["category"], "kids", "{answer}");
    assert_eq!(answer["unanswered_fields"], serde_json::json!([]));
    assert_eq!(count(&app, "metadata_cache").await, 0, "an enriched placement stored an answer");
}

#[tokio::test]
async fn a_series_is_placed_through_sonarr() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("sonarr", &arr).await;

    let placed = app.get("/api/v1/route?type=series&tvdb=76885").await;
    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "library");
    assert_eq!(answer["title"], "Cowboy Bebop");
    let unknown = app.get("/api/v1/route?type=series&tvdb=1").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}
