//! Keeping a title where it is: an exception to the category of its current folder.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;

/// Totoro in `/movies/standard`, which the anime rule would move.
async fn proposed_to_move() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    app.simulate().await;
    let pending = app.get("/api/v1/decisions?status=pending").await;
    assert_eq!(pending.assert_ok()["data"][0]["target_category"], "anime", "no move to keep from");
    app
}

#[tokio::test]
async fn keeping_a_title_pins_its_folder_category_and_withdraws_the_move() {
    let app = proposed_to_move().await;

    let kept = app.post("/api/v1/media/m-1/keep", json!({})).await;

    assert_eq!(kept.assert_status(StatusCode::CREATED)["target_category"], "standard");
    let pending = app.get("/api/v1/decisions?status=pending").await;
    assert_eq!(pending.assert_ok()["data"], json!([]), "the title stays, so nothing is proposed");
    let routed: String =
        sqlx::query_scalar("SELECT category FROM media_routing WHERE media_id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(routed, "standard", "the title is decided again at once");
    let explained = app.get("/api/v1/media/m-1/explain").await;
    let explained = explained.assert_ok();
    assert_eq!(explained["current_category"], "standard");
    assert_eq!(explained["override_category"], "standard");
}

#[tokio::test]
async fn a_proposal_names_the_category_of_the_folder_its_title_is_in() {
    let app = proposed_to_move().await;

    let pending = app.get("/api/v1/decisions?status=pending").await;
    let simulated = app.post("/api/v1/simulate", json!({})).await;

    assert_eq!(pending.assert_ok()["data"][0]["current_category"], "standard");
    assert_eq!(simulated.assert_ok()["decisions"][0]["current_category"], "standard");
}

#[tokio::test]
async fn a_title_whose_folder_leads_nowhere_or_to_two_places_is_not_kept() {
    let app = proposed_to_move().await;
    let refused = |reason: &'static str| {
        let app = &app;
        async move {
            let answer = app.post("/api/v1/media/m-1/keep", json!({})).await;
            assert_eq!(answer.assert_status(StatusCode::CONFLICT)["reason"], reason);
        }
    };
    let set = |sql: &'static str| {
        let app = &app;
        async move { sqlx::query(sql).execute(&app.state.pool).await.unwrap() }
    };

    set("INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-3', 'inst-1', 3, '/movies/more', 1, 'standard')")
    .await;
    refused("category_ambiguous").await;
    set("UPDATE root_folders SET category = NULL WHERE id = 'rf-1'").await;
    refused("folder_unmapped").await;
    set("UPDATE media SET current_root_folder = NULL WHERE id = 'm-1'").await;
    refused("no_current_folder").await;

    assert_eq!(app.count("SELECT COUNT(*) FROM overrides").await, 0);
    let missing = app.post("/api/v1/media/nobody/keep", json!({})).await;
    missing.assert_status(StatusCode::NOT_FOUND);
}

/// A category name typed with its accent apart from its letter names the category written with
/// it, as every writer of a category reads one.
#[tokio::test]
async fn a_pin_reads_a_category_name_as_its_stored_form() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.post("/api/v1/categories", json!({ "name": "léa" }))
        .await
        .assert_status(StatusCode::CREATED);

    let pin = json!({ "media_id": "m-1", "target_category": " Le\u{301}a " });
    let set = app.post("/api/v1/overrides", pin).await;

    assert_eq!(set.assert_status(StatusCode::CREATED)["target_category"], "léa");
}
