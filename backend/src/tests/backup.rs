//! Backups: taking one, keeping the right number, and restoring safely. An archive carries its
//! master key, without which no restored credential opens, and a restore never half-applies.

use crate::localization::Localizer;
use crate::services::backup;
use crate::state::AppState;

use super::{TempDir, TestApp, warning_messages};

/// A harness on a real database file in a throwaway directory, since `VACUUM INTO` writes
/// nothing against `:memory:`.
pub(crate) async fn app_with_files(label: &str) -> (TestApp, TempDir) {
    let dir = TempDir::new(&format!("backup-{label}"));

    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    // The key file seals the data, as on an installation without `ROUTARR_SECRET_KEY`.
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
    (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().unwrap().to_string()).collect()
}

/// With the master key in `ROUTARR_SECRET_KEY`, a stale `routarr.key` stays out of the archive,
/// and the manifest says it carries no key.
#[tokio::test]
async fn a_key_from_the_environment_is_not_claimed_by_the_archive() {
    let (app, dir) = app_with_files("environment-key").await;
    let mut config = (*app.state.config).clone();
    config.secret_key = Some("the-key-in-the-environment".to_string());
    let app = TestApp::around(app.state.clone().with_config(config));

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);
    assert!(!backup::read_manifest(&archive, &Localizer::new("en")).unwrap().includes_master_key);
    assert!(!entries(&archive).contains(&"routarr.key".to_string()), "a stale key file travelled");
}

#[tokio::test]
async fn a_backup_carries_everything_a_restore_needs() {
    let (app, dir) = app_with_files("complete").await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);
    let names = entries(&archive);

    // Without the master key, no Arr credential in the database decrypts.
    assert!(names.contains(&"routarr.db".to_string()));
    assert!(names.contains(&"routarr.key".to_string()));
    assert!(names.contains(&"routarr.api_key".to_string()));
    assert!(names.contains(&"manifest.json".to_string()));

    let manifest = backup::read_manifest(&archive, &Localizer::new("en")).unwrap();
    assert!(manifest.includes_master_key);
    assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn the_snapshot_is_a_real_database_taken_without_stopping() {
    let (app, dir) = app_with_files("snapshot").await;
    app.seed_library().await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    // A `VACUUM INTO` snapshot is a self-contained database, the WAL folded in.
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

/// The maintenance pass folds the WAL into the database file, so a copy of `routarr.db` taken
/// while Routarr runs holds what was written before the pass.
#[tokio::test]
async fn a_maintenance_pass_folds_the_log_into_the_database() {
    let (app, dir) = app_with_files("checkpoint").await;
    app.execute(&["INSERT INTO categories (id, name) VALUES ('c-1', 'anime')"]).await;

    crate::services::maintenance::run(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let copy = dir.join("copy.db");
    std::fs::copy(dir.join("routarr.db"), &copy).unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", copy.display())).await.unwrap();
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&pool).await.unwrap();
    assert!(names.contains(&"anime".to_string()), "the copy holds {names:?}");
    pool.close().await;
}

/// A retention count above the ceiling stays until the operator's own save lowers it, since
/// lowering it removes archives. The warnings name it by its label meanwhile.
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
    let warnings = warning_messages(&status);
    let named = warnings.iter().any(|w| w.contains("'Backups kept' is 200"));
    assert!(named, "the warnings do not name the setting by its label: {status}");
    let raw = warnings.iter().any(|w| w.contains("backup_retention_count"));
    assert!(!raw, "a warning names the setting by its key: {status}");

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

    // Written by hand, since three backups taken in one second would share a name.
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
        let options = zip::write::SimpleFileOptions::default();
        for i in 0..source.len() {
            let mut entry = source.by_index(i).unwrap();
            let name = entry.name().unwrap().to_string();
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
    assert!(message.contains("Upgrade Routarr, then restore"), "unhelpful refusal: {message}");

    let staged = dir.join("routarr.db.restore-pending");
    assert!(!staged.exists(), "a refused restore left files behind");
}

#[tokio::test]
async fn a_restore_is_staged_and_applied_only_at_the_next_start() {
    let (app, dir) = app_with_files("restore").await;
    app.seed_library().await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    // Changed after the backup, so the restore is observable.
    sqlx::query("DELETE FROM media").execute(&app.state.pool).await.unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();

    // Only staged: the pool is still open on the old file.
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
    // The backup's own job is in the archive as a success, never as one left running.
    let interrupted = crate::jobs::JobRegistry::new(pool.clone()).recover_orphans().await.unwrap();
    assert_eq!(interrupted, 0, "the restored database holds its backup as still running");
    let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE kind = 'backup'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "success");
    pool.close().await;
}

/// A key revoked since the backup stays revoked, and the signing secrets stay today's.
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

/// The master API key or a signing secret withdrawn since the backup stays withdrawn, and the
/// record travels. A new host, or one whose replacements were lost, gets the archive's. A key
/// `ROUTARR_API_KEY` overrides is not brought back.
#[tokio::test]
async fn a_restore_brings_back_no_key_or_secret_withdrawn_since() {
    use crate::services::notify;

    // (case, key file restored, secrets restored)
    for (case, expected) in [
        ("withdrawn", (false, false)),
        ("new-host", (true, true)),
        ("replaced-then-lost", (true, true)),
        ("pinned", (false, true)),
    ] {
        let (app, _dir) = app_with_files(case).await;
        notify::rotate_signing_secret(&app.state).await.unwrap();
        let file =
            backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
        if matches!(case, "withdrawn" | "replaced-then-lost") {
            app.state.clear_api_key().await.unwrap();
            notify::remove_signing_secrets(&app.state).await.unwrap();
        }
        if case == "replaced-then-lost" {
            app.state.rotate_api_key().await.unwrap();
            notify::rotate_signing_secret(&app.state).await.unwrap();
        }
        if case != "withdrawn" {
            std::fs::remove_file(app.state.config.api_key_path()).unwrap();
            app.execute(&["DELETE FROM webhook_secrets"]).await;
        }
        let app = if case == "pinned" {
            let mut config = (*app.state.config).clone();
            config.api_key = Some("a-pinned-key".into());
            TestApp::around(app.state.clone().with_config(config))
        } else {
            app
        };

        backup::stage_restore(&app.state, &file.name, None).await.unwrap();
        let config = app.state.config.clone();
        app.state.pool.close().await;
        assert!(backup::apply_pending_restore(&config).await.unwrap());

        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", config.db_path.display()))
            .await
            .unwrap();
        let secrets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_secrets")
            .fetch_one(&pool)
            .await
            .unwrap();
        let restored = (config.api_key_path().exists(), secrets > 0);
        assert_eq!(restored, expected, "{case}");
        if case == "withdrawn" {
            let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM withdrawn_credentials")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(recorded, 2, "the restored database forgot what was withdrawn");
        }
        pool.close().await;
    }
}

/// A restore writes the withdrawals table into an archive older than it, and the next start's
/// migration still applies.
#[tokio::test]
async fn an_archive_given_the_withdrawals_ahead_of_its_migration_still_upgrades() {
    let pool = super::database_of_0_1_5().await;
    sqlx::raw_sql(crate::services::backup::WITHDRAWN_CREDENTIALS_TABLE)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO withdrawn_credentials (name) VALUES ('signing_secret')")
        .execute(&pool)
        .await
        .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM withdrawn_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 1, "the migration replaced the record the restore wrote");
}

/// A restore keeps today's master API key and password, and closes every session the archive
/// held.
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

/// A restore onto a new host brings back the archive's key file, and its credentials open.
/// Counting rows would pass with the key left behind.
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

/// With `ROUTARR_SECRET_KEY` set, which wins over a restored `routarr.key`, an archive sealed
/// under its key file is refused at staging, naming the variable.
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

/// Without the variable, an archive whose own key file does not open its credentials is
/// refused too.
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

/// The download, delete and restore routes each refuse a name that climbs out of the backup
/// folder, and leave the file it names untouched.
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

/// A start applies a staged restore before opening anything, so the database and the master
/// key it opens are the archive's.
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

/// Every file a start writes beside the database holds a secret or the data, so each is
/// readable by its owner alone.
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

    assert!(
        matches!(
            refused,
            Err(crate::error::AppError::InProgress { reason: "backup_just_taken", .. })
        ),
        "{refused:?}"
    );
    let left: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "a copy was written for nothing");
}

/// A restore sets the database it replaces aside as `routarr.db.pre-restore`, so picking the
/// wrong archive loses nothing written since.
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

/// Flip one byte in the middle of an entry's compressed data, which the CRC finds only once
/// part of the entry is copied out.
pub(crate) fn damage_entry(archive: &std::path::Path, entry: &str) {
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
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..source.len() {
        let mut file = source.by_index(i).unwrap();
        let name = file.name().unwrap().to_string();
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

    // What is copied before the damage is found would replace the live database.
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
    // An empty database entry: its CRC is right and its manifest reads.
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

    // A pending set whose database is damaged after staging.
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

    // Keys pending alone are an interrupted staging, and would lock the live database away.
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

/// A database whose first pages read and whose later page is garbage is refused at staging,
/// since `integrity_check` finds it.
#[tokio::test]
async fn a_database_damaged_inside_is_refused_before_anything_is_staged() {
    let (app, dir) = app_with_files("damaged-inside").await;
    app.seed_library().await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    // The media table's root page, given a page type SQLite does not have.
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

/// A key entry the archive holds and cannot read is refused as damage: taken for an absent
/// key, the restore would leave no Arr credential readable.
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

/// An archive missing the master key its manifest lists is refused: restored under the
/// current key, no sealed credential would open.
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

/// A refused restore leaves the one already staged in place.
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

/// Stagings write the same files, so a second waits for the first, which runs on after a
/// proxy drops its request.
#[tokio::test]
async fn a_restore_is_staged_one_at_a_time() {
    let (app, _dir) = app_with_files("one-at-a-time").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    let held = app.state.jobs.try_lock("restore").expect("the restore lock");
    let refused = backup::stage_restore(&app.state, &file.name, None)
        .await
        .expect_err("a second staging must wait for the first");
    assert!(
        matches!(refused, crate::error::AppError::InProgress { reason: "restore_staging", .. }),
        "{refused}"
    );
    drop(held);

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
}

/// A database pending from a newer schema, as a newer binary may stage before a downgrade, is
/// not applied.
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

/// A start sweeps what a killed backup or staging leaves, database copies and the master key
/// in the clear, and nothing else.
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

/// A backup whose caller leaves still writes its archive, records it on its task and holds
/// the lock until then.
#[tokio::test]
async fn a_backup_whose_caller_leaves_still_records_its_outcome() {
    let (app, dir) = app_with_files("caller-leaves").await;
    let request = axum::http::Request::post("/api/v1/backups")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let held = super::races::left_at(&app, "backup::create", request).await;
    let meanwhile = app.post("/api/v1/backups", serde_json::json!({})).await;
    assert_eq!(
        meanwhile.assert_status(axum::http::StatusCode::CONFLICT)["reason"],
        "backup_running"
    );
    held.release();

    let task: String = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'backup'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let task = super::finished(&app, &task).await;
    assert_eq!(task["status"], "success", "{task}");
    assert_eq!(std::fs::read_dir(dir.join("backups")).unwrap().count(), 1);
    assert!(task["result"]["name"].is_string(), "the task keeps no report: {task}");
}

#[tokio::test]
async fn the_api_takes_lists_and_deletes_a_backup() {
    let (app, dir) = app_with_files("api").await;

    let created = app.post("/api/v1/backups", serde_json::json!({})).await;
    let name = created.assert_ok()["name"].as_str().unwrap().to_string();

    // Dated by the stamp in its name, since a copy of the folder resets the file's own date.
    let file = std::fs::File::options().write(true).open(dir.join("backups").join(&name)).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200)).unwrap();
    let stamp = name.trim_start_matches("routarr-backup-").trim_end_matches(".zip");
    let stamped = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").unwrap().and_utc();

    let listed = app.get("/api/v1/backups").await;
    let body = listed.assert_ok();
    assert_eq!(body["backups"].as_array().unwrap().len(), 1);
    assert_eq!(body["retention_count"], 7);
    let listed_at = body["backups"][0]["created_at"].as_str().unwrap();
    assert_eq!(listed_at, stamped.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    // Asked as a browser would: gzipped again, an archive loses the length progress reads.
    use http_body_util::BodyExt;
    let request = axum::http::Request::get(format!("/api/v1/backups/{name}"))
        .header("accept-encoding", "gzip, deflate, br")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.send_raw(request).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(response.headers().get("content-encoding"), None);
    let size = std::fs::metadata(dir.join("backups").join(&name)).unwrap().len();
    assert_eq!(response.headers()["content-length"], size.to_string().as_str());
    let document = axum::http::Request::get("/api/v1/openapi.json")
        .header("accept-encoding", "gzip")
        .body(axum::body::Body::empty())
        .unwrap();
    let document = app.send_raw(document).await;
    assert_eq!(document.headers()["content-encoding"], "gzip", "nothing is compressed any more");
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
    assert_eq!(deleted.assert_ok(), &serde_json::json!({ "deleted": true }));
    assert!(backup::list(&app.state).is_empty());
}

// ------------------------------------------------- what bounds the list

/// A lowered retention prunes at the save, without waiting for the next scheduled backup.
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

    // This route turns a URL segment into a file name, next to the master key.
    let refused = app.post("/api/v1/backups/..%2Froutarr.key/restore", serde_json::json!({})).await;
    assert_ne!(
        refused.status,
        axum::http::StatusCode::OK,
        "a traversal attempt was accepted: {}",
        refused.message()
    );
}

/// An archive taken without a key file says so in its manifest, and the restore returns it:
/// it is the only warning before the restart.
#[tokio::test]
async fn an_archive_without_the_master_key_says_so() {
    let (app, dir) = app_with_files("no-master-key").await;

    // The shape a key held in the environment leaves on disk: nothing.
    std::fs::remove_file(app.state.config.secret_key_path()).unwrap();

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    let manifest = backup::read_manifest(&archive, &Localizer::new("en")).unwrap();
    assert!(!manifest.includes_master_key, "the manifest has to admit it");

    // The interface warns on what the restore returns.
    let staged = backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    assert!(!staged.includes_master_key);
}

/// A failing backup waits for its interval like a working one: stamped on success alone,
/// `last_backup` would retry it at every tick.
#[tokio::test]
async fn a_backup_that_cannot_be_written_is_not_retried_every_tick() {
    let (app, dir) = app_with_files("failing-cadence").await;

    // A file where the backups directory goes, so every attempt fails.
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

/// A success is stamped as a failure is, or it would run again at every tick.
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

/// An archive a newer release migrated is refused at staging, as the start applying it would
/// refuse it.
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

/// A pending restore whose database a newer release migrated is discarded at the start.
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
        crate::commands::restore(&config, Some(archive)).await.unwrap();
    }
    let (_, pool, _) = crate::open_storage(&config).await.unwrap();

    let titles: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
    assert_eq!(titles, 1, "the archive was not restored");
}

/// A start refused for a database a newer release migrated names the newest archive this
/// build can open, and the command restoring it.
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

/// A start that migrates a database backs it up first, at the schema it had. A first start
/// has nothing to back up.
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
    crate::db::run_migrations_through(&pool, "001_release_0_1_5").await.unwrap();
    super::left_by_0_1_5(&pool).await;
    pool.close().await;

    crate::db::init_pool(&config).await.unwrap().close().await;

    let taken = backups(&upgraded);
    assert_eq!(taken.len(), 1, "{taken:?}");
    let manifest =
        backup::read_manifest(&upgraded.join("backups").join(&taken[0]), &Localizer::new("en"))
            .unwrap();
    assert_eq!(manifest.schema, "013_opened_by", "the archive is not of the schema 0.1.5 left");
}

/// An archive older than this build's first migration is refused, naming the release to go
/// through, and nothing is staged.
#[tokio::test]
async fn an_archive_from_before_0_1_5_is_refused_naming_the_release_to_go_through() {
    let (app, dir) = app_with_files("before-0-1-5").await;
    let name = "routarr-backup-20200101-000000.zip";
    std::fs::create_dir_all(dir.join("backups")).unwrap();
    {
        let archive = std::fs::File::create(dir.join("backups").join(name)).unwrap();
        let mut zip = zip::ZipWriter::new(archive);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        let manifest = serde_json::json!({
            "version": "0.1.2", "schema": "010_job_result",
            "created_at": "2020-01-01 00:00:00", "includes_master_key": false
        });
        std::io::Write::write_all(&mut zip, manifest.to_string().as_bytes()).unwrap();
        zip.start_file("routarr.db", options).unwrap();
        std::io::Write::write_all(&mut zip, b"never read").unwrap();
        zip.finish().unwrap();
    }

    let refused = backup::stage_restore(&app.state, name, None)
        .await
        .expect_err("an archive 0.1.2 took was staged");

    assert!(refused.to_string().contains("0.1.5"), "{refused}");
    assert!(!dir.join("routarr.db.restore-pending").exists(), "something was staged");
}

/// No archive for two intervals raises a warning. A recent archive, or an installation too
/// young to have missed one, raises none.
#[tokio::test]
async fn a_backup_missed_for_two_intervals_is_warned_of() {
    async fn overdue(app: &TestApp) -> bool {
        let body = app.get("/api/v1/status").await;
        let warnings = body.assert_ok()["warnings"].as_array().unwrap().clone();
        warnings.iter().any(|warning| warning["code"] == "backup_overdue")
    }
    let (app, dir) = app_with_files("overdue").await;
    assert!(!overdue(&app).await, "a new installation was warned of a missed backup");

    std::fs::create_dir_all(dir.join("backups")).unwrap();
    std::fs::write(dir.join("backups/routarr-backup-20200101-000000.zip"), b"").unwrap();
    assert!(overdue(&app).await, "an archive years old was not warned of");

    let recent = chrono::Utc::now() - chrono::Duration::hours(30);
    let name = format!("routarr-backup-{}.zip", recent.format("%Y%m%d-%H%M%S"));
    std::fs::write(dir.join("backups").join(name), b"").unwrap();
    assert!(!overdue(&app).await, "an archive inside two intervals was warned of");
}

/// The disk is low under twice the database and 100 MiB: what a backup of it
/// and the database's own growth need.
#[test]
fn the_data_disk_is_low_under_twice_the_database_and_a_margin() {
    let margin = 100 * 1024 * 1024;
    assert!(!crate::api::health::disk_is_low(2 * 500 + margin, 500));
    assert!(crate::api::health::disk_is_low(2 * 500 + margin - 1, 500));
    assert!(crate::api::health::disk_is_low(margin - 1, 0));
}
