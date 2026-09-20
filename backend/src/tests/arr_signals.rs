//! The four signals the Arr already reports, and that rules can match on.
//!
//! Their value is that they need no external provider: a tag is the user's own
//! intent, and Sonarr's `seriesType` settles the anime question that TMDb genres
//! answer badly. These tests follow each one from the Arr's JSON through the
//! database to a rule that matches on it.

use crate::services::routing::{self, SimulationOptions};
use crate::services::sync;

use super::TestApp;
use super::fake_arr::FakeArr;

/// Sync a library from the fake Arr and hand back the app.
async fn synced(kind: &str, arr: &FakeArr) -> TestApp {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", kind, &arr.base_url).await;
    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();
    app
}

/// Add a rule whose only condition is the one under test.
async fn seed_rule(app: &TestApp, condition: serde_json::Value) {
    sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-anime', 'anime')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('r-1', 'Signal rule', 10, 1, 'both', ?, 'anime', 'all')",
    )
    .bind(serde_json::json!([condition]).to_string())
    .execute(&app.state.pool)
    .await
    .unwrap();
}

/// The category the engine settles on for the first media item.
async fn decided_category(app: &TestApp) -> String {
    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: false, ..Default::default() },
    )
    .await
    .unwrap();
    result.decisions[0].target_category.clone()
}

#[tokio::test]
async fn sync_stores_the_tags_a_user_attached_in_radarr() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;

    // Stored as labels, not ids: a rule must be written against "anime", and
    // ids are not stable from one instance to the next.
    let tags: Option<String> =
        sqlx::query_scalar("SELECT tags FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(tags.as_deref(), Some(r#"["anime"]"#));

    // The catalogue itself is kept, so the rule editor can offer real choices.
    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM arr_tags WHERE instance_id = 'inst-1' ORDER BY label",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(labels, vec!["anime", "kids"]);
}

#[tokio::test]
async fn a_rule_can_route_on_an_arr_tag() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "tag_in", "value": ["anime", "docs"] })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn a_tag_the_media_does_not_carry_does_not_match() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "tag_in", "value": ["concerts"] })).await;

    assert_eq!(decided_category(&app).await, "standard", "the default category");
}

#[tokio::test]
async fn sonarrs_own_series_type_settles_the_anime_question() {
    let arr = FakeArr::start().await;
    let app = synced("sonarr", &arr).await;

    let series_type: Option<String> = sqlx::query_scalar("SELECT series_type FROM media")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(series_type.as_deref(), Some("anime"));

    seed_rule(&app, serde_json::json!({ "type": "series_type_is", "value": ["anime"] })).await;
    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn a_movie_never_matches_a_series_type_condition() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    // Radarr has no series type, and an absent value must not satisfy a
    // condition — otherwise every film would match "not standard".
    seed_rule(&app, serde_json::json!({ "type": "series_type_is", "value": ["anime"] })).await;

    assert_eq!(decided_category(&app).await, "standard");
}

#[tokio::test]
async fn specials_do_not_count_towards_the_season_total() {
    let arr = FakeArr::start().await;
    let app = synced("sonarr", &arr).await;

    // The fake reports seasons 0, 1 and 2. Season 0 is bonus material: counting
    // it would make "more than two seasons" true for a two-season show.
    let seasons: Option<i64> = sqlx::query_scalar("SELECT season_count FROM media")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(seasons, Some(2));

    seed_rule(&app, serde_json::json!({ "type": "season_count_over", "value": 2 })).await;
    assert_eq!(decided_category(&app).await, "standard", "two is not more than two");
}

#[tokio::test]
async fn a_long_running_series_can_be_routed_to_an_archive() {
    let arr = FakeArr::start().await;
    let app = synced("sonarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "season_count_over", "value": 1 })).await;

    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn size_on_disk_is_compared_in_gigabytes_on_both_sides_of_the_threshold() {
    let arr = FakeArr::start().await;
    let app = synced("sonarr", &arr).await;

    // The fake reports 200 GiB.
    let bytes: Option<i64> = sqlx::query_scalar("SELECT size_on_disk FROM media")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(bytes, Some(214_748_364_800));

    seed_rule(&app, serde_json::json!({ "type": "size_on_disk_over_gb", "value": 500 })).await;
    assert_eq!(decided_category(&app).await, "standard", "200 GiB is under 500 GB");

    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();
    seed_rule(&app, serde_json::json!({ "type": "size_on_disk_over_gb", "value": 100 })).await;
    assert_eq!(decided_category(&app).await, "anime");
}

#[tokio::test]
async fn the_explanation_names_the_signal_and_what_was_observed() {
    let arr = FakeArr::start().await;
    let app = synced("radarr", &arr).await;
    seed_rule(&app, serde_json::json!({ "type": "tag_in", "value": ["anime"] })).await;

    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: false, ..Default::default() },
    )
    .await
    .unwrap();

    // Expected *and* observed, like every other condition: the explainability
    // contract does not get an exemption for new signals.
    let reason = result.decisions[0].reasons.join(" ");
    assert!(reason.contains("anime"), "got {reason:?}");
    assert!(reason.contains("found"), "the observed value must be shown: {reason:?}");
}

#[tokio::test]
async fn the_new_signals_are_offered_by_the_condition_catalogue() {
    let app = TestApp::new().await;
    let catalogue = app.get("/api/v1/rules/conditions").await;
    let body = catalogue.assert_ok();
    let conditions = body["conditions"].as_array().unwrap();
    let types: Vec<&str> = conditions.iter().filter_map(|c| c["type"].as_str()).collect();

    for expected in ["tag_in", "series_type_is", "size_on_disk_over_gb", "season_count_over"] {
        assert!(types.contains(&expected), "{expected} missing from the catalogue");
    }

    // None of them needs TMDb — that is the point of using what the Arr knows.
    for condition in conditions {
        if condition["type"] == "tag_in" || condition["type"] == "series_type_is" {
            assert_eq!(condition["needs_metadata"], false);
            // The label is translated, not the raw kind.
            assert_ne!(condition["label"], condition["type"]);
        }
    }
}

#[tokio::test]
async fn an_arr_without_a_tag_endpoint_still_syncs() {
    // Tags are one signal among many. An Arr too old to expose `/api/v3/tag`,
    // or one that errors on it, must not take the whole library down with it.
    let arr = FakeArr::failing(500).await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let report = sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    assert!(report.media > 0, "the library must still be read");
}
