//! A category leading to several folders of one instance: a title in any of them stays, and one
//! entering the category goes to the folder with the most room.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;

/// Totoro, in `/movies/standard`, routed to `anime`, which `/movies/anime` and
/// `/movies/anime2` share, with `anime` and `anime2` free as given.
async fn anime_over_two_folders(anime: Option<i64>, anime2: Option<i64>) -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category, free_space)
         VALUES ('rf-3', 'inst-1', 3, '/movies/anime2', 1, 'anime', ?)",
    )
    .bind(anime2)
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE root_folders SET free_space = ? WHERE id = 'rf-2'")
        .bind(anime)
        .execute(&app.state.pool)
        .await
        .unwrap();
    app
}

async fn proposed(app: &TestApp, media_id: &str) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT action, target_root_folder FROM decisions
          WHERE media_id = ? AND superseded = 0",
    )
    .bind(media_id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

/// A second folder for a category is refused unless the request asks to join the set: written
/// by mistake, it would share the category's new titles without a word.
#[tokio::test]
async fn a_second_folder_joins_a_category_only_when_asked() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible)
         VALUES ('rf-3', 'inst-1', 3, '/movies/anime2', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let refused =
        app.put("/api/v1/root-folders/rf-3/category", json!({ "category": "anime" })).await;
    assert_eq!(refused.assert_status(StatusCode::CONFLICT)["reason"], "category_mapped");
    let joined = app
        .put(
            "/api/v1/root-folders/rf-3/category",
            json!({ "category": "anime", "add_to_set": true }),
        )
        .await;
    joined.assert_ok();
    assert_eq!(app.count("SELECT COUNT(*) FROM root_folders WHERE category = 'anime'").await, 2);
    let conflicts = app.get("/api/v1/root-folders/conflicts").await;
    assert_eq!(conflicts.assert_ok().clone(), json!([]), "a chosen set is no conflict");
}

/// A title entering the set goes to the folder with the most room, and a folder whose room is
/// unknown, as on a NAS asleep, comes after any known figure.
#[tokio::test]
async fn a_title_entering_a_set_goes_where_there_is_most_room() {
    let app = anime_over_two_folders(Some(100), Some(500)).await;
    app.simulate().await;
    assert_eq!(proposed(&app, "m-1").await, ("move".into(), Some("/movies/anime2".into())));

    let app = anime_over_two_folders(None, Some(10)).await;
    app.simulate().await;
    assert_eq!(proposed(&app, "m-1").await.1.as_deref(), Some("/movies/anime2"));

    let app = anime_over_two_folders(Some(900), Some(10)).await;
    app.simulate().await;
    assert_eq!(proposed(&app, "m-1").await.1.as_deref(), Some("/movies/anime"));
}

/// A pending proposal keeps its folder while it stays in the set, however the free space moves
/// at the syncs in between: the simulation, the explanation and the apply's revalidation agree.
#[tokio::test]
async fn a_pending_proposal_keeps_its_folder_as_free_space_changes() {
    let app = anime_over_two_folders(Some(100), Some(500)).await;
    app.simulate().await;
    app.execute(&["UPDATE root_folders SET free_space = 9000 WHERE id = 'rf-2'"]).await;

    app.simulate().await;
    assert_eq!(proposed(&app, "m-1").await.1.as_deref(), Some("/movies/anime2"));
    let explained = app.get("/api/v1/library/m-1/explain").await;
    assert_eq!(explained.assert_ok()["target_root_folder"], "/movies/anime2");
    let revalidated =
        crate::services::routing::revalidated_targets(&app.state.pool, &["m-1".to_string()])
            .await
            .unwrap();
    assert_eq!(revalidated["m-1"].as_deref(), Some("/movies/anime2"));

    app.execute(&["UPDATE root_folders SET category = NULL WHERE id = 'rf-3'"]).await;
    app.simulate().await;
    assert_eq!(proposed(&app, "m-1").await.1.as_deref(), Some("/movies/anime"), "out of the set");
}

/// A title already in any folder of its category's set stays there, and that folder is its
/// target, so the explanation never reads a move beside "no move".
#[tokio::test]
async fn a_title_in_any_folder_of_its_set_stays_there() {
    let app = anime_over_two_folders(Some(900), Some(10)).await;
    app.execute(&["UPDATE media SET current_root_folder = '/movies/anime2',
                          current_path = '/movies/anime2/My Neighbor Totoro (1988)'
          WHERE id = 'm-1'"])
        .await;

    app.simulate().await;

    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE action = 'move'").await, 0);
    let explained = app.get("/api/v1/library/m-1/explain").await;
    let explained = explained.assert_ok();
    assert_eq!(explained["action"], "none");
    assert_eq!(explained["target_root_folder"], "/movies/anime2");
}

/// One pass spreads its moves over the set: what it already sends into a folder counts
/// against that folder's room, or a whole library would land in the one folder that had most.
#[tokio::test]
async fn a_pass_spreads_its_moves_over_the_set() {
    let app = anime_over_two_folders(Some(1000), Some(900)).await;
    app.execute(&[
        "UPDATE media SET size_on_disk = 300 WHERE id = 'm-1'",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
                            current_path, current_root_folder, monitored, has_files, size_on_disk)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Spirited Away', 2001, 8392,
                 '/movies/standard/Spirited Away (2001)', '/movies/standard', 1, 1, 300)",
    ])
    .await;

    app.simulate().await;

    let mut targets =
        [proposed(&app, "m-1").await.1.unwrap(), proposed(&app, "m-2").await.1.unwrap()];
    targets.sort();
    assert_eq!(targets, ["/movies/anime", "/movies/anime2"]);
}

/// A title in a folder its category shares is kept there: any folder of the set is its place.
#[tokio::test]
async fn a_title_in_a_shared_folder_is_kept_there() {
    let app = anime_over_two_folders(Some(100), Some(500)).await;
    app.execute(&["UPDATE root_folders SET category = 'standard' WHERE id = 'rf-3'"]).await;
    app.simulate().await;

    let kept = app.post("/api/v1/library/m-1/keep", json!({})).await;

    assert_eq!(kept.assert_status(StatusCode::CREATED)["target_category"], "standard");
    app.simulate().await;
    let moves = "SELECT COUNT(*) FROM decisions WHERE action = 'move' AND superseded = 0";
    assert_eq!(app.count(moves).await, 0);
}
