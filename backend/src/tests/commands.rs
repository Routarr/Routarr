//! The subcommands `routarr` runs instead of the server.

use axum::routing::get;

use super::{TempDir, TestApp, listen, serve_on};
use crate::commands;
use crate::config::Config;

/// A configuration whose probe asks `address`, as the image's health check
/// asks the server it runs beside.
fn probing(address: &str) -> Config {
    let url = reqwest::Url::parse(address).unwrap();
    let mut config = Config::for_tests();
    config.host = url.host_str().unwrap().to_string();
    config.port = url.port().unwrap();
    config
}

/// The image's HEALTHCHECK passes on the API's own ping alone: a page that
/// answers 200 at its address, as the interface's page does under a mount
/// point the probe got wrong, fails it, and so do an answer without the
/// ping's status and an address nothing answers.
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

/// With `--revoke-keys`, the reset writes a new API key beside the new
/// password, for an operator who believes the keys leaked with it.
#[tokio::test]
async fn a_reset_that_revokes_the_keys_writes_a_new_api_key() {
    let dir = TempDir::new("reset-account");
    let mut config = Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.api_key = None;
    std::fs::write(config.api_key_path(), "the-old-key").unwrap();

    commands::reset_account(&config, true).await.unwrap();

    let key = std::fs::read_to_string(config.api_key_path()).unwrap();
    assert!(!key.trim().is_empty() && key.trim() != "the-old-key", "the old key stayed");
    assert!(config.password_path().exists(), "no password was written");
}

/// A subcommand called without what it works on says how to call it.
#[tokio::test]
async fn a_subcommand_without_its_arguments_says_how_to_call_it() {
    let config = Config::for_tests();

    let restore = commands::restore(&config, None).await.unwrap_err().to_string();
    let decrypt = commands::decrypt_backup(&config, Some("archive".into()), None).await;

    assert!(restore.starts_with("usage: routarr restore"), "{restore}");
    let decrypt = decrypt.unwrap_err().to_string();
    assert!(decrypt.starts_with("usage: routarr decrypt-backup"), "{decrypt}");
}
