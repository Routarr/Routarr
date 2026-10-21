//! Scores, by scale: Radarr's on the media row, each source's in the cache, read by a
//! `score_range` rule and counted as metadata a rule reads.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;
use super::fake_arr::FakeArr;

/// Radarr's scores reach the media row at sync, and a rule on one routes the title by its
/// score and its votes.
#[tokio::test]
async fn radarr_s_scores_reach_the_row_and_route_a_rule_on_them() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT ratings FROM media WHERE id = 'm-inst-1-10'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&stored.unwrap()).unwrap();
    assert_eq!(stored["imdb"], json!({ "value": 8.1, "votes": 393000 }));
    assert_eq!(stored["rotten_tomatoes"], json!({ "value": 94.0, "votes": null }));

    for (condition, routed) in [
        (json!({ "scale": "imdb", "min": 8, "max": null, "min_votes": 1000 }), true),
        (json!({ "scale": "imdb", "min": 8.5, "max": null }), false),
        (json!({ "scale": "imdb", "min": null, "max": null, "min_votes": 500000 }), false),
        (json!({ "scale": "rotten_tomatoes", "min": 90, "max": 100 }), true),
        (json!({ "scale": "tmdb", "min": 0, "max": 10 }), false),
    ] {
        app.execute(&["DELETE FROM rules"]).await;
        app.seed_rule_on(json!({ "type": "score_range", "value": condition })).await;
        let explained = app.get("/api/v1/library/m-inst-1-10/explain").await;
        let explained = explained.assert_ok();
        assert_eq!(explained["rules_category"] == "anime", routed, "{condition}: {explained}");
        assert_eq!(explained["metadata"]["ratings"]["imdb"]["value"], 8.1);
    }
}

/// A row holding no score but an empty object holds nothing a rule reads, and one score is
/// something, as `MediaMetadata::merge` counts them.
#[tokio::test]
async fn an_empty_score_list_is_no_metadata_and_one_score_is() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "UPDATE media SET genres = '[]', original_language = NULL, certification = NULL,
                          tmdb_id = NULL, imdb_id = NULL, ratings = '{}'",
        "DELETE FROM metadata_cache",
    ])
    .await;
    assert!(!known(&app).await, "an empty object counted as a score");

    app.execute(&[r#"UPDATE media SET ratings = '{"imdb":{"value":7.2,"votes":40}}'"#]).await;
    assert!(known(&app).await);
}

/// Whether the library says the first title has metadata a rule reads.
async fn known(app: &TestApp) -> bool {
    let listed = app.get("/api/v1/library").await;
    listed.assert_ok()["data"][0]["has_metadata"].as_bool().unwrap()
}

/// A score range is refused with the reason the validator gives, before it is stored.
#[tokio::test]
async fn a_score_off_its_scale_is_refused_on_save() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let rule = json!({
        "name": "Acclaimed",
        "target_category": "anime",
        "media_type": "both",
        "conditions": [
            { "type": "score_range", "value": { "scale": "imdb", "min": 70, "max": null } }
        ],
    });
    let refused = app.post("/api/v1/rules", rule).await;
    let message = refused.assert_status(StatusCode::BAD_REQUEST)["message"].as_str().unwrap();
    assert!(message.contains("between 0 and 10"), "{message}");
    assert_eq!(app.count("SELECT COUNT(*) FROM rules").await, 0);
}
