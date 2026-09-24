//! Backups: taking one, keeping the right number, and restoring safely.
//!
//! The property that matters most is not that a zip appears. It is that the
//! archive is **self-sufficient** — a database restored without `routarr.key`
//! opens fine and has no readable Arr credential in it, which is the failure a
//! backup exists to prevent — and that a restore never half-applies.

use std::sync::Arc;

use crate::services::backup;
use crate::state::AppState;

use super::{TempDir, TestApp};

/// A harness backed by a real database file in a throwaway directory.
///
/// Not the usual in-memory pool: a backup is a *file* operation, and
/// `VACUUM INTO` — the whole point of taking a consistent copy without stopping
/// the server — does nothing at all against `:memory:`. Testing it there would
/// prove something that cannot happen in production.
async fn app_with_files(label: &str) -> (TestApp, TempDir) {
    let dir = TempDir(
        std::env::temp_dir().join(format!("routarr-backup-{label}-{}", std::process::id())),
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();

    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    std::fs::write(config.secret_key_path(), "a-master-key").unwrap();
    std::fs::write(config.api_key_path(), "an-api-key").unwrap();

    let pool = crate::db::init_pool(&config).await.unwrap();
    let secrets = crate::crypto::SecretBox::load(
        config.secret_key.as_deref(),
        None,
        &config.secret_key_path(),
    )
    .unwrap();

    let state = AppState {
        http: crate::http::build_client(&config).expect("test http client"),
        secrets,
        tvdb_token: Arc::new(tokio::sync::Mutex::new(None)),
        jobs: crate::jobs::JobRegistry::new(pool.clone()),
        api_key: Arc::new(std::sync::RwLock::new(config.api_key.clone())),
        sign_in: Arc::new(Default::default()),
        oidc_provider: Arc::new(tokio::sync::RwLock::new(None)),
        config: Arc::new(config),
        pool,
    };

    (TestApp::around(state), dir)
}

fn entries(archive: &std::path::Path) -> Vec<String> {
    let file = std::fs::File::open(archive).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect()
}

#[tokio::test]
async fn a_backup_carries_everything_a_restore_needs() {
    let (app, dir) = app_with_files("complete").await;

    let file = backup::create(&app.state, "manual").await.unwrap();
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_snapshot_is_a_real_database_taken_without_stopping() {
    let (app, dir) = app_with_files("snapshot").await;
    app.seed_library().await;

    let file = backup::create(&app.state, "manual").await.unwrap();
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

    std::fs::remove_dir_all(&dir).ok();
}

/// A retention count stored above the ceiling this build enforces is honoured
/// as it is: lowering it removes archives, and nothing but the operator's own
/// save may do that. Named in the warnings until then, since the screen
/// refuses to save it as it stands.
#[tokio::test]
async fn a_retention_count_above_the_maximum_is_kept_until_the_operator_lowers_it() {
    let (app, dir) = app_with_files("retention-ceiling").await;

    sqlx::query("INSERT INTO settings (key, value) VALUES ('backup_retention_count', '200')")
        .execute(&app.state.pool)
        .await
        .unwrap();
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
    let named = status["warnings"]
        .as_array()
        .map(|w| {
            w.iter().any(|w| w.as_str().unwrap_or_default().contains("backup_retention_count"))
        })
        .unwrap_or(false);
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn only_the_retained_count_survives_a_prune() {
    let (app, dir) = app_with_files("retention").await;

    sqlx::query("INSERT INTO settings (key, value) VALUES ('backup_retention_count', '2')")
        .execute(&app.state.pool)
        .await
        .unwrap();

    // The name carries a second-resolution timestamp, so three in the same
    // second would collide; they are written directly instead.
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_backup_from_a_newer_schema_is_refused_rather_than_half_applied() {
    let (app, dir) = app_with_files("schema").await;

    let file = backup::create(&app.state, "manual").await.unwrap();
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

    let outcome = backup::stage_restore(&app.state, "routarr-backup-99999999-000000.zip").await;
    let message = outcome.expect_err("a newer schema must be refused").to_string();
    assert!(message.contains("Upgrade Routarr first"), "unhelpful refusal: {message}");

    // And nothing was staged: a refusal that still wrote files would be worse
    // than no check at all.
    let staged = dir.join("routarr.db.restore-pending");
    assert!(!staged.exists(), "a refused restore left files behind");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_restore_is_staged_and_applied_only_at_the_next_start() {
    let (app, dir) = app_with_files("restore").await;
    app.seed_library().await;

    let file = backup::create(&app.state, "manual").await.unwrap();

    // Change the live database after the backup, so a successful restore is
    // observable rather than a no-op.
    sqlx::query("DELETE FROM media").execute(&app.state.pool).await.unwrap();

    backup::stage_restore(&app.state, &file.name).await.unwrap();

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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn nothing_is_applied_when_no_restore_is_pending() {
    let (app, dir) = app_with_files("nopending").await;
    assert!(!backup::apply_pending_restore(&app.state.config).await.unwrap());
    std::fs::remove_dir_all(&dir).ok();
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
    let file = backup::create(&app.state, "manual").await.unwrap();
    damage_entry(&dir.join("backups").join(&file.name), "routarr.db");

    let refused = backup::stage_restore(&app.state, &file.name)
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn an_archive_whose_database_is_not_one_is_refused_before_anything_is_staged() {
    let (app, dir) = app_with_files("empty-db").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();
    // A well-formed archive whose database entry is empty: its CRC is right,
    // its manifest reads, and restoring it would start the next run on an
    // empty schema.
    forge(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.db",
        b"",
    );

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip")
        .await
        .expect_err("an archive holding no database must be refused");

    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");
    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");

    std::fs::remove_dir_all(&dir).ok();
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

    std::fs::remove_dir_all(&dir).ok();
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn staging_a_second_backup_replaces_the_first_entirely() {
    let (app, dir) = app_with_files("restage").await;
    let backups = dir.join("backups");

    let first = backup::create(&app.state, "manual").await.unwrap();
    // Two archives taken in one second would share a name.
    std::fs::rename(backups.join(&first.name), backups.join("routarr-backup-20000101-000000.zip"))
        .unwrap();
    backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip").await.unwrap();
    assert!(
        dir.join("routarr.api_key.restore-pending").exists(),
        "precondition: the first archive carries the API key"
    );

    std::fs::remove_file(app.state.config.api_key_path()).unwrap();
    let second = backup::create(&app.state, "manual").await.unwrap();
    assert!(
        !entries(&backups.join(&second.name)).contains(&"routarr.api_key".to_string()),
        "precondition: the second archive carries no API key"
    );
    backup::stage_restore(&app.state, &second.name).await.unwrap();

    assert!(
        !dir.join("routarr.api_key.restore-pending").exists(),
        "the first archive's API key would be restored beside the second archive's database"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `integrity_check` is what finds damage inside a database whose first pages
/// read: the migrations table answers, and a page further in is garbage that
/// the first query to reach it after the restart reports as a malformed file.
#[tokio::test]
async fn a_database_damaged_inside_is_refused_before_anything_is_staged() {
    let (app, dir) = app_with_files("damaged-inside").await;
    app.seed_library().await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();

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

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip")
        .await
        .expect_err("a database damaged inside must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");

    std::fs::remove_dir_all(&dir).ok();
}

/// A key entry the archive holds and cannot read is a damaged archive, not an
/// archive without a key. Taken for an absent one, the restore goes ahead
/// without it while the manifest says the key is included, and after the
/// restart no Arr credential opens.
#[tokio::test]
async fn a_key_the_archive_cannot_read_is_refused_rather_than_left_out() {
    let (app, dir) = app_with_files("damaged-key").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();
    let archive = backups.join(&file.name);

    // The entry's local header, which the reader checks before its data.
    let header = {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive).unwrap()).unwrap();
        zip.by_name("routarr.key").unwrap().header_start()
    };
    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[usize::try_from(header).unwrap()] ^= 0xFF;
    std::fs::write(&archive, bytes).unwrap();

    let refused = backup::stage_restore(&app.state, &file.name)
        .await
        .expect_err("a key that cannot be read must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");

    std::fs::remove_dir_all(&dir).ok();
}

/// The manifest says whether the master key travelled. An archive that says
/// it did and does not carry it has lost it, and restoring its database under
/// the current key leaves every sealed credential unreadable.
#[tokio::test]
async fn an_archive_missing_the_key_its_manifest_lists_is_refused() {
    let (app, dir) = app_with_files("missing-key").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();
    rewrite(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.key",
        None,
    );

    let refused = backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip")
        .await
        .expect_err("an archive that lost its master key must be refused");

    assert!(matches!(refused, crate::error::AppError::BadRequest(_)), "{refused}");
    assert_eq!(restore_leftovers(&dir), Vec::<String>::new(), "a refused restore left files");

    std::fs::remove_dir_all(&dir).ok();
}

/// A restore that is refused replaces nothing. Discarded before the second
/// archive is read, the first one is lost, and the operator hears only about
/// the second.
#[tokio::test]
async fn a_refused_restore_leaves_the_one_already_staged() {
    let (app, dir) = app_with_files("refused-second").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();
    backup::stage_restore(&app.state, &file.name).await.unwrap();

    forge(
        &backups.join(&file.name),
        &backups.join("routarr-backup-20000101-000000.zip"),
        "routarr.db",
        b"",
    );
    backup::stage_restore(&app.state, "routarr-backup-20000101-000000.zip")
        .await
        .expect_err("precondition: the second archive is refused");

    let mut left = restore_leftovers(&dir);
    left.sort();
    assert_eq!(
        left,
        [
            "routarr.api_key.restore-pending",
            "routarr.db.restore-pending",
            "routarr.key.restore-pending"
        ],
        "the restore staged first did not survive a refused one"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Two stagings write the same files. A request dropped by a proxy leaves its
/// staging running on a blocking thread, and a retry beside it would have the
/// first one remove, or overwrite, what the second has just checked.
#[tokio::test]
async fn a_restore_is_staged_one_at_a_time() {
    let (app, dir) = app_with_files("one-at-a-time").await;
    let file = backup::create(&app.state, "manual").await.unwrap();

    let held = app.state.jobs.try_lock("restore").expect("the restore lock");
    let refused = backup::stage_restore(&app.state, &file.name)
        .await
        .expect_err("a second staging must wait for the first");
    assert!(matches!(refused, crate::error::AppError::Conflict(_)), "{refused}");
    drop(held);

    backup::stage_restore(&app.state, &file.name).await.unwrap();

    std::fs::remove_dir_all(&dir).ok();
}

/// A newer binary may stage a restore and an older one make the next start.
/// The migrations only go forward, so the older one would run against a schema
/// it does not know.
#[tokio::test]
async fn a_pending_database_from_a_newer_schema_is_not_applied() {
    let (app, dir) = app_with_files("newer-pending").await;
    app.seed_library().await;
    let file = backup::create(&app.state, "manual").await.unwrap();
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

    std::fs::remove_dir_all(&dir).ok();
}

/// A process killed mid-backup or mid-staging leaves files no later pass takes
/// away: a copy of the whole database, an archive holding the master key in
/// clear, a staged database. Nothing else runs at a start, so that is where
/// they go.
#[tokio::test]
async fn what_an_interrupted_run_leaves_is_swept_at_the_next_start() {
    let (app, dir) = app_with_files("sweep").await;
    let backups = dir.join("backups");
    let file = backup::create(&app.state, "manual").await.unwrap();
    let config = app.state.config.clone();
    app.state.pool.close().await;

    for leftover in [
        backups.join(".routarr-backup-20260101-000000.zip.partial"),
        backups.join(".20260101-000000.db"),
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

    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------------- API

#[tokio::test]
async fn the_api_takes_lists_and_deletes_a_backup() {
    let (app, dir) = app_with_files("api").await;

    let created = app.post("/api/v1/backups", serde_json::json!({})).await;
    let name = created.assert_ok()["name"].as_str().unwrap().to_string();

    let listed = app.get("/api/v1/backups").await;
    let body = listed.assert_ok();
    assert_eq!(body["backups"].as_array().unwrap().len(), 1);
    assert_eq!(body["retention_count"], 7);

    // The download is the point of the route, and only its refusals were
    // exercised: the bytes have to be the archive, typed as one.
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

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_download_cannot_walk_out_of_the_backup_directory() {
    let (app, dir) = app_with_files("traversal").await;

    // The archives sit in the same directory as the master key, so this is the
    // one place a caller picks a path on the server's filesystem.
    for hostile in [
        "..%2F..%2Froutarr.key",
        "routarr-backup-..%2F..%2Froutarr.key.zip",
        "routarr.key",
        "routarr.db",
    ] {
        let response = app.get(&format!("/api/v1/backups/{hostile}")).await;
        assert_eq!(response.status, axum::http::StatusCode::NOT_FOUND, "{hostile} was not refused");
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------- what bounds the list

/// The interface renders every backup it is given, so the retention count is
/// what stops that card growing without end. As a plain positive integer it
/// accepts "keep 10 000", and the disk fills with the very thing meant to
/// protect it.
#[tokio::test]
async fn the_retention_count_is_bounded_on_both_sides() {
    let (app, dir) = app_with_files("retention-bounds").await;

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

    std::fs::remove_dir_all(&dir).ok();
}

/// Pruning only after a backup is taken would leave every archive above the new
/// retention on disk — and on screen — until the next scheduled run, which with
/// a 24-hour interval is the next day.
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

    std::fs::remove_dir_all(&dir).ok();
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
/// sealed Arr credential unreadable — and the manifest is the only thing that
/// can say so before the restart makes it visible.
#[tokio::test]
async fn an_archive_without_the_master_key_says_so() {
    let (app, dir) = app_with_files("no-master-key").await;

    // The shape a key held in the environment leaves on disk: nothing.
    std::fs::remove_file(app.state.config.secret_key_path()).unwrap();

    let file = backup::create(&app.state, "manual").await.unwrap();
    let archive = dir.join("backups").join(&file.name);

    let manifest = backup::read_manifest(&archive).unwrap();
    assert!(!manifest.includes_master_key, "the manifest has to admit it");

    // And the restore hands that answer to the caller rather than swallowing
    // it: this is what the interface warns on.
    let staged = backup::stage_restore(&app.state, &file.name).await.unwrap();
    assert!(!staged.includes_master_key);

    std::fs::remove_dir_all(&dir).ok();
}

/// A backup that fails is retried on its own cadence, not on every tick.
///
/// `last_backup` was stamped on success alone, so a backup failing for a reason
/// that will not resolve itself — a full disk, a directory it cannot write —
/// came due again at the very next tick. With the default fifteen-minute
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
    let mut chain = None;
    for _ in 0..2 {
        crate::jobs::scheduler::tick(
            &app.state,
            &mut last_sync,
            &mut last_maintenance,
            &mut last_backup,
            &mut chain,
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

/// Backups on, everything else off, so the tick reaches the backup and nothing
/// else writes a job beside it.
async fn set_enabled(app: &TestApp) {
    for (key, value) in [
        ("backup_enabled", "true"),
        ("auto_sync_enabled", "false"),
        ("backup_interval_hours", "24"),
    ] {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
}
