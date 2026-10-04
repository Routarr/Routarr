//! Exporting the configuration and restoring it elsewhere.
//!
//! The value is in what cannot be regenerated: rules, folder mappings and above
//! all the manual overrides, which are human judgements no resync brings back.
//! The risk is in what must *not* travel: ids that mean nothing on another
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
    app.store_setting("batch_limit", "25").await;

    // A sealed setting, so `no_api_key_leaves_the_installation` has something
    // to prove. Without one, the assertion on `enc:v1:` would pass over a bundle
    // that could not contain it, a test reporting a property it never
    // exercised.
    app.store_setting("tmdb_api_key", &app.state.secrets.seal("tmdb-key-not-a-secret").unwrap())
        .await;
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, reason)
         VALUES ('o-1', 'm-1', 'kids', 'The rules read it as anime')",
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

/// A webhook address lets whoever holds it post to the channel, and a bundle is
/// something people share.
#[tokio::test]
async fn an_export_leaves_the_notification_webhook_out() {
    let app = configured().await;
    let url = "https://discord.com/api/webhooks/123/hook-secret-8d2e";
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "notification_webhook_url": url } }),
    )
    .await
    .assert_ok();

    let bundle = export(&app).await;

    let serialised = serde_json::to_string(&bundle).unwrap();
    assert!(!serialised.contains("notification_webhook_url"), "the setting travelled");
    assert!(!serialised.contains("hook-secret"), "the address travelled");
}

/// A bundle exported before a credential was sealed, or edited by hand, carries
/// it in the clear: the notification address, or a source's key. The import
/// leaves every one out and says to set it again, rather than storing a
/// credential that arrived in a shared file.
#[tokio::test]
async fn a_bundle_carrying_a_credential_is_told_to_set_it_again() {
    let app = TestApp::new().await;
    let credentials = crate::services::settings::sealed_keys();
    assert!(credentials.contains(&"notification_webhook_url"), "{credentials:?}");
    assert!(credentials.contains(&"tmdb_api_key"), "{credentials:?}");
    let mut settings: Vec<serde_json::Value> = credentials
        .iter()
        .map(|key| serde_json::json!({ "key": key, "value": "https://discord.com/api/webhooks/1/abc" }))
        .collect();
    settings.push(serde_json::json!({ "key": "batch_limit", "value": "25" }));
    let bundle = serde_json::json!({ "bundle": { "version": 1, "settings": settings } });

    let report = app.post("/api/v1/config/import", bundle).await.assert_ok().clone();

    let skipped = report["skipped"].to_string();
    for key in credentials {
        let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(stored, None, "{key} was stored from the bundle");
        assert!(skipped.contains(key), "{key} not reported: {skipped}");
    }
    assert_eq!(report["settings"], 1, "the other setting was not restored");
}

/// A setting this release does not read, whether an older release exported it
/// or a later one did, is reported and not stored: stored, it would sit in the
/// table for ever, unreachable through the API and impossible to remove. The
/// rest of the bundle is restored.
#[tokio::test]
async fn a_bundle_setting_this_release_does_not_read_is_reported_not_stored() {
    let app = TestApp::new().await;
    // `move_files_default` is one an older release exported, and reads nothing.
    let unread = ["move_files_default", "tmdb_cache_ttl_days", "from_a_later_routarr"];
    let mut settings: Vec<serde_json::Value> =
        unread.iter().map(|key| serde_json::json!({ "key": key, "value": "14" })).collect();
    settings.push(serde_json::json!({ "key": "batch_limit", "value": "25" }));
    let bundle = serde_json::json!({ "bundle": { "version": 1, "settings": settings } });

    let report = app.post("/api/v1/config/import", bundle).await.assert_ok().clone();

    let skipped = report["skipped"].to_string();
    for key in unread {
        let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(stored, None, "{key} was stored from the bundle");
        assert!(skipped.contains(key), "{key} not reported: {skipped}");
    }
    assert_eq!(report["settings"], 1, "the other setting was not restored");
}

/// A proxy's password in an Arr's address stays behind, like the API key: the
/// restored instance needs both typed again.
#[tokio::test]
async fn an_export_writes_an_address_without_its_credentials() {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", "http://proxy-user:proxy-pass@radarr.lan:7878").await;

    let bundle = export(&app).await;

    assert_eq!(bundle["instances"][0]["base_url"], "http://radarr.lan:7878");
    assert!(!bundle.to_string().contains("proxy-"), "the credentials travelled");
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

/// A rule scoped to the configured installation's instance.
async fn with_a_scoped_rule(app: &TestApp) {
    let rule = serde_json::json!({
        "name": "Ghibli", "media_type": "movie", "target_category": "kids",
        "instance_ids": ["inst-1"], "exclusions": [{ "type": "genre_contains", "value": ["Horror"] }],
        "conditions": [{ "type": "keyword_contains", "value": ["studio ghibli"] }]
    });
    app.post("/api/v1/rules", rule).await.assert_ok();
}

/// The rules travel with the rest, their instance scope by name: on another
/// installation the instance has another id, and the rule follows it there.
#[tokio::test]
async fn the_configuration_brings_its_rules_back_scoped_to_the_same_instance() {
    let source = configured().await;
    with_a_scoped_rule(&source).await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;

    let report =
        target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle })).await;

    assert_eq!(report.assert_ok()["rules"], 1, "{:?}", report.json);
    let rules = target.get("/api/v1/rules").await.assert_ok().clone();
    let rule = &rules[0];
    assert_eq!(rule["name"], "Ghibli");
    assert_eq!(rule["exclusions"][0]["value"][0], "Horror");
    let instance: String = sqlx::query_scalar("SELECT id FROM instances WHERE name = 'Radarr'")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(rule["instance_ids"], serde_json::json!([instance]), "the scope was lost");
}

/// As the Rules screen does: the bundle's rules join the ones in place, or
/// replace them when asked, and a bundle with no rule replaces nothing.
#[tokio::test]
async fn the_configuration_rules_join_or_replace_the_rules_in_place() {
    let source = configured().await;
    with_a_scoped_rule(&source).await;
    let bundle = export(&source).await;
    let mut no_rules = bundle.clone();
    no_rules["rules"] = serde_json::json!([]);

    for (sent, replace, expected) in [(&bundle, false, 2), (&bundle, true, 1), (&no_rules, true, 1)]
    {
        let target = TestApp::new().await;
        target.seed_library().await;
        target.seed_anime_rule().await;
        let request = serde_json::json!({ "bundle": sent, "replace_rules": replace });
        target.post("/api/v1/config/import", request).await.assert_ok();
        let rules = target.count("SELECT COUNT(*) FROM rules").await;
        assert_eq!(
            rules,
            expected,
            "replace {replace}, {} rule(s) sent",
            sent["rules"].as_array().unwrap().len()
        );
    }
}

/// The import judges as the routes do: a mapping or an exception naming its
/// instance with a stray space still finds it, and a source the bundle enables
/// without its key here is refused, as `PUT /settings` refuses it.
#[tokio::test]
async fn the_import_judges_names_and_sources_as_the_routes_do() {
    let source = configured().await;
    let mut bundle = export(&source).await;
    bundle["root_folders"][0]["instance_name"] = serde_json::json!(" Radarr ");
    bundle["overrides"][0]["instance_name"] = serde_json::json!("Radarr ");
    let settings = bundle["settings"].as_array_mut().unwrap();
    settings.retain(|setting| setting["key"] != "metadata_providers");
    settings.push(serde_json::json!({ "key": "metadata_providers", "value": "arr,omdb" }));
    let target = TestApp::new().await;
    target.seed_library().await;
    target.execute(&["UPDATE root_folders SET category = NULL"]).await;

    let report =
        target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle })).await;

    let report = report.assert_ok().clone();
    assert!(report["root_folders"].as_i64().unwrap() > 0, "{report}");
    assert_eq!(report["overrides"], 1, "{report}");
    assert!(report["skipped"].to_string().contains("omdb"), "{report}");
    let providers: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'metadata_providers'")
            .fetch_optional(&target.state.pool)
            .await
            .unwrap();
    assert_eq!(providers.as_deref(), Some("arr"), "a source with no key was enabled");
}

/// The same film held by a second instance, as a 4K Radarr holds it.
async fn second_copy(app: &TestApp) {
    app.execute(&[
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-2', 'Radarr 4K', 'radarr', 'http://127.0.0.1:1', 'k', 1, 'tok-2')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id, monitored,
                            has_files)
         VALUES ('m-2', 'inst-2', 10, 'movie', 'My Neighbor Totoro', 1988, 8392, 1, 1)",
    ])
    .await;
}

/// Two instances holding one film hold two titles, each with its own
/// exception, and an exception comes back on the copy it was pinned on.
#[tokio::test]
async fn an_exception_comes_back_on_the_copy_it_was_pinned_on() {
    let source = configured().await;
    second_copy(&source).await;
    source.execute(&["UPDATE overrides SET media_id = 'm-2' WHERE id = 'o-1'"]).await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;
    target.seed_library().await;
    second_copy(&target).await;

    let report =
        target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle })).await;

    assert_eq!(report.assert_ok()["overrides"], 1);
    let pinned: Vec<String> = sqlx::query_scalar("SELECT media_id FROM overrides")
        .fetch_all(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(pinned, ["m-2"], "the exception landed on the other instance's copy");
}

/// A bundle naming no instance cannot say which copy was meant: every copy is
/// pinned, and the report says so.
#[tokio::test]
async fn an_exception_naming_no_instance_is_pinned_on_every_copy_and_said() {
    let source = configured().await;
    let mut bundle = export(&source).await;
    bundle["overrides"][0].as_object_mut().unwrap().remove("instance_name");
    let target = TestApp::new().await;
    target.seed_library().await;
    second_copy(&target).await;

    let report =
        target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle })).await;

    let report = report.assert_ok();
    assert_eq!(target.count("SELECT COUNT(*) FROM overrides").await, 2);
    assert!(report["skipped"].to_string().contains("every copy"), "{report}");
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
    let category: String = sqlx::query_scalar("SELECT target_category FROM overrides")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert_eq!(category, "kids");
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
                    // The engine reads the Arr's own metadata at no cost, and a
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

/// `POST /instances` trims the name and clamps the interval to a day, and a
/// bundle stores both the same way. The scheduler clamps the interval when it
/// reads it, so a raw number on screen would not be the one running, and a
/// name with a stray space would be a second instance the existence check
/// cannot see.
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

/// A row written by an earlier build can still carry the space the import
/// trims, and it must count as the instance it is: matched raw, the bundle's
/// trimmed name would find nothing and a second `Radarr` would be created
/// beside it.
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

/// One category, one folder per instance, as `PUT /root-folders/{id}/category`
/// insists: two folders answering to one category leave the target ambiguous,
/// and the simulation and the explanation each pick their own.
#[tokio::test]
async fn a_bundle_cannot_map_one_category_to_two_folders_of_an_instance() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": { "version": 1, "root_folders": [
                { "instance_name": "Radarr", "path": "/movies/standard", "category": "anime" }
            ] } }),
        )
        .await
        .assert_ok()
        .clone();

    let mapped: Vec<String> =
        sqlx::query_scalar("SELECT path FROM root_folders WHERE category = 'anime' ORDER BY path")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(mapped, ["/movies/anime"], "the category now answers for two folders");
    assert_eq!(report["root_folders"], 0);
    assert!(report["skipped"].to_string().contains("/movies/anime"), "{}", report["skipped"]);
}

/// Categories are joined by value with no foreign key, so the database would
/// not stop a mapping naming one that does not exist: it would route nowhere,
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

/// The categories loop lowercases, and so must the mapping loop, or a bundle
/// carrying `Kids` would point the folder at a name no row holds.
#[tokio::test]
async fn a_mapping_is_matched_to_a_category_whatever_its_case() {
    let app = configured().await;

    app.post(
        "/api/v1/config/import",
        serde_json::json!({ "bundle": {
            "version": 1, "settings": [],
            "categories": [{ "name": "Kids", "description": null }],
            "instances": [],
            "root_folders": [
                { "instance_name": "Radarr", "path": "/movies/standard",
                  "category": "Kids" }
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
    assert_eq!(stored.as_deref(), Some("kids"), "the mapping names a category that exists");
}

/// `POST /categories` refuses a name with a space, an ampersand or five hundred
/// characters, because it reaches paths, rule payloads and query strings. A
/// bundle that wrote one anyway would leave a name that could never be
/// corrected, since `rename` runs the check such an import skips.
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
/// this installation does not have would route its item nowhere. It is refused
/// and reported in `skipped`, which is the one place a partial restore is
/// visible.
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

/// The gate validates the trimmed value, so binding the raw one would let
/// `"kids "` pass a check that `"kids"` answered and land as a name no category
/// holds.
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
