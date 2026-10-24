//! The subcommands `routarr` runs instead of the server.

use axum::routing::get;

use super::{TempDir, TestApp, listen, serve_on};
use crate::commands;
use crate::config::Config;

/// A configuration whose health probe asks `address`.
fn probing(address: &str) -> Config {
    let url = reqwest::Url::parse(address).unwrap();
    let mut config = Config::for_tests();
    config.host = url.host_str().unwrap().to_string();
    config.port = url.port().unwrap();
    config
}

/// Under a wrong mount point the interface's page still answers 200, so only the ping passes.
#[tokio::test]
async fn the_health_check_passes_on_the_api_and_nothing_else() {
    let app = TestApp::new().await;
    let (listener, api) = listen().await;
    let _api = serve_on(listener, crate::build_router(app.state.clone()));
    let elsewhere = axum::Router::new()
        .route("/api/v1/ping", get(|| async { axum::response::Html("<!doctype html>") }))
        .route(
            "/other/api/v1/ping",
            get(|| async { axum::Json(serde_json::json!({ "status": "starting" })) }),
        );
    let (listener, address) = listen().await;
    let _elsewhere = serve_on(listener, elsewhere);
    let mut other = probing(&address);
    other.base_path = "/other".into();

    commands::healthcheck(&probing(&api)).await.expect("the API answered its ping");
    assert!(commands::healthcheck(&probing(&address)).await.is_err(), "a page passed for the API");
    assert!(commands::healthcheck(&other).await.is_err(), "an answer without its status passed");
    assert!(commands::healthcheck(&probing("http://127.0.0.1:1")).await.is_err());
}

#[tokio::test]
async fn a_reset_that_revokes_the_keys_writes_a_new_api_key() {
    let dir = TempDir::new("reset-account");
    let mut config = Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.api_key = None;
    config.auth_mode = crate::config::AuthMode::Forms;
    std::fs::write(config.api_key_path(), "the-old-key").unwrap();

    commands::reset_account(&config, true, false).await.unwrap();

    let key = std::fs::read_to_string(config.api_key_path()).unwrap();
    assert!(!key.trim().is_empty() && key.trim() != "the-old-key", "the old key stayed");
    assert!(config.password_path().exists(), "no password was written");
    // The security log says what the command did, as it says what Settings does.
    let pool = crate::db::init_pool(&config).await.unwrap();
    let said: Vec<String> = sqlx::query_scalar("SELECT message FROM security_events ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(said, ["AuditAccountReset", "AuditEveryKeyRevoked"]);
}

/// An owner who withdrew the API key from Settings keeps it withdrawn: revoking every key
/// replaces a key there is, and makes none where there was none.
#[tokio::test]
async fn a_reset_makes_no_api_key_where_there_was_none() {
    let dir = TempDir::new("reset-account-no-key");
    let mut config = Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.api_key = None;
    config.auth_mode = crate::config::AuthMode::Forms;

    commands::reset_account(&config, true, false).await.unwrap();

    assert!(!config.api_key_path().exists(), "a key the owner withdrew is back");
    assert!(config.password_path().exists(), "no password was written");
}

/// Outside `forms` no sign-in reads the account, and a password printed for a sign-in screen
/// that does not exist would read as a way in.
#[tokio::test]
async fn a_reset_outside_forms_writes_no_account() {
    let dir = TempDir::new("reset-account-apikey");
    let mut config = Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.auth_mode = crate::config::AuthMode::ApiKey;

    let refused = commands::reset_account(&config, false, false).await;

    assert!(refused.is_err_and(|e| e.to_string().contains("--force")));
    assert!(!config.password_path().exists(), "a password was written");
    commands::reset_account(&config, false, true).await.unwrap();
    assert!(config.password_path().exists(), "--force reset nothing");
}

#[tokio::test]
async fn a_subcommand_without_its_arguments_says_how_to_call_it() {
    let config = Config::for_tests();

    let restore = commands::restore(&config, None).await.unwrap_err().to_string();
    let decrypt = commands::decrypt_backup(&config, Some("archive".into()), None).await;

    assert!(restore.starts_with("usage: routarr restore"), "{restore}");
    let decrypt = decrypt.unwrap_err().to_string();
    assert!(decrypt.starts_with("usage: routarr decrypt-backup"), "{decrypt}");
}
