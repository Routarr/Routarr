//! Metadata credentials saved from the interface.
//!
//! Settable from the screen that orders the sources, rather than from the
//! environment alone: the screen that names a missing key is the one that
//! should be able to supply it. Sealed exactly as an Arr key is, and an Arr key
//! is the *more* dangerous of the two, since it writes to the library while
//! these only read.

use super::TestApp;

async fn settings(app: &TestApp) -> serde_json::Value {
    app.get("/api/v1/settings").await.assert_ok().clone()
}

/// A secret written as one to the setting `key`: what a test looks for in a
/// row and an answer, and a value its own validation takes.
fn secret_for(key: &str) -> String {
    match key {
        "notification_webhook_url" => "https://discord.com/api/webhooks/1/secret-8f3a".into(),
        _ => format!("{key}-secret-8f3a"),
    }
}

/// The claim every credential setting rests on: what is stored is sealed,
/// and what is returned is nothing but whether one is set, through every
/// answer that speaks of the settings or the sources.
#[tokio::test]
async fn every_credential_setting_is_sealed_and_never_read_back() {
    let app = TestApp::new().await;
    app.list_tmdb().await;
    let keys = crate::services::settings::sealed_keys();
    for credential in [
        "tmdb_api_key",
        "omdb_api_key",
        "tvdb_api_key",
        "notification_webhook_url",
        "backup_passphrase",
    ] {
        assert!(keys.contains(&credential), "{credential} is not sealed: {keys:?}");
    }

    for key in keys {
        let secret = secret_for(key);
        // The passphrase has a route of its own, which reseals the archives.
        let saved = if key == crate::services::backup::PASSPHRASE_SETTING {
            let body = serde_json::json!({ "passphrase": secret });
            app.put("/api/v1/backups/passphrase", body).await.status
        } else {
            app.save_setting(key, &secret).await.status
        };
        assert!(saved.is_success(), "{key}: {saved}");

        let stored: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert!(stored.starts_with("enc:v2:"), "{key} stored in the clear: {stored}");
        assert_eq!(app.state.secrets.open(&stored).unwrap(), secret, "{key}");

        let body = settings(&app).await;
        assert!(body.get(key).is_none(), "{key} came back out");
        assert_eq!(body[format!("{key}_configured")], true, "the screen cannot tell {key} is set");
        for path in [
            "/api/v1/settings",
            "/api/v1/metadata/providers",
            "/api/v1/health?probe=false",
            "/api/v1/config/export",
        ] {
            let answer = app.text(path).await;
            assert!(!answer.contains("secret-8f3a"), "{key} appears in {path}");
        }
    }
}

/// A database from before sealing holds a credential in the clear. The pass
/// that seals every secret at startup covers each one, and each opens to the
/// text it held.
#[tokio::test]
async fn every_credential_setting_stored_in_the_clear_is_sealed_at_startup() {
    let app = TestApp::new().await;
    for key in crate::services::settings::sealed_keys() {
        app.store_setting(key, &secret_for(key)).await;
    }

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    for key in crate::services::settings::sealed_keys() {
        let stored: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert!(stored.starts_with("enc:v2:"), "{key} left in the clear: {stored}");
        assert_eq!(app.state.secrets.open(&stored).unwrap(), secret_for(key), "{key}");
    }
}

/// Saved beats the environment, or setting one in the interface would look like
/// it worked and change nothing.
#[tokio::test]
async fn a_saved_key_outranks_the_environment_variable() {
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    let app = TestApp::around(crate::state::AppState::for_tests().await.with_config(config));

    assert_eq!(
        app.state.provider_key("tmdb").await.as_deref(),
        Some("from-the-environment"),
        "the environment should answer while nothing is saved"
    );

    app.save_setting("tmdb_api_key", "from-the-interface").await;
    assert_eq!(app.state.provider_key("tmdb").await.as_deref(), Some("from-the-interface"));
}

/// And clearing it falls back rather than leaving the source dead: an existing
/// deployment configured by its orchestrator must keep working untouched.
#[tokio::test]
async fn clearing_a_saved_key_falls_back_to_the_environment() {
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    let app = TestApp::around(crate::state::AppState::for_tests().await.with_config(config));

    app.save_setting("tmdb_api_key", "from-the-interface").await;
    app.save_setting("tmdb_api_key", "").await;

    assert_eq!(app.state.provider_key("tmdb").await.as_deref(), Some("from-the-environment"));
    // Empty, not an opaque blob meaning "unset".
    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'tmdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, "");
}

/// A listed source answers while its key is saved, and stops once it is
/// removed: no key anywhere is no source.
#[tokio::test]
async fn a_listed_source_answers_while_its_key_is_saved() {
    let app = TestApp::new().await;
    let both = app
        .put(
            "/api/v1/settings",
            serde_json::json!({ "settings": { "omdb_api_key": "a-key", "metadata_providers": "arr,omdb" } }),
        )
        .await;
    both.assert_ok();

    let usable = app.state.metadata_providers().await;
    assert!(usable.iter().any(|p| p.id == "omdb"), "saving a key did not enable the source");

    app.save_setting("omdb_api_key", "").await;
    let unusable = app.state.metadata_providers().await;
    assert!(!unusable.iter().any(|p| p.id == "omdb"), "omdb answered with no key at all");
}

/// A source that needs a key cannot answer without one. The Settings screen
/// refuses to add it until one is typed, and the API it calls refuses as well,
/// or a script lists a source the health page can only warn about.
#[tokio::test]
async fn a_source_that_needs_a_key_is_not_added_without_one() {
    let app = TestApp::new().await;

    assert_eq!(app.save_setting("metadata_providers", "arr,omdb").await.status, 400);
    let listed = app.state.metadata_order().await;
    assert!(!listed.iter().any(|p| p.id == "omdb"), "the refused list was stored anyway");
}

/// Already listed, a source that lost its key stays: the screen sends the
/// whole list on every save, and refusing it as it stands would refuse every
/// setting until the source is removed.
#[tokio::test]
async fn a_listed_source_without_its_key_does_not_block_a_save() {
    let app = TestApp::new().await;
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "omdb_api_key": "a-key", "metadata_providers": "arr,omdb" } }),
    )
    .await
    .assert_ok();
    app.save_setting("omdb_api_key", "").await;

    let saved = app
        .put(
            "/api/v1/settings",
            serde_json::json!({ "settings": { "metadata_providers": "arr,omdb", "batch_limit": "20" } }),
        )
        .await;

    saved.assert_ok();
}

/// A settings secret no key can open is left exactly as it was.
///
/// `reseal_secrets` runs at startup, on nobody's request, and a metadata key
/// has no second copy here. The instance half of the same pass is pinned in
/// `services::maintenance`. An inverted condition, or an `unwrap_or_default()`
/// where the `else` is, writes an empty key over the real one.
#[tokio::test]
async fn a_settings_secret_no_key_can_open_is_left_exactly_as_it_was() {
    let app = TestApp::new().await;

    // Sealed under a master key this installation has never held, which is what
    // a database restored beside the wrong `routarr.key` looks like.
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
        None,
    )
    .unwrap()
    .seal("tmdb-only-copy")
    .unwrap();
    app.store_setting("tmdb_api_key", &foreign).await;
    assert!(app.state.secrets.open(&foreign).is_err(), "the fixture key is readable after all");

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'tmdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, foreign, "the only copy of the metadata key was overwritten");
}
