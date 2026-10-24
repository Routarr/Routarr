//! Exporting the configuration and restoring it elsewhere. A bundle carries what no resync
//! brings back (rules, folder mappings, manual overrides) and never ids that mean nothing on
//! another machine, nor API keys sealed with a master key that exists on one.

use super::TestApp;

/// A configured installation: two categories, a mapped folder, an override, a rule, a rule test.
async fn configured() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query("INSERT INTO categories (id, name, description) VALUES ('c-k', 'kids', 'Family')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    app.store_setting("batch_limit", "25").await;

    // A sealed setting, without which the `enc:v` assertion of
    // `no_api_key_leaves_the_installation` would pass over a bundle that could not hold one.
    app.store_setting("tmdb_api_key", &app.state.secrets.seal("tmdb-key-not-a-secret").unwrap())
        .await;
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, reason)
         VALUES ('o-1', 'm-1', 'kids', 'The rules read it as anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let pinned = serde_json::json!({
        "name": "Totoro is for kids", "media_id": "m-1", "expected_category": "kids",
    });
    app.post("/api/v1/rule-tests", pinned).await.assert_status(axum::http::StatusCode::CREATED);

    app
}

async fn export(app: &TestApp) -> serde_json::Value {
    app.get("/api/v1/config/export").await.assert_ok().clone()
}

#[tokio::test]
async fn the_bundle_carries_what_cannot_be_regenerated() {
    let app = configured().await;

    let bundle = export(&app).await;

    assert_eq!(bundle["version"], 2);
    assert!(bundle["exported_at"].is_string());

    // A rule test holds automatic application while it fails, and no resync brings it back.
    let test = &bundle["rule_tests"][0];
    assert_eq!(test["name"], "Totoro is for kids");
    assert_eq!(test["expected_category"], "kids");
    assert_eq!(test["instance_name"], "Radarr", "its title's instance travels by name");
    assert_eq!(test["media"]["title"], "My Neighbor Totoro");
    assert!(test["media"].get("instance_id").is_none(), "an id of this installation travelled");

    let categories: Vec<&str> = bundle["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(categories.contains(&"kids"));

    // Keyed by instance name and path, not by ids that mean nothing on the restoring machine.
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
    assert!(!serialised.contains("enc:v"), "nor its ciphertext, which nothing else can open");
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

/// A bundle edited by hand, or exported before sealing, carries a credential in the clear, which
/// is never stored from a shared file.
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

/// Stored, it would sit for ever, unreachable through the API and impossible to remove.
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

    // The library, the decisions and the cache come back from the Arrs and the sources, and
    // would bloat the bundle past being readable.
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
    assert_eq!(report["rule_tests"], 1, "{report}");
    let restored: (bool, String) = sqlx::query_as(
        "SELECT json_extract(t.media_json, '$.instance_id') = i.id, t.expected_category
           FROM rule_tests t, instances i",
    )
    .fetch_one(&target.state.pool)
    .await
    .unwrap();
    assert_eq!(restored, (true, "kids".to_string()), "the test is not on the copy's Radarr");

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

/// An instance kept to proposals stays so on the copy, and a bundle carrying no switch leaves
/// the instance to automatic application, as an upgrade does.
#[tokio::test]
async fn an_instance_kept_to_proposals_is_kept_so_on_the_copy() {
    let source = configured().await;
    source.execute(&["UPDATE instances SET auto_apply = 0"]).await;
    let mut bundle = export(&source).await;
    assert_eq!(bundle["instances"][0]["auto_apply"], false);

    let restored = |bundle: serde_json::Value| async move {
        let target = TestApp::new().await;
        target
            .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
            .await
            .assert_ok();
        sqlx::query_scalar::<_, bool>("SELECT auto_apply FROM instances")
            .fetch_one(&target.state.pool)
            .await
            .unwrap()
    };
    assert!(!restored(bundle.clone()).await);
    bundle["instances"][0].as_object_mut().unwrap().remove("auto_apply");
    assert!(restored(bundle).await);
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

    // Enabled without a key, it would fail on every scheduler tick and fill the log.
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM instances")
        .fetch_one(&target.state.pool)
        .await
        .unwrap();
    assert!(!enabled);

    // Named apart from `skipped`, which the interface shows as the part that did not work.
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

    // Root folders exist once the instance has synced, so the mappings cannot land yet, and a
    // backup that silently drops half its contents is worse than none.
    assert_eq!(report["root_folders"], 0);
    let skipped = report["skipped"].as_array().unwrap();
    assert!(
        skipped.iter().any(|s| s.as_str().unwrap().contains("sync that instance")),
        "got {skipped:?}"
    );
}

/// A destination declared in Routarr is the operator's, never brought back by a sync: the bundle
/// declares it again, with its category, on the instance of the same name.
#[tokio::test]
async fn a_declared_destination_comes_back_with_its_category() {
    let source = configured().await;
    source
        .execute(&["INSERT INTO root_folders (id, instance_id, path, accessible, category, origin)
                      VALUES ('rf-d', 'inst-1', '/movies/anime/kids', 1, 'kids', 'declared'),
                             ('rf-e', 'inst-1', '/movies/empty', 1, NULL, 'declared')"])
        .await;
    let bundle = export(&source).await;
    let target = TestApp::new().await;
    target.execute(&[crate::tests::AN_INSTANCE]).await;
    crate::api::instances::converge_names(&target.state.pool).await.unwrap();

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    let declared: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT path, category FROM root_folders
          WHERE origin = 'declared' AND instance_id = 'inst-1' ORDER BY path",
    )
    .fetch_all(&target.state.pool)
    .await
    .unwrap();
    assert_eq!(
        declared,
        [
            ("/movies/anime/kids".to_string(), Some("kids".to_string())),
            ("/movies/empty".into(), None)
        ]
    );
    assert_eq!(report["root_folders"], 2, "{report}");
    let skipped = report["skipped"].to_string();
    assert!(
        !skipped.contains("/movies/anime/kids") && !skipped.contains("/movies/empty"),
        "{skipped}"
    );
    // Only an Arr's own folders wait for a sync.
    assert!(skipped.contains("sync that instance"), "{skipped}");
}

/// A destination or a rule test naming an instance this installation does not hold is left out,
/// and the report says which and why.
#[tokio::test]
async fn what_names_an_instance_not_here_is_skipped_and_said() {
    let source = configured().await;
    source
        .execute(&["INSERT INTO root_folders (id, instance_id, path, accessible, origin)
                      VALUES ('rf-d', 'inst-1', '/movies/anime/kids', 1, 'declared')"])
        .await;
    let mut bundle = export(&source).await;
    bundle["instances"] = serde_json::json!([]);
    let target = TestApp::new().await;

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["rule_tests"], 0);
    let skipped = report["skipped"].to_string();
    for said in [
        "Rule test 'Totoro is for kids': no instance here is named 'Radarr'",
        "Destination '/movies/anime/kids': no instance here is named 'Radarr'",
    ] {
        assert!(skipped.contains(said), "{said} missing from {skipped}");
    }
    assert_eq!(target.count("SELECT COUNT(*) FROM rule_tests").await, 0);
}

/// One rule tells two instance names apart, case beyond ASCII and spaces aside: a bundle
/// naming the instance otherwise maps its folder, pins its title and scopes its rule there.
#[tokio::test]
async fn an_instance_name_in_another_case_names_the_same_instance_everywhere() {
    let target = TestApp::new().await;
    target.seed_library().await;
    target.execute(&["UPDATE instances SET name = 'Émissions 4K' WHERE id = 'inst-1'"]).await;
    let named = " émissions 4k";
    let bundle = serde_json::json!({
        "version": 1,
        "root_folders": [{ "instance_name": named, "path": "/movies/standard", "category": "anime" }],
        "overrides": [{
            "media_title": "My Neighbor Totoro", "instance_name": named, "media_type": "movie",
            "tmdb_id": 8392, "target_category": "anime"
        }],
        "rules": [{
            "name": "Scoped", "media_type": "movie", "target_category": "anime",
            "conditions": [{ "type": "genre_contains", "value": ["Animation"] }],
            "instance_names": [named]
        }]
    });

    let report = target
        .post("/api/v1/config/import", serde_json::json!({ "bundle": bundle }))
        .await
        .assert_ok()
        .clone();

    assert_eq!(
        (report["root_folders"].clone(), report["overrides"].clone(), report["rules"].clone()),
        (1.into(), 1.into(), 1.into()),
        "{report}"
    );
    let scope: Option<String> =
        sqlx::query_scalar("SELECT instance_ids FROM rules WHERE name = 'Scoped'")
            .fetch_one(&target.state.pool)
            .await
            .unwrap();
    assert_eq!(scope.as_deref(), Some(r#"["inst-1"]"#));
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

    assert_eq!(report.assert_ok()["rules"], 2, "{:?}", report.json);
    let rules = target.get("/api/v1/rules").await.assert_ok().clone();
    let rule = rules.as_array().unwrap().iter().find(|rule| rule["name"] == "Ghibli").unwrap();
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

    for (sent, replace, expected) in [(&bundle, false, 3), (&bundle, true, 2), (&no_rules, true, 1)]
    {
        let target = TestApp::new().await;
        target.seed_library().await;
        target.seed_anime_rule().await;
        // Named apart from the bundle's own anime rule, which an append would take as present.
        target.execute(&["UPDATE rules SET name = 'Anime here'"]).await;
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
/// exception, and an exception comes back on the copy it was pinned on, the
/// instance named whatever its case and spaces.
#[tokio::test]
async fn an_exception_comes_back_on_the_copy_it_was_pinned_on() {
    let source = configured().await;
    second_copy(&source).await;
    source.execute(&["UPDATE overrides SET media_id = 'm-2' WHERE id = 'o-1'"]).await;
    let mut bundle = export(&source).await;
    let named = &mut bundle["overrides"][0]["instance_name"];
    *named = serde_json::json!(format!(" {} ", named.as_str().unwrap().to_uppercase()));
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

    // The target synced the film from its own Arr: the external id matches, no local id does.
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

/// An imported exception is a pin as any other: it names who set it, the
/// importer, in place of who set the one it replaces, and withdraws the
/// title's pending proposals, which the pin now decides.
#[tokio::test]
async fn an_imported_exception_withdraws_the_titles_proposals_and_names_its_author() {
    let bundle = export(&configured().await).await;
    let target = TestApp::new().await;
    target.seed_library().await;
    target.seed_anime_rule().await;
    target.simulate().await;
    target
        .execute(&["INSERT INTO overrides (id, media_id, target_category, subject)
                    VALUES ('o-n8n', 'm-1', 'anime', 'n8n')"])
        .await;
    let pending = "SELECT COUNT(*) FROM decisions
                    WHERE media_id = 'm-1' AND status = 'pending' AND superseded = 0";
    assert_eq!(target.count(pending).await, 1, "no proposal for the pin to withdraw");

    target.post("/api/v1/config/import", serde_json::json!({ "bundle": bundle })).await.assert_ok();

    let (category, subject): (String, Option<String>) =
        sqlx::query_as("SELECT target_category, subject FROM overrides WHERE media_id = 'm-1'")
            .fetch_one(&target.state.pool)
            .await
            .unwrap();
    assert_eq!((category.as_str(), subject), ("kids", None), "the import kept the old author");
    assert_eq!(target.count(pending).await, 0, "the pin left the proposal it decides listed");
}

/// A retention a bundle lowers removes the archives above it at once, as a
/// save on the Settings screen does, not at the next backup.
#[tokio::test]
async fn an_imported_retention_prunes_the_archives_above_it() {
    let (app, dir) = super::backup::app_with_files("imported-retention").await;
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    for day in 1..=3 {
        let name = format!("routarr-backup-2026010{day}-000000.zip");
        std::fs::write(backups.join(name), b"an archive").unwrap();
    }

    let bundle = serde_json::json!({ "bundle": {
        "version": 1,
        "settings": [{ "key": "backup_retention_count", "value": "1" }]
    }});
    app.post("/api/v1/config/import", bundle).await.assert_ok();

    let left: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["routarr-backup-20260103-000000.zip"]);
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
    // On the bundle's own category: a fresh install already ships a default one.
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

    // People retry a restore after fixing what was missing, so it must be safe to run again.
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
    assert_eq!(second["rules"], 0, "{second}");
    assert_eq!(target.count("SELECT COUNT(*) FROM rules").await, 1, "the rule was added twice");
    assert_eq!(second["rule_tests"], 0, "{second}");
    assert_eq!(target.count("SELECT COUNT(*) FROM rule_tests").await, 1, "the test came twice");
}

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
                    // The fallback category: named wrong, the delete guard protects a
                    // name nobody holds instead of the one routing uses.
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
                    { "name": "Queried", "instance_type": "radarr",
                      "base_url": "http://radarr:7878/?x", "enabled": true,
                      "sync_interval_minutes": 60, "has_api_key": false },
                ],
                "root_folders": [], "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["instances"], 0);
    let written: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM instances WHERE name IN ('Plex', 'Naked', 'Queried')",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(written, 0, "an instance the API would refuse was written anyway");
}

/// A bundle stores an instance as `POST /instances` does, its name trimmed and its interval
/// clamped to a day: the scheduler clamps the interval it reads, so a raw one on screen is not
/// the one running, and a stray space makes a second instance the existence check cannot see.
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
        // Found through `TRIM`, so a name stored untrimmed fails the assertion, not the fetch.
        let (name, minutes): (String, i64) = sqlx::query_as(
            "SELECT name, sync_interval_minutes FROM instances WHERE TRIM(name) = ?",
        )
        .bind(raw.trim())
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!((name.as_str(), minutes), (raw.trim(), stored_interval), "{raw:?}");
    }

    // No key travels: the instance waits for one, stored sealed as every key is, and says so.
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT api_key FROM instances WHERE name LIKE 'Sonarr%'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|key| key.starts_with("enc:v2:")), "{keys:?}");
    let listed = app.get("/api/v1/instances").await.assert_ok().clone();
    let imported = listed.as_array().unwrap().iter().filter(|instance| {
        instance["name"].as_str().is_some_and(|name| name.starts_with("Sonarr"))
    });
    for instance in imported {
        assert_eq!(instance["api_key_set"], false, "{instance}");
        assert_eq!(instance["api_key_encrypted"], true, "{instance}");
        assert_eq!(instance["api_key_masked"], "", "{instance}");
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

/// Names differing only by case or spaces are one, as the unique index holds them. A bundle
/// naming an instance so names the one held here: it creates no second one, and its folders
/// and exceptions land on it.
#[tokio::test]
async fn a_bundle_names_an_instance_whatever_the_case_and_spaces() {
    let app = configured().await;

    let report = app
        .post(
            "/api/v1/config/import",
            serde_json::json!({ "bundle": {
                "version": 1, "settings": [],
                "categories": [{ "name": "kids", "description": null }],
                "instances": [
                    { "name": " radarr ", "instance_type": "radarr",
                      "base_url": "http://radarr:7878", "enabled": true,
                      "sync_interval_minutes": 60 }
                ],
                "root_folders": [
                    { "instance_name": " RADARR", "path": "/movies/standard", "category": "kids" }
                ],
                "overrides": []
            }}),
        )
        .await
        .assert_ok()
        .clone();

    assert_eq!(report["instances"], 0, "{report}");
    assert_eq!(report["root_folders"], 1, "{report}");
    let stored: Option<String> =
        sqlx::query_scalar("SELECT category FROM root_folders WHERE path = '/movies/standard'")
            .fetch_optional(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("kids"));
}

/// A bundle carries a category spread over two folders of an instance as it is: the set was
/// chosen where the bundle was made.
#[tokio::test]
async fn a_bundle_carries_a_category_spread_over_two_folders() {
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
    assert_eq!(mapped, ["/movies/anime", "/movies/standard"]);
    assert_eq!(report["root_folders"], 1, "{report}");
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

/// `POST /categories` refuses a name with a space, an ampersand or five hundred characters, as
/// it reaches paths, rule payloads and query strings. Written by a bundle, it could never be
/// corrected: `rename` runs the check such an import skips.
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

/// An override short-circuits the engine, so one naming a category this installation lacks
/// routes its item nowhere: it is refused and reported in `skipped`, where a partial restore
/// shows. The lock a bundle may hold on one is read and dropped.
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
