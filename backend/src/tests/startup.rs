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
