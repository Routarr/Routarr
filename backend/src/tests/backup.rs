//! Backups: taking one, keeping the right number, and restoring safely.
//!
//! The property that matters most is not that a zip appears. It is that the
//! archive is **self-sufficient**, and that a restore never half-applies. A
//! database restored without `routarr.key` opens fine and has no readable Arr
//! credential in it, which is the failure a backup exists to prevent.

use crate::services::backup;
use crate::state::AppState;

use super::{TempDir, TestApp, warning_messages};

/// A harness backed by a real database file in a throwaway directory.
///
/// Not the usual in-memory pool: a backup is a *file* operation, and
/// `VACUUM INTO`, the whole point of taking a consistent copy without stopping
/// the server, does nothing at all against `:memory:`. Testing it there would
/// prove something that cannot happen in production.
pub(crate) async fn app_with_files(label: &str) -> (TestApp, TempDir) {
    let dir = TempDir::new(&format!("backup-{label}"));

    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    // The file is the key, as on an installation that never set
    // `ROUTARR_SECRET_KEY`: what the archive carries is what sealed the data.
    config.secret_key = None;
    std::fs::write(config.secret_key_path(), "a-master-key").unwrap();
    std::fs::write(config.api_key_path(), "an-api-key").unwrap();

    let pool = crate::db::init_pool(&config).await.unwrap();
    let secrets = crate::crypto::SecretBox::load(
        config.secret_key.as_deref(),
        None,
        &config.secret_key_path(),
        None,
    )
    .unwrap();

    let state = AppState { secrets, ..AppState::for_tests_on(pool).with_config(config) };
    (TestApp::around(state), dir)
}

fn entries(archive: &std::path::Path) -> Vec<String> {
    let file = std::fs::File::open(archive).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect()
}

/// With the master key in `ROUTARR_SECRET_KEY`, a `routarr.key` beside the
/// database is a stale file, not the key anything is sealed with. The archive
/// leaves it out and says it carries no key, so the restore warns rather than
/// opening a database nothing can read.
#[tokio::test]
async fn a_key_from_the_environment_is_not_claimed_by_the_archive() {
    let (app, dir) = app_with_files("environment-key").await;
    let mut config = (*app.state.config).clone();
    config.secret_key = Some("the-key-in-the-environment".to_string());
    let app = TestApp::around(app.state.clone().with_config(config));

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);
    assert!(!backup::read_manifest(&archive).unwrap().includes_master_key);
    assert!(!entries(&archive).contains(&"routarr.key".to_string()), "a stale key file travelled");
}

#[tokio::test]
async fn a_backup_carries_everything_a_restore_needs() {
    let (app, dir) = app_with_files("complete").await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);
    let names = entries(&archive);

    // The database alone is not a backup: without the master key every Arr
    // credential in it is undecryptable.
    assert!(names.contains(&"routarr.db".to_string()));
    assert!(names.contains(&"routarr.key".to_string()));
    assert!(names.contains(&"routarr.api_key".to_string()));
    assert!(names.contains(&"manifest.json".to_string()));

    let manifest = backup::read_manifest(&archive).unwrap();
    assert!(manifest.includes_master_key);
    assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn the_snapshot_is_a_real_database_taken_without_stopping() {
    let (app, dir) = app_with_files("snapshot").await;
    app.seed_library().await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    // Extract the database and open it: a `VACUUM INTO` snapshot has to be a
    // valid, self-contained database, WAL folded in.
    let extracted = dir.join("extracted.db");
    {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive).unwrap()).unwrap();
        let mut source = zip.by_name("routarr.db").unwrap();
        let mut out = std::fs::File::create(&extracted).unwrap();
        std::io::copy(&mut source, &mut out).unwrap();
    }

    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", extracted.display())).await.unwrap();
    let media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
    assert_eq!(media, 1, "the live library was not captured");
    pool.close().await;
}

/// A retention count stored above the ceiling this build enforces is honoured
/// as it is: lowering it removes archives, and nothing but the operator's own
/// save may do that. Named in the warnings until then, since the screen
/// refuses to save it as it stands.
#[tokio::test]
async fn a_retention_count_above_the_maximum_is_kept_until_the_operator_lowers_it() {
    let (app, dir) = app_with_files("retention-ceiling").await;

    app.store_setting("backup_retention_count", "200").await;
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    for stamp in ["20260101-000000", "20260102-000000", "20260103-000000", "20260104-000000"] {
        std::fs::write(backups.join(format!("routarr-backup-{stamp}.zip")), b"x").unwrap();
    }

    let converged =
        crate::services::maintenance::converge_setting_bounds(&app.state).await.unwrap();
    assert_eq!(converged, 0, "a startup rewrote a retention count");
    let stored: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'backup_retention_count'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, "200");
    assert_eq!(backup::prune(&app.state).await.unwrap(), 0, "archives went without a save");

    let status = app.get("/api/v1/status").await.assert_ok().clone();
    let named = warning_messages(&status).iter().any(|w| w.contains("backup_retention_count"));
    assert!(named, "the warnings do not name the key: {status}");

    // Saved as it stands it is refused, and the warning is what explains why.
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "backup_retention_count": "200" } }),
    )
    .await
    .assert_status(axum::http::StatusCode::BAD_REQUEST);

    // Lowered by the operator, it takes effect at once.
    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "backup_retention_count": "2" } }),
    )
    .await
    .assert_ok();
    assert_eq!(backup::list(&app.state).len(), 2);
}

#[tokio::test]
async fn only_the_retained_count_survives_a_prune() {
    let (app, dir) = app_with_files("retention").await;

    app.store_setting("backup_retention_count", "2").await;

    // The name carries a second-resolution timestamp, so three in the same
    // second would collide. They are written directly instead.
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
        std::fs::write(backups.join(format!("routarr-backup-{stamp}.zip")), b"x").unwrap();
    }

    let removed = backup::prune(&app.state).await.unwrap();
    assert_eq!(removed, 1);

    let left: Vec<String> = backup::list(&app.state).into_iter().map(|f| f.name).collect();
    assert_eq!(left.len(), 2);
    // The oldest goes first.
    assert!(left.contains(&"routarr-backup-20260103-000000.zip".to_string()));
    assert!(!left.contains(&"routarr-backup-20260101-000000.zip".to_string()));
}

/// A retention count the database fails to read deletes nothing: taken for
/// the default, it would delete the archives a larger count keeps.
#[tokio::test]
async fn a_retention_that_cannot_be_read_removes_nothing() {
    let (app, dir) = app_with_files("retention-unread").await;
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    for day in 1..=10 {
        std::fs::write(backups.join(format!("routarr-backup-202601{day:02}-000000.zip")), b"x")
            .unwrap();
    }
    app.execute(&["ALTER TABLE settings RENAME TO settings_unreadable"]).await;

    assert!(backup::prune(&app.state).await.is_err(), "the prune went ahead");
    assert_eq!(backup::list(&app.state).len(), 10);
}

#[tokio::test]
async fn a_backup_from_a_newer_schema_is_refused_rather_than_half_applied() {
    let (app, dir) = app_with_files("schema").await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    // Rewrite the manifest as if a later Routarr had produced it.
    let forged = dir.join("backups").join("routarr-backup-99999999-000000.zip");
    {
        let mut source = zip::ZipArchive::new(std::fs::File::open(&archive).unwrap()).unwrap();
        let mut out = zip::ZipWriter::new(std::fs::File::create(&forged).unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for i in 0..source.len() {
            let mut entry = source.by_index(i).unwrap();
            let name = entry.name().to_string();
            out.start_file(&name, options).unwrap();
            if name == "manifest.json" {
                let manifest = serde_json::json!({
                    "version": "9.9.9",
                    "schema": "099_from_the_future",
                    "created_at": "2099-01-01 00:00:00",
                    "includes_master_key": true
                });
                std::io::Write::write_all(&mut out, manifest.to_string().as_bytes()).unwrap();
            } else {
                std::io::copy(&mut entry, &mut out).unwrap();
            }
        }
        out.finish().unwrap();
    }

    let outcome =
        backup::stage_restore(&app.state, "routarr-backup-99999999-000000.zip", None).await;
    let message = outcome.expect_err("a newer schema must be refused").to_string();
    assert!(message.contains("Upgrade Routarr first"), "unhelpful refusal: {message}");

    // And nothing was staged: a refusal that still wrote files would be worse
    // than no check at all.
    let staged = dir.join("routarr.db.restore-pending");
    assert!(!staged.exists(), "a refused restore left files behind");
}

#[tokio::test]
async fn a_restore_is_staged_and_applied_only_at_the_next_start() {
    let (app, dir) = app_with_files("restore").await;
    app.seed_library().await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    // Change the live database after the backup, so a successful restore is
    // observable rather than a no-op.
    sqlx::query("DELETE FROM media").execute(&app.state.pool).await.unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();

    // Staged, not swapped: the pool is still open on the old file.
    assert!(dir.join("routarr.db.restore-pending").exists());
    let live: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(live, 0, "the database was swapped underneath a live pool");

    // What `main` does before opening anything.
    let config = app.state.config.clone();
    app.state.pool.close().await;
    assert!(backup::apply_pending_restore(&config).await.unwrap());
    assert!(!dir.join("routarr.db.restore-pending").exists());

    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", config.db_path.display())).await.unwrap();
    let restored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
    assert_eq!(restored, 1, "the staged database was not applied");
    pool.close().await;
}

/// A restore brings back the library of its day, not a credential the
/// installation has withdrawn since: a key revoked because it leaked stays
/// revoked, and the notifications keep the signing secrets of today.
#[tokio::test]
async fn a_restore_brings_back_no_credential_withdrawn_since() {
    use crate::services::{applications, notify};

    let (app, _dir) = app_with_files("credentials").await;
    let new = applications::NewApplication {
        name: "request-bot".into(),
        scopes: Vec::new(),
        may_confirm: Vec::new(),
        may_move_files: false,
    };
    let leaked = applications::create(&app.state, new, None).await.unwrap();
    notify::rotate_signing_secret(&app.state).await.unwrap();
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    applications::revoke(&app.state.pool, &leaked.application.id).await.unwrap();
    notify::rotate_signing_secret(&app.state).await.unwrap();
    notify::rotate_signing_secret(&app.state).await.unwrap();
    let today: Vec<String> =
        sqlx::query_scalar("SELECT secret FROM webhook_secrets ORDER BY secret")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    let config = app.state.config.clone();
    app.state.pool.close().await;
    assert!(backup::apply_pending_restore(&config).await.unwrap());

    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", config.db_path.display())).await.unwrap();
    let revoked: Option<String> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys WHERE id = ?")
            .bind(&leaked.application.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked.is_some(), "a revoked key came back with the restore");
    let restored: Vec<String> =
        sqlx::query_scalar("SELECT secret FROM webhook_secrets ORDER BY secret")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(restored, today, "the restore brought back a replaced signing secret");
    pool.close().await;
}

/// The master API key and the account's password are today's too: one
/// rotated or changed because it leaked is not brought back, and every session
/// the archive held is closed, as a changed password closes them.
#[tokio::test]
async fn a_restore_keeps_todays_api_key_and_password_and_signs_everyone_out() {
    let (app, _dir) = app_with_files("signins").await;
    for statement in [
        "INSERT INTO users (id, username, password_hash) VALUES ('u-1', 'admin', 'leaked-hash')",
        "INSERT INTO sessions (id, subject, source, expires_at)
         VALUES ('s-1', 'admin', 'forms', datetime('now', '+7 days'))",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    sqlx::query("UPDATE users SET password_hash = 'todays-hash'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions").execute(&app.state.pool).await.unwrap();
    let config = app.state.config.clone();
    std::fs::write(config.api_key_path(), "rotated-key").unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    app.state.pool.close().await;
    assert!(backup::apply_pending_restore(&config).await.unwrap());

    assert_eq!(std::fs::read_to_string(config.api_key_path()).unwrap(), "rotated-key");
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", config.db_path.display())).await.unwrap();
    let hash: String =
        sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!(hash, "todays-hash", "the restore brought back a changed password");
    let sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions").fetch_one(&pool).await.unwrap();
    assert_eq!(sessions, 0, "a session the archive held came back");
    pool.close().await;
}

/// A restore onto a host whose key file is not the archive's, as a new host
/// generates one, brings the archive's back: every credential sealed under it
/// opens again. Counting the restored rows would pass with the keys swapped
/// or left behind, and the restore would open a database nothing can read.
#[tokio::test]
async fn a_restore_brings_back_the_key_its_credentials_were_sealed_with() {
    let (app, _dir) = app_with_files("keys-travel").await;
    let sealed = app.state.secrets.seal("the-radarr-key").unwrap();
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let config = app.state.config.clone();
    std::fs::write(config.secret_key_path(), "a-new-hosts-key").unwrap();
    std::fs::remove_file(config.api_key_path()).unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    app.state.pool.close().await;
    assert!(backup::apply_pending_restore(&config).await.unwrap());

    let secrets =
        crate::crypto::SecretBox::load(None, None, &config.secret_key_path(), None).unwrap();
    assert_eq!(
        secrets.open(&sealed).unwrap(),
        "the-radarr-key",
        "the archive's key did not come back"
    );
    assert_eq!(std::fs::read_to_string(config.api_key_path()).unwrap(), "an-api-key");
}

/// On a host where `ROUTARR_SECRET_KEY` is set, that key wins at the next
/// start over the `routarr.key` a restore brings back. An archive sealed under
/// its key file would then restore credentials nothing can open, so it is
/// refused while it can still be, naming the variable.
#[tokio::test]
async fn a_restore_the_next_start_could_not_open_is_refused() {
    let (app, dir) = app_with_files("environment-wins").await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    let mut config = (*app.state.config).clone();
    config.secret_key = Some("another-key-in-the-environment".to_string());
    let app = TestApp::around(app.state.clone().with_config(config));
    let refused =
        backup::stage_restore(&app.state, &file.name, None).await.expect_err("it was staged");
    assert!(refused.to_string().contains("ROUTARR_SECRET_KEY"), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
}

/// Without the variable the next start opens with the key file the archive
/// brings back. One that is not the key its credentials were sealed with, as
/// an archive taken while the variable was set could carry, is refused too.
#[tokio::test]
async fn a_restore_whose_own_key_file_cannot_open_it_is_refused() {
    let (app, dir) = app_with_files("stale-key-file").await;
    let backups = dir.join("backups");
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let forged = "routarr-backup-20000101-000000.zip";
    let stale = b"c3RhbGUta2V5LXRoYXQtc2VhbGVkLW5vdGhpbmctaGVyZQ==";
    forge(&backups.join(&file.name), &backups.join(forged), "routarr.key", stale);

    backup::stage_restore(&app.state, &file.name, None)
        .await
        .expect("the archive as taken restores");
    let refused = backup::stage_restore(&app.state, forged, None).await.expect_err("it was staged");
    assert!(refused.to_string().contains("routarr.key"), "{refused}");
}

/// A name taken from the URL never leaves the backup folder: the routes that
/// download, delete and restore an archive each refuse one that climbs out,
/// and the file it names, the master key here, is left as it was.
#[tokio::test]
async fn no_backup_route_reaches_a_file_outside_the_backup_folder() {
    let (app, dir) = app_with_files("traversal").await;
    std::fs::create_dir_all(dir.join("backups")).unwrap();
    let key = std::fs::read(dir.join("routarr.key")).unwrap();
    let escaping = "/api/v1/backups/..%2Froutarr.key";

    let read = app.get(escaping).await;
    assert_eq!(read.status, axum::http::StatusCode::NOT_FOUND, "{:?}", read.json);
    let deleted = app.delete(escaping).await;
    assert_eq!(deleted.status, axum::http::StatusCode::NOT_FOUND, "{:?}", deleted.json);
    let restored = app.post(&format!("{escaping}/restore"), serde_json::json!({})).await;
    assert_eq!(restored.status, axum::http::StatusCode::NOT_FOUND, "{:?}", restored.json);

    assert_eq!(std::fs::read(dir.join("routarr.key")).unwrap(), key, "the key was touched");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "something was staged");
}

/// A start applies a staged restore before it opens anything the archive
/// replaces: the database it opens, and the master key it opens it with, are
/// the archive's, not the ones on disk before the restart.
#[tokio::test]
async fn a_start_opens_the_restored_database_with_the_restored_key() {
    let (app, dir) = app_with_files("start-order").await;
    app.seed_instance_at("inst-1", "radarr", "http://127.0.0.1:1").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    app.execute(&["DELETE FROM instances"]).await;
    std::fs::write(dir.join("routarr.key"), "the-key-of-today").unwrap();
    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    let config = (*app.state.config).clone();
    app.state.pool.close().await;

    let (_, pool, secrets) = crate::open_storage(&config).await.unwrap();

    let sealed: String = sqlx::query_scalar("SELECT api_key FROM instances WHERE id = 'inst-1'")
        .fetch_one(&pool)
        .await
        .expect("the database opened is not the restored one");
    assert_eq!(secrets.open(&sealed).expect("opened with the key of before"), "arr-key");
}

/// Everything a start writes beside the database holds a secret or the data:
/// the master key, the generated password, the database and its sidecars,
/// each readable by the owner alone.
#[cfg(unix)]
#[tokio::test]
async fn a_start_leaves_every_file_it_writes_readable_by_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("modes");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.secret_key = None;
    config.auth_mode = crate::config::AuthMode::Forms;

    let (_, pool, _) = crate::open_storage(&config).await.unwrap();
    crate::services::accounts::ensure_account(&pool, &config.password_path()).await.unwrap();
    sqlx::query("INSERT INTO categories (id, name) VALUES ('c-1', 'written')")
        .execute(&pool)
        .await
        .unwrap();

    for file in ["routarr.db", "routarr.key", "routarr.password"] {
        let mode = std::fs::metadata(dir.join(file)).expect(file).permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{file} is readable by others: {:o}", mode & 0o777);
    }
    for sidecar in ["routarr.db-wal", "routarr.db-shm"] {
        if let Ok(metadata) = std::fs::metadata(dir.join(sidecar)) {
            let mode = metadata.permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{sidecar} is readable by others: {:o}", mode & 0o777);
        }
    }
    pool.close().await;
}

/// An archive is named to the second, and a name already taken is refused at
/// once, as a conflict, before a copy of the database is written for nothing.
#[tokio::test]
async fn a_second_backup_in_the_same_second_is_refused_before_any_copy() {
    let (app, dir) = app_with_files("same-second").await;
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    // Every second the call can land in is taken.
    let now = chrono::Utc::now();
    for ahead in 0..3 {
        let stamp = (now + chrono::Duration::seconds(ahead)).format("%Y%m%d-%H%M%S");
        std::fs::write(backups.join(format!("routarr-backup-{stamp}.zip")), b"taken").unwrap();
    }

    let refused = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await;

    assert!(matches!(refused, Err(crate::error::AppError::Conflict(_))), "{refused:?}");
    let left: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "a copy was written for nothing");
}

/// A restore keeps the database it replaces, beside it as
/// `routarr.db.pre-restore`: the wrong line picked in the list of archives
/// would otherwise lose everything written since that archive.
#[tokio::test]
async fn a_restore_keeps_the_database_it_replaces() {
    let (app, dir) = app_with_files("pre-restore").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    app.execute(&["INSERT INTO categories (id, name) VALUES ('cat-since', 'written-since')"]).await;
    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    let config = (*app.state.config).clone();
    app.state.pool.close().await;

    assert!(backup::apply_pending_restore(&config).await.unwrap(), "nothing was applied");

    let kept = dir.join("routarr.db.pre-restore");
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&kept).read_only(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.expect("no copy of the database");
    let since: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE name = 'written-since'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(since, 1, "the copy lost what was written since the archive");
}

#[tokio::test]
async fn nothing_is_applied_when_no_restore_is_pending() {
    let (app, _dir) = app_with_files("nopending").await;
    assert!(!backup::apply_pending_restore(&app.state.config).await.unwrap());
}

/// Every file a restore leaves beside its targets, staged, pending or set aside.
fn restore_leftovers(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".restore-"))
        .collect()
}

/// Flip one byte in the middle of an entry's compressed data.
///
/// The zip reader checks an entry's CRC only once it has read the entry to its
/// end, so the damage is found after part of it has already been copied out,
/// which is the moment a restore has to recover from.
fn damage_entry(archive: &std::path::Path, entry: &str) {
    let (start, size) = {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
        let file = zip.by_name(entry).unwrap();
        (file.data_start().expect("the entry's data offset"), file.compressed_size())
    };
    let mut bytes = std::fs::read(archive).unwrap();
    let middle = usize::try_from(start + size / 2).unwrap();
    bytes[middle] ^= 0xFF;
    std::fs::write(archive, bytes).unwrap();
}

/// Copy an archive, replacing one entry's content.
fn forge(archive: &std::path::Path, forged: &std::path::Path, entry: &str, content: &[u8]) {
    rewrite(archive, forged, entry, Some(content));
}

/// Copy an archive, replacing one entry's content or, given none, leaving the
/// entry out.
fn rewrite(
    archive: &std::path::Path,
    forged: &std::path::Path,
    entry: &str,
    content: Option<&[u8]>,
) {
    let mut source = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::fs::File::create(forged).unwrap());
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    for i in 0..source.len() {
        let mut file = source.by_index(i).unwrap();
        let name = file.name().to_string();
        if name != entry {
            out.start_file(&name, options).unwrap();
            std::io::copy(&mut file, &mut out).unwrap();
        } else if let Some(content) = content {
            out.start_file(&name, options).unwrap();
            std::io::Write::write_all(&mut out, content).unwrap();
        }
    }
    out.finish().unwrap();
}

/// An archive's entry, read whole.
fn entry_bytes(archive: &std::path::Path, entry: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut zip.by_name(entry).unwrap(), &mut bytes).unwrap();
    bytes
}

async fn media_count(config: &crate::config::Config) -> i64 {
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", config.db_path.display())).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
    pool.close().await;
    count
}

#[tokio::test]
async fn a_restore_that_fails_while_staging_leaves_nothing_for_the_next_start() {
    let (app, dir) = app_with_files("staging-fails").await;
    app.seed_library().await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    damage_entry(&dir.join("backups").join(&file.name), "routarr.db");

    let refused = backup::stage_restore(&app.state, &file.name, None)
        .await
        .expect_err("a damaged archive must be refused");

    // What was copied before the damage was found must not stay behind: the
    // next start would move it over the live database.
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
    assert!(
        matches!(refused, crate::error::AppError::BadRequest(_)),
        "a damaged archive is the operator's to hear about, not an internal error: {refused}"
    );

    let config = app.state.config.clone();
    app.state.pool.close().await;
    assert!(!backup::apply_pending_restore(&config).await.unwrap(), "something was applied");
    assert_eq!(media_count(&config).await, 1, "the live database did not survive");
}

#[tokio::test]
async fn an_archive_whose_database_is_not_one_is_refused_before_anything_is_staged() {
    let (app, dir) = app_with_files("empty-db").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    // A well-formed archive whose database entry is empty: its CRC is right,
    // its manifest reads, and restoring it would start the next run on an
    // empty schema.
    forge(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.db",
        b"",
    );

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip", None)
        .await
        .expect_err("an archive holding no database must be refused");

    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
}

#[tokio::test]
async fn a_pending_restore_that_cannot_be_read_is_not_applied_at_the_next_start() {
    let (app, dir) = app_with_files("unreadable-pending").await;
    app.seed_library().await;
    let config = app.state.config.clone();
    app.state.pool.close().await;

    // A pending set whose database is not one, damaged since it was staged.
    // The next start is where it would be applied.
    std::fs::write(dir.join("routarr.db.restore-pending"), b"not a database").unwrap();
    std::fs::write(dir.join("routarr.key.restore-pending"), b"another-master-key").unwrap();

    assert!(
        !backup::apply_pending_restore(&config).await.unwrap(),
        "an unreadable restore was applied"
    );
    assert_eq!(media_count(&config).await, 1, "the live database did not survive");
    assert_eq!(
        std::fs::read_to_string(config.secret_key_path()).unwrap(),
        "a-master-key",
        "half of a rejected restore was applied"
    );
    // Left pending, it would be retried, and refused, on every start.
    assert_eq!(
        restore_leftovers(&dir),
        Vec::<String>::new(),
        "the rejected restore is still pending"
    );
}

#[tokio::test]
async fn keys_pending_without_their_database_are_not_applied() {
    let (app, dir) = app_with_files("keys-alone").await;
    let config = app.state.config.clone();
    app.state.pool.close().await;

    // The database is staged, and applied, last. Keys pending without it are a
    // staging that stopped before its end, and applied alone they would leave
    // the live database under a master key that does not open it.
    std::fs::write(dir.join("routarr.key.restore-pending"), b"another-master-key").unwrap();

    assert!(!backup::apply_pending_restore(&config).await.unwrap(), "keys were applied alone");
    assert_eq!(std::fs::read_to_string(config.secret_key_path()).unwrap(), "a-master-key");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "the keys are still pending");
}

#[tokio::test]
async fn staging_a_second_backup_replaces_the_first_entirely() {
    let (app, dir) = app_with_files("restage").await;
    let backups = dir.join("backups");

    let first = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    // Two archives taken in one second would share a name.
    std::fs::rename(backups.join(&first.name), backups.join("routarr-backup-20000101-000000.zip"))
        .unwrap();
    // A key in use stays, so the archive's is staged only on a host that has none.
    std::fs::remove_file(app.state.config.api_key_path()).unwrap();
    backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip", None).await.unwrap();
    assert!(
        dir.join("routarr.api_key.restore-pending").exists(),
        "precondition: the first archive carries the API key"
    );

    let second = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    assert!(
        !entries(&backups.join(&second.name)).contains(&"routarr.api_key".to_string()),
        "precondition: the second archive carries no API key"
    );
    backup::stage_restore(&app.state, &second.name, None).await.unwrap();

    assert!(
        !dir.join("routarr.api_key.restore-pending").exists(),
        "the first archive's API key would be restored beside the second archive's database"
    );
}

/// `integrity_check` is what finds damage inside a database whose first pages
/// read: the migrations table answers, and a page further in is garbage that
/// the first query to reach it after the restart reports as a malformed file.
#[tokio::test]
async fn a_database_damaged_inside_is_refused_before_anything_is_staged() {
    let (app, dir) = app_with_files("damaged-inside").await;
    app.seed_library().await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    // The media table's page, found in the archive's own database and then
    // overwritten with a page type SQLite does not have.
    let copy = dir.join("inspected.db");
    std::fs::write(&copy, entry_bytes(&backups.join(&file.name), "routarr.db")).unwrap();
    let inspect = sqlx::SqlitePool::connect(&format!("sqlite://{}", copy.display())).await.unwrap();
    let page: i64 = sqlx::query_scalar("SELECT rootpage FROM sqlite_master WHERE name = 'media'")
        .fetch_one(&inspect)
        .await
        .unwrap();
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size").fetch_one(&inspect).await.unwrap();
    inspect.close().await;
    let mut bytes = std::fs::read(&copy).unwrap();
    bytes[usize::try_from((page - 1) * page_size).unwrap()] = 0xFF;
    forge(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.db",
        &bytes,
    );

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip", None)
        .await
        .expect_err("a database damaged inside must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
}

/// A key entry the archive holds and cannot read is a damaged archive, not an
/// archive without a key. Taken for an absent one, the restore goes ahead
/// without it while the manifest says the key is included, and after the
/// restart no Arr credential opens.
#[tokio::test]
async fn a_key_the_archive_cannot_read_is_refused_rather_than_left_out() {
    let (app, dir) = app_with_files("damaged-key").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = backups.join(&file.name);

    // The entry's local header, which the reader checks before its data.
    let header = {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive).unwrap()).unwrap();
        zip.by_name("routarr.key").unwrap().header_start()
    };
    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[usize::try_from(header).unwrap()] ^= 0xFF;
    std::fs::write(&archive, bytes).unwrap();

    let refused = backup::stage_restore(&app.state, &file.name, None)
        .await
        .expect_err("a key that cannot be read must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
}

/// The manifest says whether the master key travelled. An archive that says
/// it did and does not carry it has lost it, and restoring its database under
/// the current key leaves every sealed credential unreadable.
#[tokio::test]
async fn an_archive_missing_the_key_its_manifest_lists_is_refused() {
    let (app, dir) = app_with_files("missing-key").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    rewrite(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.key",
        None,
    );

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip", None)
        .await
        .expect_err("an archive that lost its master key must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
}

/// A restore that is refused replaces nothing. Discarded before the second
/// archive is read, the first one is lost, and the operator hears only about
/// the second.
#[tokio::test]
async fn a_refused_restore_leaves_the_one_already_staged() {
    let (app, dir) = app_with_files("refused-second").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    backup::stage_restore(&app.state, &file.name, None).await.unwrap();

    forge(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.db",
        b"",
    );
    backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip", None)
        .await
        .expect_err("precondition: the second archive is refused");

    let mut left = restore_leftovers(&dir);
    left.sort();
    assert_eq!(
        left,
        ["routarr.db.restore-pending", "routarr.key.restore-pending"],
        "the restore staged first did not survive a refused one"
    );
}

/// Two stagings write the same files. A request dropped by a proxy leaves its
/// staging running on a blocking thread, and a retry beside it would have the
/// first one remove, or overwrite, what the second has just checked.
#[tokio::test]
async fn a_restore_is_staged_one_at_a_time() {
    let (app, _dir) = app_with_files("one-at-a-time").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    let held = app.state.jobs.try_lock("restore").expect("the restore lock");
    let refused = backup::stage_restore(&app.state, &file.name, None)
        .await
        .expect_err("a second staging must wait for the first");
    assert!(matches!(refused, crate::error::AppError::Conflict(_)), "{refused}");
    drop(held);

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
}

/// A newer binary may stage a restore and an older one make the next start.
/// The migrations only go forward, so the older one would run against a schema
/// it does not know.
#[tokio::test]
async fn a_pending_database_from_a_newer_schema_is_not_applied() {
    let (app, dir) = app_with_files("newer-pending").await;
    app.seed_library().await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let config = app.state.config.clone();
    app.state.pool.close().await;

    let pending = dir.join("routarr.db.restore-pending");
    std::fs::write(&pending, entry_bytes(&dir.join("backups").join(&file.name), "routarr.db"))
        .unwrap();
    let future =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", pending.display())).await.unwrap();
    sqlx::query("INSERT INTO _migrations (id, name) VALUES (9999, '9999_from_the_future')")
        .execute(&future)
        .await
        .unwrap();
    future.close().await;

    assert!(!backup::apply_pending_restore(&config).await.unwrap(), "a newer schema was applied");
    assert_eq!(media_count(&config).await, 1, "the live database did not survive");
}

/// A process killed mid-backup or mid-staging leaves files no later pass takes
/// away: a copy of the whole database, an archive holding the master key in
/// clear, a staged database. Nothing else runs at a start, so that is where
/// they go.
#[tokio::test]
async fn what_an_interrupted_run_leaves_is_swept_at_the_next_start() {
    let (app, dir) = app_with_files("sweep").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let config = app.state.config.clone();
    app.state.pool.close().await;

    let work = dir.join(".backup-work");
    std::fs::create_dir_all(&work).unwrap();
    for leftover in [
        backups.join(".routarr-backup-20260101-000000.zip.partial"),
        backups.join(".20260101-000000.db"),
        work.join("0123456789abcdef-snapshot.db"),
        work.join("fedcba9876543210-opened.zip"),
        dir.join("routarr.db.restore-staging"),
        dir.join("routarr.key.restore-staging"),
    ] {
        std::fs::write(leftover, b"left behind").unwrap();
    }
    // Not Routarr's, even in its directory.
    std::fs::write(backups.join(".keep"), b"").unwrap();

    backup::sweep_leftovers(&config);

    let mut left: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".keep".to_string(), file.name], "a leftover survived, or more went");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a staged file survived");
    let in_the_clear = std::fs::read_dir(&work).unwrap().count();
    assert_eq!(in_the_clear, 0, "a file of the work directory survived");
}

// ------------------------------------------------------------------- API

#[tokio::test]
async fn the_api_takes_lists_and_deletes_a_backup() {
    let (app, _dir) = app_with_files("api").await;

    let created = app.post("/api/v1/backups", serde_json::json!({})).await;
    let name = created.assert_ok()["name"].as_str().unwrap().to_string();

    let listed = app.get("/api/v1/backups").await;
    let body = listed.assert_ok();
    assert_eq!(body["backups"].as_array().unwrap().len(), 1);
    assert_eq!(body["retention_count"], 7);

    // The download is the point of the route, beyond its refusals: the bytes
    // have to be the archive, typed as one.
    use http_body_util::BodyExt;
    let response = app.raw(&format!("/api/v1/backups/{name}")).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("application/zip"), "{content_type}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..2], b"PK", "not a zip archive");

    let deleted = app.delete(&format!("/api/v1/backups/{name}")).await;
    deleted.assert_ok();
    assert!(backup::list(&app.state).is_empty());
}

// ------------------------------------------------- what bounds the list

/// The interface renders every backup it is given, so the retention count is
/// what stops that card growing without end. As a plain positive integer it
/// accepts "keep 10 000", and the disk fills with the very thing meant to
/// protect it.
#[tokio::test]
async fn the_retention_count_is_bounded_on_both_sides() {
    let (app, _dir) = app_with_files("retention-bounds").await;

    for value in ["1", "7", "50"] {
        let response = app
            .put(
                "/api/v1/settings",
                serde_json::json!({ "settings": { "backup_retention_count": value } }),
            )
            .await;
        response.assert_ok();
    }

    for value in ["0", "-1", "51", "10000", "many"] {
        let response = app
            .put(
                "/api/v1/settings",
                serde_json::json!({ "settings": { "backup_retention_count": value } }),
            )
            .await;
        assert_eq!(
            response.status,
            axum::http::StatusCode::BAD_REQUEST,
            "a retention of {value} should be refused"
        );
    }
}

/// Pruning only after a backup is taken would leave every archive above the new
/// retention on disk, and on screen, until the next scheduled run, which with a
/// 24-hour interval is the next day.
#[tokio::test]
async fn lowering_the_retention_takes_effect_immediately() {
    let (app, dir) = app_with_files("retention-now").await;

    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    for stamp in ["20260101-000000", "20260102-000000", "20260103-000000", "20260104-000000"] {
        std::fs::write(backups.join(format!("routarr-backup-{stamp}.zip")), b"x").unwrap();
    }
    assert_eq!(backup::list(&app.state).len(), 4);

    app.put(
        "/api/v1/settings",
        serde_json::json!({ "settings": { "backup_retention_count": "2" } }),
    )
    .await
    .assert_ok();

    let left: Vec<String> = backup::list(&app.state).into_iter().map(|f| f.name).collect();
    assert_eq!(
        left.len(),
        2,
        "the old archives outlived the setting that should have removed them"
    );
    // Newest kept, oldest gone.
    assert!(left.contains(&"routarr-backup-20260104-000000.zip".to_string()));
    assert!(!left.contains(&"routarr-backup-20260101-000000.zip".to_string()));
}

#[tokio::test]
async fn the_restore_route_stages_and_asks_for_a_restart() {
    let (app, dir) = app_with_files("restore-route").await;
    app.seed_library().await;

    let created = app.post("/api/v1/backups", serde_json::json!({})).await;
    let name = created.assert_ok()["name"].as_str().unwrap().to_string();

    let body = app.post(&format!("/api/v1/backups/{name}/restore"), serde_json::json!({})).await;
    let response = body.assert_ok();
    assert_eq!(response["restart_required"], true, "got {response}");

    // Staged beside the database, never swapped under a live pool.
    assert!(dir.join("routarr.db.restore-pending").exists());

    // A name that is not a backup must not become a path: this route turns a
    // URL segment into a filename, next to the master key.
    let refused = app.post("/api/v1/backups/..%2Froutarr.key/restore", serde_json::json!({})).await;
    assert_ne!(
        refused.status,
        axum::http::StatusCode::OK,
        "a traversal attempt was accepted: {}",
        refused.message()
    );
}

/// An installation whose master key lives in `ROUTARR_SECRET_KEY` has no key
/// file, so its archives carry none. Restoring one elsewhere leaves every
/// sealed Arr credential unreadable, and the manifest is the only thing that
/// can say so before the restart makes it visible.
#[tokio::test]
async fn an_archive_without_the_master_key_says_so() {
    let (app, dir) = app_with_files("no-master-key").await;

    // The shape a key held in the environment leaves on disk: nothing.
    std::fs::remove_file(app.state.config.secret_key_path()).unwrap();

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    let manifest = backup::read_manifest(&archive).unwrap();
    assert!(!manifest.includes_master_key, "the manifest has to admit it");

    // And the restore hands that answer to the caller rather than swallowing
    // it: this is what the interface warns on.
    let staged = backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    assert!(!staged.includes_master_key);
}

/// A backup that fails is retried on its own cadence, not on every tick.
///
/// Stamped on success alone, `last_backup` would bring a backup failing for a
/// reason that will not resolve itself (a full disk, a directory it cannot
/// write) due again at the very next tick. With the default fifteen-minute
/// interval that is ninety-six `VACUUM INTO` a day against SQLite's single
/// writer, and ninety-six failed rows on the screen an operator opens to find
/// out what needs attention.
///
/// `sync` already draws this line: `last_sync_attempt_at` is stamped always,
/// `last_sync_at` only on success. This is the same distinction, for a job
/// that costs far more.
#[tokio::test]
async fn a_backup_that_cannot_be_written_is_not_retried_every_tick() {
    let (app, dir) = app_with_files("failing-cadence").await;

    // A file where the backups directory has to be: `create_dir_all` fails and
    // no amount of retrying will change that.
    std::fs::write(dir.join("backups"), b"not a directory").unwrap();

    set_enabled(&app).await;

    // Cadence state shared across the two ticks, as the loop holds it.
    let mut last_sync = std::collections::HashMap::new();
    let mut last_maintenance = None;
    let mut last_backup = None;
    for _ in 0..2 {
        crate::jobs::scheduler::tick(
            &app.state,
            &mut last_sync,
            &mut last_maintenance,
            &mut last_backup,
        )
        .await
        .expect("a failing backup must not fail the tick");
    }

    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'backup' AND trigger = 'schedule'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1, "a failing backup was attempted {attempts} times in two ticks");
}

/// What worked is stamped as what failed is: a second tick inside every
/// interval syncs nothing again, backs up nothing again and purges nothing
/// again. A success left unstamped would run on every tick.
#[tokio::test]
async fn a_second_tick_inside_every_interval_repeats_nothing_that_worked() {
    let (app, _dir) = app_with_files("working-cadence").await;
    let arr = super::fake_arr::FakeArr::start().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.store_setting("backup_enabled", "true").await;

    let mut last_sync = std::collections::HashMap::new();
    let mut last_maintenance = None;
    let mut last_backup = None;
    for _ in 0..2 {
        crate::jobs::scheduler::tick(
            &app.state,
            &mut last_sync,
            &mut last_maintenance,
            &mut last_backup,
        )
        .await
        .expect("the tick itself must not fail");
    }

    for kind in ["sync", "backup", "maintenance"] {
        let runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = ? AND trigger = 'schedule' AND status = 'success'",
        )
        .bind(kind)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        assert_eq!(runs, 1, "{kind} ran {runs} times in two ticks");
    }
}

/// Backups on, everything else off, so the tick reaches the backup and nothing
/// else writes a job beside it.
async fn set_enabled(app: &TestApp) {
    for (key, value) in [
        ("backup_enabled", "true"),
        ("auto_sync_enabled", "false"),
        ("backup_interval_hours", "24"),
    ] {
        app.store_setting(key, value).await;
    }
}

/// The record a newer release leaves after migrating the database further
/// than this build knows.
pub(super) const AHEAD: &str =
    "INSERT INTO _opened_by (version, schema) VALUES ('99.0.0', '099_from_a_later_release')";

/// One connection to a database file outside the pool, left in the journal
/// mode it has.
async fn open_file(path: &std::path::Path) -> sqlx::SqliteConnection {
    use sqlx::ConnectOptions;
    crate::db::with_paths(sqlx::sqlite::SqliteConnectOptions::new().filename(path))
        .connect()
        .await
        .unwrap()
}

/// An archive a newer release migrated further than this build knows is
/// refused when it is staged, as the start that would apply it refuses it,
/// rather than staged and then refused at every start.
#[tokio::test]
async fn an_archive_a_newer_release_migrated_is_refused_when_staged() {
    let (app, dir) = app_with_files("ahead").await;
    app.execute(&[AHEAD]).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    let refused = backup::stage_restore(&app.state, &file.name, None).await;

    let message = refused.expect_err("the archive was staged").to_string();
    assert!(message.contains("v99.0.0"), "{message}");
    assert!(!dir.join("routarr.db.restore-pending").exists(), "a refused restore left files");
}

/// A pending restore whose database a newer release migrated, as one staged
/// by that release before going back, is set aside at the start rather than
/// applied and then refused at every start.
#[tokio::test]
async fn a_pending_restore_a_newer_release_migrated_is_discarded() {
    let (app, dir) = app_with_files("ahead-pending").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    let pending = dir.join("routarr.db.restore-pending");
    let mut connection = open_file(&pending).await;
    sqlx::query(AHEAD).execute(&mut connection).await.unwrap();
    sqlx::Connection::close(connection).await.unwrap();
    let config = app.state.config.clone();
    app.state.pool.close().await;

    assert!(!backup::apply_pending_restore(&config).await.unwrap(), "the restore was applied");
    assert!(!pending.exists());
}

/// With the server stopped, `routarr restore` stages an archive, named or
/// given by its path, and the next start applies it.
#[tokio::test]
async fn a_restore_can_be_staged_without_the_server() {
    let (app, dir) = app_with_files("offline").await;
    app.seed_library().await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    app.execute(&["DELETE FROM media"]).await;
    let config = (*app.state.config).clone();
    app.state.pool.close().await;

    let path = dir.join("backups").join(&file.name);
    for archive in [file.name.clone(), path.to_string_lossy().into_owned()] {
        let manifest = backup::stage_offline(&config, &archive, None).await.unwrap();
        assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
    }
    let (_, pool, _) = crate::open_storage(&config).await.unwrap();

    let titles: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
    assert_eq!(titles, 1, "the archive was not restored");
}

/// A start refused for a database a newer release migrated names the way
/// back: the newest archive this build can open, and the command restoring it
/// with the server stopped.
#[tokio::test]
async fn a_refused_start_names_the_archive_to_restore() {
    let (app, _dir) = app_with_files("refused-start").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    app.execute(&[AHEAD]).await;
    let config = app.state.config.clone();
    app.state.pool.close().await;

    let refused = crate::db::init_pool(&config).await;

    let message = refused.expect_err("the database was opened").to_string();
    assert!(message.contains(&format!("routarr restore {}", file.name)), "{message}");
}

/// A start that migrates a database takes a backup of it first, at the schema
/// it had: the archives kept afterwards are of the new schema, which the
/// release before cannot open. A first start has nothing to keep.
#[tokio::test]
async fn a_start_that_migrates_takes_a_backup_first() {
    let backups = |dir: &TempDir| -> Vec<String> {
        std::fs::read_dir(dir.join("backups"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let first = TempDir::new("first-start");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(first.join("routarr.db"));
    crate::db::init_pool(&config).await.unwrap().close().await;
    assert_eq!(backups(&first), Vec::<String>::new(), "a first start took a backup");

    let upgraded = TempDir::new("upgrade-start");
    config.set_db_path(upgraded.join("routarr.db"));
    let options = crate::db::with_paths(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&config.db_path).create_if_missing(true),
    );
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    crate::db::run_migrations_through(&pool, "023_constraints_the_code_keeps").await.unwrap();
    pool.close().await;

    crate::db::init_pool(&config).await.unwrap().close().await;

    let taken = backups(&upgraded);
    assert_eq!(taken.len(), 1, "{taken:?}");
    let manifest = backup::read_manifest(&upgraded.join("backups").join(&taken[0])).unwrap();
    assert_eq!(manifest.schema, "023_constraints_the_code_keeps");
}

/// An archive from before the signing secrets were stored keeps today's: their
/// table is made in the staged database, as its migration makes it, or every
/// notification would go out unsigned after the restore.
#[tokio::test]
async fn a_restore_from_before_the_signing_secrets_keeps_todays() {
    let (app, dir) = app_with_files("secrets-table").await;
    app.execute(&["INSERT INTO webhook_secrets (secret, created_at)
                   VALUES ('enc:today', '2026-10-01 00:00:00')"])
        .await;
    let old = dir.join("old.db");
    let options = crate::db::with_paths(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&old).create_if_missing(true),
    );
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    crate::db::run_migrations_through(&pool, "010_job_result").await.unwrap();
    pool.close().await;
    let name = "routarr-backup-20200101-000000.zip";
    std::fs::create_dir_all(dir.join("backups")).unwrap();
    {
        let archive = std::fs::File::create(dir.join("backups").join(name)).unwrap();
        let mut zip = zip::ZipWriter::new(archive);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        let manifest = serde_json::json!({
            "version": "0.1.2", "schema": "010_job_result",
            "created_at": "2020-01-01 00:00:00", "includes_master_key": false
        });
        std::io::Write::write_all(&mut zip, manifest.to_string().as_bytes()).unwrap();
        zip.start_file("routarr.db", options).unwrap();
        std::io::Write::write_all(&mut zip, &std::fs::read(&old).unwrap()).unwrap();
        zip.finish().unwrap();
    }

    backup::stage_restore(&app.state, name, None).await.unwrap();

    let mut staged = open_file(&dir.join("routarr.db.restore-pending")).await;
    let secrets: Vec<String> = sqlx::query_scalar("SELECT secret FROM webhook_secrets")
        .fetch_all(&mut staged)
        .await
        .expect("the staged database has no signing secrets table");
    assert_eq!(secrets, ["enc:today"]);
}
