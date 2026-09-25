//! Exporting the configuration and restoring it elsewhere.
//!
//! The value is in what cannot be regenerated: rules, folder mappings and above
//! all the manual overrides, which are human judgements no resync brings back.
//! The risk is in what must *not* travel — ids that mean nothing on another
//! machine, and API keys sealed with a master key that exists on one.

use super::TestApp;

/// A configured installation: two categories, a mapped folder, an override.
async fn configured() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query("INSERT INTO categories (id, name, description) VALUES ('c-k', 'kids', 'Family')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('batch_limit', '25')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    // A sealed setting, so `no_api_key_leaves_the_installation` has something
    // to prove. Without one the assertion on `enc:v1:` passed over a bundle
    // that could not have contained it — a test that reported a property it
    // never exercised.
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('tmdb_api_key', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(app.state.secrets.seal("tmdb-key-not-a-secret").unwrap())
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, reason, locked)
         VALUES ('o-1', 'm-1', 'kids', 'The rules read it as anime', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    app
}

async fn export(app: &TestApp) -> serde_json::Value {
    app.get("/api/v1/config/export").await.assert_ok().clone()
}

#[tokio::test]
async fn the_bundle_carries_what_cannot_be_regenerated() {
    let app = configured().await;

    let bundle = export(&app).await;

    assert_eq!(bundle["version"], 1);
    assert!(bundle["exported_at"].is_string());

    let categories: Vec<&str> = bundle["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(categories.contains(&"kids"));

    // The mapping is keyed by instance name and path, not by ids that mean
    // nothing on the machine restoring it.
    let mapping = &bundle["root_folders"].as_array().unwrap()[0];
    assert!(mapping["instance_name"].is_string());
    assert!(mapping["path"].as_str().unwrap().starts_with('/'));
    assert!(mapping["category"].is_string());

    // The override is keyed by external id, the only identity a media keeps.
    let over = &bundle["overrides"].as_array().unwrap()[0];
    assert_eq!(over["target_category"], "kids");
    assert_eq!(over["locked"], true);
    assert!(over["tmdb_id"].is_i64(), "matched on external id, not on our own");
}

#[tokio::test]
async fn no_api_key_leaves_the_installation() {
    let app = configured().await;

    let bundle = export(&app).await;
    let serialised = serde_json::to_string(&bundle).unwrap();

    // The fixture stores 'secret' as the instance key.
    assert!(!serialised.contains("secret"), "an API key must never appear in a bundle");
    assert!(!serialised.contains("enc:v1:"), "nor its ciphertext, which nothing else can open");
    assert_eq!(bundle["instances"].as_array().unwrap()[0]["has_api_key"], false);
}

#[tokio::test]
async fn nothing_reproducible_is_carried() {
    let app = configured().await;

    let bundle = export(&app).await;

    // The library, the decision history and the metadata cache all come back
    // from the Arrs and TMDb. Carrying them would bloat the bundle past being
    // readable for no gain.
    for absent in ["media", "decisions", "metadata_cache", "jobs", "execution_logs"] {
        assert!(bundle.get(absent).is_none(), "{absent} has no business in a bundle");
    }
}

#[tokio::test]
async fn a_bundle_restores_onto_an_empty_installation() {
    let source = configured().await;
    let bundle = export(&source).await;

    let target = TestApp::new().await;
    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    assert!(report["categories"].as_u64().unwrap() >= 2);
    assert_eq!(report["instances"], 1);

    let restored: Vec<String> = sqlx::query_scalar("SELECT name FROM categories ORDER BY name")
        .fetch_all(&target.state.pool)
        .await
        .unwrap();
    assert!(restored.contains(&"kids".to_string()));

    let limit: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'batch_limit'")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(limit, "25");
}

#[tokio::test]
async fn a_restored_instance_arrives_disabled_and_says_why() {
    let source = configured().await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    // Enabled without a key, it would fail on every scheduler tick and fill the
    // log with noise the user did not ask for.
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM instances")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert!(!enabled);

    // Named apart from what failed: the instance is restored, and the
    // interface shows `skipped` as the part of the import that did not work.
    let name = bundle["instances"][0]["name"].as_str().unwrap().trim();
    assert_eq!(report["needs_key"], serde_json::json!([name]));
    let skipped = report["skipped"].as_array().unwrap();
    assert!(
        !skipped.iter().any(|s| s.as_str().unwrap().contains("API key")),
        "a restored instance is not a refusal, got {skipped:?}"
    );
}

#[tokio::test]
async fn a_mapping_whose_folder_is_not_synced_yet_is_reported_not_dropped() {
    let source = configured().await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    // Root folders only exist once the instance has been synced, so on a fresh
    // install the mappings cannot land yet. Saying so is the whole point: a
    // backup that silently drops half its contents is worse than none.
    assert_eq!(report["root_folders"], 0);
    let skipped = report["skipped"].as_array().unwrap();
    assert!(
        skipped.iter().any(|s| s.as_str().unwrap().contains("sync that instance")),
        "got {skipped:?}"
    );
}

#[tokio::test]
async fn an_override_lands_once_its_media_exists() {
    let source = configured().await;
    let bundle = export(&source).await;

    // The destination has already synced the same film from its own Arr, so the
    // external id matches even though every local id differs.
    let target = TestApp::new().await;
    target.seed_library().await;

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["overrides"], 1);
    let (category, locked): (String, bool) =
        sqlx::query_as("SELECT target_category, locked FROM overrides")
            .fetch_one(&target.state.pool)
            .await
            .unwrap();
    assert_eq!(category, "kids");
    assert!(locked, "the lock is part of the human decision");
}

#[tokio::test]
async fn a_bundle_from_a_future_version_is_refused_rather_than_half_applied() {
    let app = TestApp::new().await;

    let refused = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": { "version": 99, "categories": [{ "name": "x" }] } }),
        )
        .await;

    assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
    // Asserted on the bundle's own content rather than on an empty table: a
    // fresh install already ships a default category.
    let leaked: Option<String> = sqlx::query_scalar("SELECT name FROM categories WHERE name = 'x'")
        .fetch_optional(&app.state.pool)
        .await
        .unwrap();
    assert!(leaked.is_none(), "nothing may be written from a bundle we cannot read");
}

#[tokio::test]
async fn a_setting_this_version_does_not_know_is_reported_not_stored() {
    let app = TestApp::new().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({
                "bundle": {
                    "version": 1,
                    "settings": [
                        { "key": "batch_limit", "value": "30" },
                        { "key": "from_a_later_routarr", "value": "42" }
                    ]
                }
            }),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["settings"], 1);
    // Stored, it would sit in the table for ever: unreachable through the API
    // and impossible to remove.
    let unknown: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'from_a_later_routarr'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert!(unknown.is_none());
    assert!(
        report["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().contains("from_a_later_routarr"))
    );
}

#[tokio::test]
async fn importing_twice_changes_nothing_the_second_time() {
    let source = configured().await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;
    target.seed_library().await;

    target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle.clone() })).await;
    let second = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    // Restoring is something people retry after fixing what was missing, so it
    // has to be safe to run again.
    assert_eq!(second["instances"], 0, "the existing instance is left alone");
    let instances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(instances, 1, "no duplicate");
    let overrides: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM overrides")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(overrides, 1);
}

// ------------------------------------------------- what a bundle may write

/// A bundle is the settings table's second writer. Everything `PUT /settings`
/// refuses, it has to refuse too, or the export becomes the way to store what
/// the API will not take.
#[tokio::test]
async fn a_bundle_cannot_write_a_setting_the_api_refuses() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1,
                "settings": [
                    // The fallback every unmatched item lands in. Named wrong,
                    // the delete guard compares against a category nobody holds
                    // and stops protecting the one routing actually uses.
                    { "key": "default_category", "value": "ghost" },
                    // The engine reads the Arr's own metadata at no cost; a
                    // list without it makes every metadata rule dead.
                    { "key": "metadata_providers", "value": "tmdb" },
                    { "key": "ui_theme", "value": "banana" },
                    { "key": "batch_limit", "value": "-5" },
                ],
                "categories": [], "instances": [], "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["settings"], 0, "nothing invalid was written");
    assert_eq!(report["skipped"].as_array().unwrap().len(), 4, "each one is reported");

    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert_ne!(stored.as_deref(), Some("ghost"), "the fallback category was overwritten");
}

/// The categories a bundle brings count as existing: validating against the
/// table alone would reject every bundle from another installation, which is
/// the only kind worth importing.
#[tokio::test]
async fn a_bundle_may_name_a_category_it_brings_with_it() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1,
                "settings": [{ "key": "default_category", "value": "arthouse" }],
                "categories": [{ "name": "arthouse", "description": null }],
                "instances": [], "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["settings"], 1);
    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, "arthouse");
}

/// `POST /instances` refuses an unknown type and a URL with no scheme. A bundle
/// that wrote them would leave a row the edit screen cannot save without
/// fixing, and that no adapter can build a client for.
#[tokio::test]
async fn a_bundle_cannot_write_an_instance_the_api_refuses() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1, "settings": [], "categories": [],
                "instances": [
                    { "name": "Plex", "instance_type": "plex",
                      "base_url": "http://plex:32400", "enabled": true,
                      "sync_interval_minutes": 60, "has_api_key": false },
                    { "name": "Naked", "instance_type": "radarr",
                      "base_url": "radarr:7878", "enabled": true,
                      "sync_interval_minutes": 60, "has_api_key": false },
                ],
                "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["instances"], 0);
    let written: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM instances WHERE name IN ('Plex', 'Naked')")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(written, 0, "an instance the API would refuse was written anyway");
}

/// `POST /instances` trims the name and clamps the interval to a day; a bundle
/// stored both raw. The scheduler clamps the interval when it reads it, so the
/// number on screen was not the one running — and a name with a stray space
/// was a second instance the existence check could not see.
#[tokio::test]
async fn an_imported_instance_is_stored_as_the_api_would_store_it() {
    let app = configured().await;

    let bundle = |name: &str, interval: i64| {
        serde_json::json!({ "bundle": {
            "version": 1, "settings": [], "categories": [],
            "instances": [
                { "name": name, "instance_type": "sonarr",
                  "base_url": "http://sonarr:8989/", "enabled": true,
                  "sync_interval_minutes": interval, "has_api_key": false },
            ],
            "root_folders": [], "overrides": []
        }})
    };

    for (raw, interval, stored_interval) in
        [("  Sonarr  ", 0, 1), ("Sonarr 4K", 100_000, 1440), ("Sonarr Anime", 60, 60)]
    {
        let report =
            app.post("/api/v1/config/import", bundle(raw, interval)).await.assert_ok().clone();
        assert_eq!(report["instances"], 1, "{raw:?}: {report}");
        // Found through `TRIM` so a raw name stored as it came is still seen,
        // and reported by the assertion rather than by a missing row.
        let (name, minutes): (String, i64) = sqlx::query_as(
            "SELECT name, sync_interval_minutes FROM instances WHERE TRIM(name) = ?",
        )
        .bind(raw.trim())
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!((name.as_str(), minutes), (raw.trim(), stored_interval), "{raw:?}");
    }

    // The existence check sees through the same whitespace the store trims.
    let again = app.post("/api/v1/config/import", bundle("Sonarr ", 60)).await.assert_ok().clone();
    assert_eq!(again["instances"], 0, "{again}");
    let sonarrs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances WHERE name = 'Sonarr'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sonarrs, 1, "a name with a stray space became a second instance");
}

/// A row written by an earlier build can still carry the space the import now
/// trims, and it must count as the instance it is: matched raw, the bundle's
/// trimmed name found nothing and a second `Radarr` was created beside it.
#[tokio::test]
async fn an_instance_stored_with_a_stray_space_still_counts_as_existing() {
    let app = TestApp::new().await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-legacy', 'Radarr ', 'radarr', 'http://radarr:7878', 'secret', 1, 'tok')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1, "settings": [], "categories": [],
                "instances": [
                    { "name": "Radarr", "instance_type": "radarr",
                      "base_url": "http://radarr:7878", "enabled": true,
                      "sync_interval_minutes": 60 }
                ],
                "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["instances"], 0, "{report}");
    let radarrs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM instances WHERE TRIM(name) = 'Radarr'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(radarrs, 1, "the legacy row was not seen and a second instance was created");
}

/// Categories are joined by value with no foreign key, so the database would
/// not stop a mapping naming one that does not exist — it would route nowhere,
/// and no warning could count it, there being no category row.
#[tokio::test]
async fn a_bundle_cannot_map_a_folder_to_a_category_that_does_not_exist() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1, "settings": [], "categories": [], "instances": [],
                "root_folders": [
                    { "instance_name": "Radarr", "path": "/movies/standard",
                      "category": "ghost" }
                ],
                "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["root_folders"], 0);
    assert!(
        report["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap_or_default().contains("does not exist")),
        "the reason has to reach the screen"
    );
}

/// The categories loop lowercases; the mapping loop did not. A bundle carrying
/// `Anime` created `anime` and pointed the folder at a name no row holds.
#[tokio::test]
async fn a_mapping_is_matched_to_a_category_whatever_its_case() {
    let app = configured().await;

    app.post(
        "/api/v1/config/import",
        serde_json::json!({ "bundle": {
            "version": 1, "settings": [],
            "categories": [{ "name": "Anime", "description": null }],
            "instances": [],
            "root_folders": [
                { "instance_name": "Radarr", "path": "/movies/standard",
                  "category": "Anime" }
            ],
            "overrides": []
        }}),
    )
    .await
    .assert_ok();

    let stored: Option<String> =
        sqlx::query_scalar("SELECT category FROM root_folders WHERE path = '/movies/standard'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("anime"), "the mapping names a category that exists");
}

/// `POST /categories` refuses a name with a space, an ampersand or five hundred
/// characters, because it reaches paths, rule payloads and query strings. A
/// bundle wrote one anyway — and then it could never be corrected, since
/// `rename` runs the check the import skipped.
#[tokio::test]
async fn a_bundle_cannot_create_a_category_the_api_would_refuse() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1,
                "settings": [],
                "categories": [
                    { "name": "Kids & Family", "description": null },
                    { "name": "a".repeat(300), "description": null },
                    { "name": "documentaries", "description": null },
                ],
                "instances": [], "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["categories"], 1, "only the sound one was created");
    let written: Vec<String> = sqlx::query_scalar("SELECT name FROM categories ORDER BY name")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert!(written.contains(&"documentaries".to_string()));
    assert!(!written.iter().any(|n| n.contains(' ') || n.len() > 64), "got {written:?}");
}

/// And a refused name is not something `default_category` may then point at.
#[tokio::test]
async fn a_category_the_bundle_could_not_create_is_not_a_valid_default() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1,
                "settings": [{ "key": "default_category", "value": "kids & family" }],
                "categories": [{ "name": "Kids & Family", "description": null }],
                "instances": [], "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["settings"], 0, "the fallback was pointed at a name nothing holds");
    assert_eq!(report["categories"], 0);
}

/// An override short-circuits the engine entirely, so one naming a category
/// this installation does not have routes its item nowhere — and did so without
/// appearing in `skipped`, which is the one place a partial restore is visible.
#[tokio::test]
async fn a_bundle_cannot_pin_media_to_a_category_that_does_not_exist() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1, "settings": [], "categories": [], "instances": [],
                "root_folders": [],
                "overrides": [{
                    "media_title": "My Neighbor Totoro", "media_type": "movie",
                    "tmdb_id": 8392, "tvdb_id": null,
                    "target_category": "ghost", "reason": "imported", "locked": true
                }]
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["overrides"], 0);
    assert!(
        report["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap_or_default().contains("does not exist")),
        "the reason has to reach the screen: {}",
        report["skipped"]
    );
}

/// The gate validates the trimmed value, so binding the raw one let `"kids "`
/// pass a check that `"kids"` had answered and land as a name no category holds.
#[tokio::test]
async fn a_setting_is_stored_as_it_was_validated() {
    let app = configured().await;

    app.post(
        "/api/v1/config/import",
        serde_json::json!({ "bundle": {
            "version": 1,
            "settings": [{ "key": "default_category", "value": "  kids  " }],
            "categories": [], "instances": [], "root_folders": [], "overrides": []
        }}),
    )
    .await
    .assert_ok();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, "kids", "stored with the whitespace the check had removed");
}
