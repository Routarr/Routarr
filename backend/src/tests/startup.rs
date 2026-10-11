//! A start that cannot go on says why in one sentence naming the path and
//! the variable, where SQLite would say "unable to open database file".

use crate::config::{AuthMode, Config};

/// The configuration of a first start in `path`, generating its API key.
fn first_start_at(path: std::path::PathBuf) -> Config {
    let mut config = Config::for_tests();
    config.set_db_path(path);
    config.auth_mode = AuthMode::ApiKey;
    config.api_key = None;
    config
}

/// `ROUTARR_DB_PATH` set to the data folder rather than the file in it is
/// refused by name, before a key is written beside the folder.
#[tokio::test]
async fn a_database_path_naming_a_directory_is_refused_by_name() {
    let dir = super::TempDir::new("db-path-directory");
    let config = first_start_at(dir.join("data"));
    std::fs::create_dir(dir.join("data")).unwrap();

    let refused = crate::open_storage(&config).await.err().map(|e| e.to_string());

    let refused = refused.expect("a directory opened as a database");
    assert!(refused.contains("ROUTARR_DB_PATH"), "{refused}");
    assert!(!dir.join("routarr.api_key").exists(), "a key was written beside the folder");
}

/// A data folder that cannot be made or written is named, with the remedy.
/// A path through a regular file fails whoever runs the test, root included.
#[tokio::test]
async fn an_unusable_data_directory_is_named_at_start() {
    let dir = super::TempDir::new("data-unusable");
    std::fs::write(dir.join("file"), "").unwrap();
    let config = first_start_at(dir.join("file").join("routarr.db"));

    let refused = crate::open_storage(&config).await.err().map(|e| e.to_string());

    let refused = refused.expect("a database opened under a file");
    let named = dir.join("file");
    assert!(refused.contains(&named.display().to_string()), "{refused}");
    assert!(refused.contains("chown"), "{refused}");
}

/// A start in a mode that opens the API, or that trusts what stands in front
/// of it, says so in the log, each in the words of its own setting, and the
/// mode that asks for a key says nothing.
#[tokio::test]
async fn a_start_that_opens_the_api_says_so() {
    let cases = [
        (AuthMode::None, false, Some("ROUTARR_AUTH=none")),
        (AuthMode::External, false, Some("ROUTARR_AUTH=external")),
        (AuthMode::Oidc, true, Some("ROUTARR_OIDC_ALLOW_ANYONE=true")),
        (AuthMode::Oidc, false, None),
        (AuthMode::ApiKey, false, None),
    ];
    for (mode, allow_anyone, said) in cases {
        let mut config = Config::for_tests();
        config.auth_mode = mode;
        config.oidc_allow_anyone = allow_anyone;
        let state = crate::state::AppState::for_tests().await.with_config(config);
        let log = super::LogCapture::default();
        let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(log.clone());
        tracing::subscriber::with_default(subscriber.finish(), || {
            crate::warn_on_insecure_defaults(&state);
        });
        let logged = log.contents();
        match said {
            Some(setting) => assert!(logged.contains(setting), "{mode:?}: {logged}"),
            None => assert!(logged.is_empty(), "{mode:?}: {logged}"),
        }
    }
}
