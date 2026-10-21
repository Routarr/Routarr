//! Categories suggested from folder names: what each unmapped folder is offered, what is never
//! offered, and a dismissal kept through syncs. The fake Radarr holds `/movies/standard`,
//! `/movies/anime` and `/movies/kids`.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::TestApp;
use super::fake_arr::FakeArr;

const SUGGESTIONS: &str = "/api/v1/root-folders/suggestions";

/// The suggestions, by path.
async fn offered(app: &TestApp) -> std::collections::BTreeMap<String, Value> {
    let answer = app.get(SUGGESTIONS).await.assert_ok().clone();
    let list = answer.as_array().expect("a list").clone();
    list.into_iter().map(|each| (each["path"].as_str().unwrap().to_owned(), each)).collect()
}

async fn folder_id(app: &TestApp, path: &str) -> String {
    sqlx::query_scalar("SELECT id FROM root_folders WHERE path = ?")
        .bind(path)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn create_category(app: &TestApp, name: &str) {
    app.post("/api/v1/categories", json!({ "name": name }))
        .await
        .assert_status(StatusCode::CREATED);
}

async fn sync(app: &TestApp) {
    let attribution = crate::jobs::Attribution::manual(None);
    crate::services::sync::sync_instance(&app.state, "inst-1", &attribution).await.unwrap();
}

#[tokio::test]
async fn each_unmapped_folder_is_offered_the_category_its_name_says() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    let offered = offered(&app).await;
    assert_eq!(offered.len(), 3, "{offered:?}");
    let standard = &offered["/movies/standard"];
    assert_eq!(standard["category"], "standard");
    assert_eq!(standard["reason"], "Named like the category standard.");
    let anime = &offered["/movies/anime"];
    assert_eq!(
        (&anime["category"], &anime["reason"]),
        (&json!("anime"), &json!("The folder name says anime."))
    );
    assert_eq!(anime["template"], json!({ "id": "anime", "name": "Anime" }));
    let kids = &offered["/movies/kids"];
    assert_eq!((&kids["category"], &kids["choices"]), (&json!("kids"), &json!(["kids"])));
    assert_eq!(kids["template"]["id"], "children");
    assert_eq!(kids["dismissed"], false);
    assert_eq!(kids["instance_id"], "inst-1");
}

/// A mapped folder, and every folder of a disabled instance, is offered nothing: the person
/// already said, or the instance routes nothing.
#[tokio::test]
async fn a_mapped_folder_or_a_disabled_instance_is_offered_nothing() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    let kids = folder_id(&app, "/movies/kids").await;
    create_category(&app, "family").await;
    app.put(&format!("/api/v1/root-folders/{kids}/category"), json!({ "category": "family" }))
        .await
        .assert_ok();

    let offered = offered(&app).await;
    assert!(!offered.contains_key("/movies/kids"), "{offered:?}");
    assert!(offered.contains_key("/movies/anime"), "the control: an unmapped folder is offered");

    app.execute(&["UPDATE instances SET enabled = 0"]).await;
    assert_eq!(app.get(SUGGESTIONS).await.assert_ok(), &json!([]));
}

/// A category the instance already leads to is never offered again: mapped, a second folder
/// would be refused. Two unmapped folders saying one category leave the choice to the person.
#[tokio::test]
async fn a_category_is_offered_to_one_folder_of_an_instance_at_most() {
    let arr = FakeArr::start().await;
    arr.report_root_folder(
        json!({ "id": 4, "path": "/movies/animes", "freeSpace": 1, "accessible": true }),
    );
    arr.report_root_folder(
        json!({ "id": 5, "path": "/movies/KIDS 2", "freeSpace": 1, "accessible": true }),
    );
    let app = TestApp::synced_from("radarr", &arr).await;
    let anime = folder_id(&app, "/movies/anime").await;
    create_category(&app, "anime").await;
    app.put(&format!("/api/v1/root-folders/{anime}/category"), json!({ "category": "anime" }))
        .await
        .assert_ok();

    let offered = offered(&app).await;
    assert!(!offered.contains_key("/movies/animes"), "{offered:?}");
    for path in ["/movies/kids", "/movies/KIDS 2"] {
        let shared = &offered[path];
        assert_eq!(shared["category"], Value::Null, "{path}");
        assert_eq!(shared["choices"], json!(["kids"]), "{path}");
        let reason = shared["reason"].as_str().unwrap();
        assert!(reason.starts_with("Another folder of Fake radarr says kids too"), "{reason}");
    }
}

#[tokio::test]
async fn a_template_is_offered_until_the_library_holds_its_rules() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.execute(&[
        "INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime')",
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category,
                            match_mode, template)
         VALUES ('r-1', 'Anime series', 10, 1, 'series', '[]', 'anime', 'any', 'anime@1')",
    ])
    .await;

    let offered = offered(&app).await;
    assert_eq!(offered["/movies/anime"]["template"], Value::Null);
    assert_eq!(offered["/movies/kids"]["template"]["id"], "children", "the control");
}

/// Dismissed, a suggestion is still listed, marked, so the screen can show it again on demand,
/// and it stays dismissed through a sync. A folder the Arr stops reporting takes it along.
#[tokio::test]
async fn a_dismissal_holds_through_a_sync_and_goes_with_its_folder() {
    let arr = FakeArr::start().await;
    arr.report_root_folder(
        json!({ "id": 4, "path": "/movies/docs", "freeSpace": 1, "accessible": true }),
    );
    let app = TestApp::synced_from("radarr", &arr).await;
    let docs = folder_id(&app, "/movies/docs").await;
    let dismissal = format!("/api/v1/root-folders/{docs}/dismissal");

    assert_eq!(app.post(&dismissal, json!({})).await.assert_ok(), &json!({ "dismissed": true }));
    sync(&app).await;
    let offered = offered(&app).await;
    assert_eq!(offered["/movies/docs"]["dismissed"], true);
    assert_eq!(offered["/movies/kids"]["dismissed"], false, "the control");

    app.delete(&dismissal).await.assert_ok();
    assert!(!dismissed(&app, "/movies/docs").await);

    app.post(&dismissal, json!({})).await.assert_ok();
    arr.withdraw_reported_root_folders();
    // A sync tells the folders it did not see by a stamp to the second: the last one is older.
    app.execute(&["UPDATE root_folders SET last_synced_at = datetime('now', '-1 minute')"]).await;
    sync(&app).await;
    let left = app.count("SELECT COUNT(*) FROM folder_suggestion_dismissals").await;
    assert_eq!(left, 0, "a dismissal outlived its folder");

    let missing = app.post("/api/v1/root-folders/nope/dismissal", json!({})).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

async fn dismissed(app: &TestApp, path: &str) -> bool {
    offered(app).await[path]["dismissed"].as_bool().unwrap()
}
