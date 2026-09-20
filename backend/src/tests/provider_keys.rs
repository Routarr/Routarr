//! Metadata credentials saved from the interface.
//!
//! Settable from the screen that orders the sources, rather than from the
//! environment alone: the screen that names a missing key is the one that
//! should be able to supply it. Sealed exactly as an Arr key is — and an Arr
//! key is the *more* dangerous of the two, since it writes to the library while
//! these only read.

use super::TestApp;

async fn save(app: &TestApp, key: &str, value: &str) -> u16 {
    app.put("/api/v1/settings", serde_json::json!({ "settings": { key: value } }))
        .await
        .status
        .as_u16()
}

async fn settings(app: &TestApp) -> serde_json::Value {
    app.get("/api/v1/settings").await.assert_ok().clone()
}

/// The claim the whole feature rests on: what is stored is sealed, and what is
/// returned is nothing.
#[tokio::test]
async fn a_saved_key_is_sealed_in_the_table_and_never_read_back() {
    let app = TestApp::new().await;
    assert_eq!(save(&app, "tmdb_api_key", "super-secret-value").await, 200);

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'tmdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(stored.starts_with("enc:v1:"), "stored in the clear: {stored}");
    assert!(!stored.contains("super-secret"), "the plaintext survived in the row");

    let body = settings(&app).await;
    assert_eq!(body["tmdb_api_key"], "", "the value came back out");
    assert_eq!(body["tmdb_api_key_configured"], true, "the screen cannot tell one is set");
    assert!(
        !serde_json::to_string(&body).unwrap().contains("super-secret"),
        "the plaintext appears somewhere in the payload"
    );
}

/// Saved beats the environment, or setting one in the interface would look like
/// it worked and change nothing.
#[tokio::test]
async fn a_saved_key_outranks_the_environment_variable() {
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    let mut state = crate::state::AppState::for_tests().await;
    state.config = std::sync::Arc::new(config);
    let app = TestApp::around(state);

    assert_eq!(
        app.state.provider_key("tmdb").await.as_deref(),
        Some("from-the-environment"),
        "the environment should answer while nothing is saved"
    );

    save(&app, "tmdb_api_key", "from-the-interface").await;
    assert_eq!(app.state.provider_key("tmdb").await.as_deref(), Some("from-the-interface"));
}

/// And clearing it falls back rather than leaving the source dead: an existing
/// deployment configured by its orchestrator must keep working untouched.
#[tokio::test]
async fn clearing_a_saved_key_falls_back_to_the_environment() {
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("from-the-environment".into());
    let mut state = crate::state::AppState::for_tests().await;
    state.config = std::sync::Arc::new(config);
    let app = TestApp::around(state);

    save(&app, "tmdb_api_key", "from-the-interface").await;
    save(&app, "tmdb_api_key", "").await;

    assert_eq!(app.state.provider_key("tmdb").await.as_deref(), Some("from-the-environment"));
    // Empty, not an opaque blob meaning "unset".
    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'tmdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, "");
}

/// A source with no key anywhere stays unusable, and one saved here becomes
/// usable — which is the point of saving it.
#[tokio::test]
async fn saving_a_key_makes_its_source_usable() {
    let app = TestApp::new().await;
    save(&app, "metadata_providers", "arr,omdb").await;

    let unusable = app.state.metadata_providers().await;
    assert!(!unusable.iter().any(|p| p.id == "omdb"), "omdb answered with no key at all");

    save(&app, "omdb_api_key", "a-key").await;
    let usable = app.state.metadata_providers().await;
    assert!(usable.iter().any(|p| p.id == "omdb"), "saving a key did not enable the source");
}

/// A rotation that leaves the metadata keys behind is a rotation that silently
/// stops the conditions reading them from matching.
#[tokio::test]
async fn the_resealing_pass_covers_the_metadata_keys_too() {
    let app = TestApp::new().await;
    // Plaintext, as a database upgraded from before sealing would hold.
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('omdb_api_key', 'bare', datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'omdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(stored.starts_with("enc:v1:"), "left in the clear: {stored}");
    assert_eq!(app.state.provider_key("omdb").await.as_deref(), Some("bare"));
}

/// A secret no key can open is left exactly as it was.
///
/// `reseal_secrets` runs at startup, on nobody's request, and it is the only
/// code in Routarr that can destroy something outright: an Arr credential has
/// no second copy here. `maintenance.rs` says so — "overwriting it would
/// destroy the only copy" — and the `let … else { continue }` that keeps that
/// promise was guarded by nothing. The one test this function had seeds a
/// *plaintext* value and checks it comes back sealed, which exercises the
/// other branch entirely.
///
/// An inverted condition, or an `unwrap_or_default()` where the `else` is,
/// writes an empty key over the real one and leaves every other test green.
#[tokio::test]
async fn a_secret_no_key_can_open_is_left_exactly_as_it_was() {
    let app = TestApp::new().await;

    // Sealed under a master key this installation has never held, which is what
    // a database restored beside the wrong `routarr.key` looks like.
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
    )
    .unwrap()
    .seal("the-only-copy")
    .unwrap();

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('i-foreign', 'Radarr', 'radarr', 'http://127.0.0.1:1', ?, 1)",
    )
    .bind(&foreign)
    .execute(&app.state.pool)
    .await
    .unwrap();

    // It cannot be opened — the precondition, asserted rather than assumed.
    assert!(app.state.secrets.open(&foreign).is_err(), "the fixture key is readable after all");

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT api_key FROM instances WHERE id = 'i-foreign'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, foreign, "the only copy of the credential was overwritten");

    // And a settings secret in the same position, since the pass covers both
    // tables and only one of them had a test at all.
    let foreign_setting = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
    )
    .unwrap()
    .seal("tmdb-only-copy")
    .unwrap();
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('tmdb_api_key', ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(&foreign_setting)
    .execute(&app.state.pool)
    .await
    .unwrap();

    crate::services::maintenance::reseal_secrets(&app.state).await.unwrap();

    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'tmdb_api_key'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, foreign_setting, "the only copy of the metadata key was overwritten");
}
