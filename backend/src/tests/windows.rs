//! An Arr running on Windows writes `D:\Media\movies\standard\` where one on
//! Linux writes `/movies/standard`: a drive letter, backslashes, a separator
//! closing every root folder, and names Windows compares without their case.
//! A share is `\\nas\films`. Each journey here runs against such an Arr.

use super::TestApp;
use super::fake_arr::FakeArr;
use axum::http::StatusCode;
use serde_json::json;

const EVERY_GUARDRAIL: [&str; 4] = ["batch", "threshold", "capacity", "unreachable"];

/// A library synced from a Windows `kind`, with `standard` mapped to the
/// folder its titles sit in and `kids` to another one, the dry run off.
async fn synced_on_windows(kind: &str, arr: &FakeArr) -> TestApp {
    let app = TestApp::synced_from(kind, arr).await;
    app.execute(&[
        "INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-standard', 'standard')",
        "INSERT OR IGNORE INTO categories (id, name) VALUES ('cat-kids', 'kids')",
    ])
    .await;
    app.store_setting("default_category", "standard").await;
    app.store_setting("global_dry_run", "false").await;
    app
}

async fn map(app: &TestApp, folder: &str, category: &str) {
    app.put(&format!("/api/v1/root-folders/{folder}/category"), json!({ "category": category }))
        .await
        .assert_ok();
}

/// The one move the last simulation proposed.
async fn proposed_move(app: &TestApp) -> String {
    let simulation = app.simulate().await;
    sqlx::query_scalar(
        "SELECT id FROM decisions WHERE simulation_id = ? AND action = 'move' AND superseded = 0",
    )
    .bind(&simulation)
    .fetch_one(&app.state.pool)
    .await
    .expect("one proposed move")
}

/// Where the last move asked the Arr to put a title: Radarr's editor names
/// the root folder, Sonarr's update the series' own folder. A command the
/// move sends after it names neither.
fn last_destination(arr: &FakeArr) -> String {
    let recorded = arr.recorded();
    recorded
        .writes
        .iter()
        .rev()
        .find_map(|write| {
            write["rootFolderPath"]
                .as_str()
                .filter(|_| write.get("movieIds").is_some())
                .or_else(|| write["path"].as_str())
        })
        .expect("a move")
        .to_string()
}

#[tokio::test]
async fn a_title_already_in_its_windows_folder_is_not_moved() {
    let arr = FakeArr::on_windows().await;
    let app = synced_on_windows("radarr", &arr).await;
    map(&app, "rf-inst-1-1", "standard").await;

    let moves =
        app.count("SELECT COUNT(*) FROM decisions WHERE action = 'move' AND superseded = 0").await;
    assert_eq!(moves, 0, "nothing was simulated yet");
    app.simulate().await;
    let moves =
        app.count("SELECT COUNT(*) FROM decisions WHERE action = 'move' AND superseded = 0").await;
    assert_eq!(moves, 0, "D:\\Media\\movies\\standard and its root folder are one folder");
}

#[tokio::test]
async fn a_move_on_a_windows_radarr_names_its_folder_and_can_be_put_back() {
    let arr = FakeArr::on_windows().await;
    let app = synced_on_windows("radarr", &arr).await;
    map(&app, "rf-inst-1-1", "standard").await;
    map(&app, "rf-inst-1-3", "kids").await;
    app.seed_rule_on(json!({ "type": "genre_contains", "value": ["Family"] })).await;
    app.execute(&["UPDATE rules SET target_category = 'kids'"]).await;

    let decision = proposed_move(&app).await;
    let applied = app
        .post(
            "/api/v1/decisions/apply",
            json!({ "decision_ids": [decision], "move_files": false, "confirm": EVERY_GUARDRAIL }),
        )
        .await;
    assert_eq!(applied.assert_ok()["applied"], 1, "{:?}", applied.json);
    assert_eq!(last_destination(&arr), "D:\\Media\\movies\\kids\\");

    let reverted = app
        .post(
            "/api/v1/decisions/revert",
            json!({ "decision_ids": [decision], "move_files": false, "confirm": EVERY_GUARDRAIL }),
        )
        .await;
    assert_eq!(reverted.assert_ok()["applied"], 1, "{:?}", reverted.json);
    assert!(
        last_destination(&arr).trim_end_matches('\\') == "D:\\Media\\movies\\standard",
        "put back where it was: {}",
        last_destination(&arr)
    );
}

#[tokio::test]
async fn a_series_on_a_windows_sonarr_moves_and_comes_back() {
    let arr = FakeArr::on_windows().await;
    arr.report_root_folder(json!({
        "id": 4, "path": "D:\\Media\\tv\\standard\\", "freeSpace": 8192, "accessible": true
    }));
    arr.report_root_folder(json!({
        "id": 5, "path": "D:\\Media\\tv\\anime\\", "freeSpace": 8192, "accessible": true
    }));
    let app = synced_on_windows("sonarr", &arr).await;
    map(&app, "rf-inst-1-4", "standard").await;
    map(&app, "rf-inst-1-5", "kids").await;
    app.seed_rule_on(json!({ "type": "genre_contains", "value": ["Action"] })).await;
    app.execute(&["UPDATE rules SET target_category = 'kids'"]).await;

    let decision = proposed_move(&app).await;
    app.post(
        "/api/v1/decisions/apply",
        json!({ "decision_ids": [decision], "move_files": false, "confirm": EVERY_GUARDRAIL }),
    )
    .await
    .assert_ok();
    assert_eq!(last_destination(&arr), "D:\\Media\\tv\\anime\\Cowboy Bebop (1998)");

    let reverted = app
        .post(
            "/api/v1/decisions/revert",
            json!({ "decision_ids": [decision], "move_files": false, "confirm": EVERY_GUARDRAIL }),
        )
        .await;
    assert_eq!(reverted.assert_ok()["applied"], 1, "{:?}", reverted.json);
    assert_eq!(last_destination(&arr), "D:\\Media\\tv\\standard\\Cowboy Bebop (1998)");
}

#[tokio::test]
async fn a_rule_names_a_windows_folder_in_any_case_and_with_either_separator() {
    let arr = FakeArr::on_windows().await;
    let app = synced_on_windows("radarr", &arr).await;
    app.seed_rule_on(json!({ "type": "current_root_folder", "value": "d:/media/MOVIES/standard" }))
        .await;
    assert_eq!(app.decided_category().await, "anime", "the same folder as Windows reads it");

    let app = synced_on_windows("radarr", &arr).await;
    app.seed_rule_on(json!({
        "type": "current_root_folder_starts_with", "value": "D:/MEDIA/movies"
    }))
    .await;
    assert_eq!(app.decided_category().await, "anime", "a folder under the one named");
}

#[tokio::test]
async fn a_destination_declared_on_windows_is_kept_and_inherits_its_folder() {
    let arr = FakeArr::on_windows().await;
    let app = synced_on_windows("radarr", &arr).await;

    let declared = app
        .post(
            "/api/v1/root-folders",
            json!({ "instance_id": "inst-1", "path": "d:/media/movies/anime/films/" }),
        )
        .await;
    declared.assert_ok();
    let (path, free, accessible): (String, Option<i64>, bool) = sqlx::query_as(
        "SELECT path, free_space, accessible FROM root_folders WHERE origin = 'declared'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(path, "d:\\media\\movies\\anime\\films", "written as Windows writes it");
    assert_eq!(free, Some(2048), "the free space of D:\\Media\\movies\\anime");
    assert!(!accessible, "and its silence");

    let again = app
        .post(
            "/api/v1/root-folders",
            json!({ "instance_id": "inst-1", "path": "D:\\MEDIA\\Movies\\Anime\\Films" }),
        )
        .await;
    again.assert_status(StatusCode::CONFLICT);

    let share = app
        .post(
            "/api/v1/root-folders",
            json!({ "instance_id": "inst-1", "path": "\\\\nas\\films\\4k" }),
        )
        .await;
    share.assert_ok();

    for relative in ["movies\\4k", "D:movies", "\\\\nas"] {
        let refused = app
            .post("/api/v1/root-folders", json!({ "instance_id": "inst-1", "path": relative }))
            .await;
        refused.assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn a_windows_title_is_placed_in_the_folder_it_sits_in() {
    let arr = FakeArr::on_windows().await;
    let app = synced_on_windows("radarr", &arr).await;
    map(&app, "rf-inst-1-1", "standard").await;

    let answer = app.get("/api/v1/route?type=movie&tmdb=8392").await;
    let placed = &answer.assert_ok()["answers"][0];
    assert_eq!(placed["action"], "none", "{placed}");
    assert_eq!(placed["root_folder"]["path"], "D:\\Media\\movies\\standard\\", "{placed}");
}
