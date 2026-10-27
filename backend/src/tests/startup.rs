//! What a start settles before it serves, and what it tells the operator: why it cannot go on,
//! and that its mode opens the API.

use crate::config::{AuthMode, Config};

/// The configuration of a first start in `path`, generating its API key.
fn first_start_at(path: std::path::PathBuf) -> Config {
    let mut config = Config::for_tests();
    config.set_db_path(path);
    config.auth_mode = AuthMode::ApiKey;
    config.api_key = None;
    config
}

/// Refused before an API key is written beside the folder.
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

/// A data folder that cannot be made or written is named, and told no remedy that does not
/// apply: a path through a regular file is no matter of ownership. It fails whoever runs the
/// test, root included.
#[tokio::test]
async fn an_unusable_data_directory_is_named_at_start() {
    let dir = super::TempDir::new("data-unusable");
    std::fs::write(dir.join("file"), "").unwrap();
    let config = first_start_at(dir.join("file").join("routarr.db"));

    let refused = crate::open_storage(&config).await.err().map(|e| e.to_string());

    let refused = refused.expect("a database opened under a file");
    let named = dir.join("file");
    assert!(refused.contains(&named.display().to_string()), "{refused}");
    assert!(!refused.contains("chown"), "a path through a file is told to change owner: {refused}");
}

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

/// Each step a start takes between opening its storage and serving, run on the storage a start
/// opens: a step dropped from `prepare` fails here. A second start resumes a sealing pass.
#[tokio::test]
async fn a_start_settles_what_it_finds_before_it_serves() {
    let dir = super::TempDir::new("prepare");
    let copies = super::TempDir::new("prepare-copies");
    let mut config = first_start_at(dir.join("routarr.db"));
    config.auth_mode = AuthMode::Forms;
    config.tmdb_api_key = Some("tmdb-k1".into());
    let (api_key, pool, secrets) = crate::open_storage(&config).await.unwrap();
    let state = crate::state::AppState::new(config.clone(), pool, secrets, api_key).unwrap();
    let by = crate::jobs::Attribution::manual(None);
    let archive = crate::services::backup::create(&state, &by).await.unwrap();
    for seeded in [
        super::AN_INSTANCE,
        "INSERT INTO jobs (id, kind, status) VALUES ('j-left', 'sync', 'running')",
        "INSERT INTO categories (id, name) VALUES ('c-docs', ' Documentaries')",
    ] {
        sqlx::query(seeded).execute(&state.pool).await.unwrap();
    }
    config.backup_copy_dir = Some(copies.0.clone());
    let state = state.with_config(config);

    crate::prepare(&state).await.unwrap();

    let read = |sql: &'static str| text(&state.pool, sql);
    assert_eq!(
        read("SELECT status FROM jobs WHERE id = 'j-left'").await.as_deref(),
        Some("failed")
    );
    let sealed = read("SELECT api_key FROM instances WHERE id = 'inst-1'").await.unwrap();
    assert_eq!(state.secrets.open(&sealed).ok().as_deref(), Some("secret"));
    assert_ne!(sealed, "secret", "the key stayed in the clear");
    let sources = read("SELECT value FROM settings WHERE key = 'metadata_providers'").await;
    assert_eq!(sources.as_deref(), Some("arr,tmdb"));
    let category = read("SELECT name FROM categories WHERE id = 'c-docs'").await;
    assert_eq!(category.as_deref(), Some("documentaries"));
    assert!(read("SELECT name_key FROM instances WHERE id = 'inst-1'").await.is_some());
    let accounts = read("SELECT CAST(COUNT(*) AS TEXT) FROM users").await;
    assert_eq!(accounts.as_deref(), Some("1"));
    assert!(state.config.password_path().exists(), "no password written");
    let copied = "SELECT CAST(COUNT(*) AS TEXT) FROM jobs
                   WHERE kind = 'backup_copy' AND status != 'running'";
    super::until(|| async { read(copied).await.as_deref() == Some("1") }).await;
    assert_eq!(read(copied).await.as_deref(), Some("1"), "the archives were not copied");

    // A stop between the passphrase saved and the pass that seals the archives.
    let passphrase = state.secrets.seal("a passphrase long enough").unwrap();
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
        .bind(crate::services::backup::PASSPHRASE_SETTING)
        .bind(&passphrase)
        .execute(&state.pool)
        .await
        .unwrap();
    let sealed = dir.join("backups").join(format!("{}.enc", archive.name));

    crate::prepare(&state).await.unwrap();

    super::until(|| async { sealed.exists() }).await;
    assert!(sealed.exists(), "the archive in the clear stayed so");
}

/// The one value `sql` reads, as text.
async fn text(pool: &sqlx::SqlitePool, sql: &'static str) -> Option<String> {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}
