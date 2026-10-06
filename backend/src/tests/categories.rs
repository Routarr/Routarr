//! Renaming a category, and the six places its name lives.
//!
//! Categories are joined by value with no foreign key, so nothing in the
//! database carries a rename along. The transaction in `api::categories` is
//! the only thing that does. These tests are the counterweight: they check the
//! result rather than the statements, so a seventh place to update fails here
//! instead of leaving one screen quietly pointing at a name nobody holds.

use axum::http::StatusCode;
use sqlx::AssertSqlSafe;

use crate::services::routing::{self, SimulationOptions};

use super::TestApp;

/// Seed a library where the `anime` category is referenced from every table
/// that can hold one: a rule, a root-folder mapping, an override, a decision
/// produced by an actual simulation, and a pinned expectation.
async fn seed_every_reference(app: &TestApp) {
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, reason)
         SELECT 'ovr-1', id, 'anime', 'test' FROM media LIMIT 1",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();

    // A pinned expectation names a category too, and its snapshot columns are
    // deliberately opaque JSON, so only `expected_category` has to follow a
    // rename.
    sqlx::query(
        "INSERT INTO rule_tests (id, name, media_type, media_json, evaluated_at,
         expected_category)
         VALUES ('rt-1', 'Akira stays', 'movie', '{}', '2026-01-01 00:00:00', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
}

/// Every column in the schema whose name says it holds a category.
///
/// Read from the database rather than listed here on purpose: a migration that
/// adds one is exactly the change these tests exist to catch.
async fn category_columns(pool: &sqlx::SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT m.name, p.name
         FROM sqlite_master m
         JOIN pragma_table_info(m.name) p
         WHERE m.type = 'table'
           AND m.name NOT LIKE 'sqlite_%'
           AND (p.name = 'category' OR p.name LIKE '%_category')",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn rows_holding(pool: &sqlx::SqlitePool, value: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (table, column) in category_columns(pool).await {
        // The identifiers come from the schema, not from anything a user sent.
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE {column} = ?"
        )))
        .bind(value)
        .fetch_one(pool)
        .await
        .unwrap();
        if count > 0 {
            found.push(format!("{table}.{column} ({count} row(s))"));
        }
    }
    found
}

#[tokio::test]
async fn a_rename_carries_every_reference_with_it() {
    let app = TestApp::new().await;
    seed_every_reference(&app).await;

    // The fixture has to actually exercise the thing, or this passes on an
    // empty database.
    let before = rows_holding(&app.state.pool, "anime").await;
    assert_eq!(
        before.len(),
        category_columns(&app.state.pool).await.len(),
        "the fixture should reference 'anime' from every category column, got {before:?}"
    );

    let body = app.put("/api/v1/categories/cat-anime", serde_json::json!({ "name": "japanese" }));
    assert_eq!(body.await.assert_ok()["name"], "japanese");

    let left = rows_holding(&app.state.pool, "anime").await;
    assert!(left.is_empty(), "these still hold the old name: {left:?}");

    let moved = rows_holding(&app.state.pool, "japanese").await;
    assert_eq!(moved.len(), before.len(), "not every reference followed: {moved:?}");

    let name: String = sqlx::query_scalar("SELECT name FROM categories WHERE id = 'cat-anime'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(name, "japanese");
}

#[tokio::test]
async fn renaming_the_default_category_moves_the_setting_with_it() {
    let app = TestApp::new().await;

    // The default is the one the setting names, and nothing else states it.
    let id: String = sqlx::query_scalar(
        "SELECT id FROM categories
         WHERE name = (SELECT value FROM settings WHERE key = 'default_category')",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    app.put(&format!("/api/v1/categories/{id}"), serde_json::json!({ "name": "library" }))
        .await
        .assert_ok();

    // Left behind, this setting names a category that no longer exists, and
    // `settings::validate` refuses it, so the next save of any setting fails.
    let setting: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(setting, "library");
}

#[tokio::test]
async fn a_rename_is_held_to_the_rules_a_creation_is() {
    let app = TestApp::new().await;

    for bad in ["", "  ", "with space", "slash/es", "quo'te"] {
        let response =
            app.put("/api/v1/categories/cat-standard", serde_json::json!({ "name": bad })).await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }

    // And the normalisation is the same one, not a second implementation.
    app.put("/api/v1/categories/cat-standard", serde_json::json!({ "name": "  MiXeD  " }))
        .await
        .assert_ok();
    let name: String = sqlx::query_scalar("SELECT name FROM categories WHERE id = 'cat-standard'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(name, "mixed");
}

/// A category is named in the reader's language, whatever its script: the
/// hint says letters in every language, and the guide suggests `børn`, `çocuk`
/// and `παιδικά`.
#[tokio::test]
async fn a_category_is_named_in_any_script() {
    let app = TestApp::new().await;

    for (typed, stored) in [
        ("Séries", "séries"),
        ("Детские", "детские"),
        ("ΠΑΙΔΙΚΆ", "παιδικά"),
        ("Børn", "børn"),
        ("İzmir", "i\u{307}zmir"),
        ("アニメ", "アニメ"),
        ("أطفال", "أطفال"),
    ] {
        let created = app.post("/api/v1/categories", serde_json::json!({ "name": typed })).await;
        assert_eq!(created.assert_ok()["name"], stored, "{typed}");
    }
}

/// A refusal is read under the field the name was typed in, in the language
/// the interface speaks.
#[tokio::test]
async fn a_refused_category_name_is_said_in_the_interface_language() {
    let app = TestApp::new().await;
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "ui_language": "fr" } }))
        .await
        .assert_ok();

    for (typed, said) in [
        ("with space", "Un nom de catégorie ne contient que des lettres"),
        ("", "Une catégorie a besoin d'un nom."),
        (&"x".repeat(65), "Un nom de catégorie compte au plus 64 caractères."),
    ] {
        let refused = app.post("/api/v1/categories", serde_json::json!({ "name": typed })).await;
        refused.assert_status(StatusCode::BAD_REQUEST);
        let message = refused.json["message"].as_str().unwrap_or_default();
        assert!(message.starts_with(said), "{typed:?} answered {message:?}");
    }
}

#[tokio::test]
async fn renaming_onto_a_name_already_taken_is_refused() {
    let app = TestApp::new().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await.assert_ok();

    app.put("/api/v1/categories/cat-standard", serde_json::json!({ "name": "kids" }))
        .await
        .assert_status(StatusCode::CONFLICT);

    // Nothing moved.
    let still: String = sqlx::query_scalar("SELECT name FROM categories WHERE id = 'cat-standard'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(still, "standard");
}

#[tokio::test]
async fn renaming_a_category_to_its_own_name_changes_nothing() {
    let app = TestApp::new().await;
    app.put("/api/v1/categories/cat-standard", serde_json::json!({ "name": "standard" }))
        .await
        .assert_ok();
}

#[tokio::test]
async fn renaming_an_unknown_category_is_a_not_found() {
    let app = TestApp::new().await;
    app.put("/api/v1/categories/cat-nope", serde_json::json!({ "name": "whatever" }))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// --------------------------------------------------- the default category
//
// It is stated once, in the `default_category` setting the engine falls back
// to. A second statement, a flag on the row read by the badge and the delete
// guard, would be kept in step by nothing: the interface would name one
// category while the engine used another, and the guard would leave the real
// fallback deletable. Every unmatched item would then route to a category that
// does not exist, has no root folder, and is silently skipped.

#[tokio::test]
async fn the_badge_and_the_engine_name_the_same_default() {
    let app = TestApp::new().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await.assert_ok();
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "default_category": "kids" } }))
        .await
        .assert_ok();

    let listed = app.get("/api/v1/categories").await;
    let categories = listed.assert_ok().as_array().unwrap().clone();
    let flagged: Vec<&str> = categories
        .iter()
        .filter(|c| c["is_default"] == serde_json::json!(true))
        .map(|c| c["name"].as_str().unwrap())
        .collect();

    assert_eq!(flagged, vec!["kids"], "the badge should follow the setting the engine reads");
}

#[tokio::test]
async fn the_category_the_engine_falls_back_to_cannot_be_deleted() {
    let app = TestApp::new().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await.assert_ok();
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "default_category": "kids" } }))
        .await
        .assert_ok();

    let id: String = sqlx::query_scalar("SELECT id FROM categories WHERE name = 'kids'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    app.delete(&format!("/api/v1/categories/{id}")).await.assert_status(StatusCode::BAD_REQUEST);

    // And the one that is merely a category, not the fallback, is deletable:
    // the other half of the same claim.
    app.delete("/api/v1/categories/cat-standard").await.assert_ok();
}

#[tokio::test]
async fn creating_a_category_as_the_default_moves_the_setting() {
    let app = TestApp::new().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids", "is_default": true }))
        .await
        .assert_ok();

    let setting: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(setting, "kids");
}

#[tokio::test]
async fn creating_a_category_whose_name_is_taken_is_a_conflict() {
    let app = TestApp::new().await;
    app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await.assert_ok();

    // The uniqueness is the database's to enforce, with no check before the
    // write to race against, so what matters is that its refusal arrives as a
    // 409 rather than as a raw sqlx error and a 500.
    let response = app.post("/api/v1/categories", serde_json::json!({ "name": "kids" })).await;
    response.assert_status(StatusCode::CONFLICT);
    assert!(response.message().contains("already exists"), "got {}", response.message());
}

/// The fallback category is one fact: the engine, the categories screen and
/// the delete guard read the same answer. With no setting row, a guard holding
/// a fallback of its own compares a name against `""`, never matches, and
/// leaves deletable the category routing actually lands in.
///
/// Migration 001 seeds the setting and nothing removes it, so only a database
/// without the row shows the difference.
#[tokio::test]
async fn the_fallback_category_is_the_same_one_everywhere_even_with_no_setting() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("DELETE FROM settings WHERE key = 'default_category'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let fallback = crate::state::AppState::default_category(&app.state.pool).await.unwrap();
    assert_eq!(fallback, crate::state::DEFAULT_CATEGORY);

    // The categories screen marks it as the default, as the engine uses it.
    let listed = app.get("/api/v1/categories").await.assert_ok().clone();
    let marked: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|category| category["is_default"] == true)
        .filter_map(|category| category["name"].as_str())
        .collect();
    assert_eq!(marked, [fallback.as_str()], "{listed}");

    // The engine routes a title no rule matches there.
    let explained = app.get("/api/v1/media/m-1/explain").await.assert_ok().clone();
    assert_eq!(explained["target_category"], fallback.as_str(), "{explained}");

    // And the guard still protects it: deleting the category the engine falls
    // back to has to be refused, setting row or not. `standard` is seeded by
    // the schema, so it is already there to be deleted.
    let id: String = sqlx::query_scalar("SELECT id FROM categories WHERE name = ?")
        .bind(&fallback)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let response = app.delete(&format!("/api/v1/categories/{id}")).await;
    assert_eq!(
        response.status,
        axum::http::StatusCode::BAD_REQUEST,
        "the fallback category was deletable: {}",
        response.json
    );
}

/// The shipped default stands for a missing setting, never for one the
/// database failed to read: a run would send every title no rule matched
/// there, and a stored one would propose those moves.
#[tokio::test]
async fn a_fallback_category_that_cannot_be_read_fails_the_run() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["UPDATE settings SET value = x'00' WHERE key = 'default_category'"]).await;

    let run = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await;

    assert!(run.is_err(), "the run routed to the shipped default");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions").await, 0);
}
