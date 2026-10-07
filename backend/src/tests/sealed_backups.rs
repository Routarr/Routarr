//! Archives sealed with the backup passphrase: worth nothing without it, and
//! restored with it, by the interface and with the server stopped.

use axum::http::StatusCode;
use serde_json::json;

use super::backup::app_with_files;
use super::{TempDir, TestApp};
use crate::services::backup;

const PASSPHRASE: &str = "a long passphrase for the archives";
const ANOTHER: &str = "another passphrase, chosen later";

/// Set the passphrase as the owner does, and wait for the conversion it
/// starts: a backup taken meanwhile would find the folder held.
async fn set_passphrase(app: &TestApp, passphrase: &str) {
    let before = finished_backups(app).await;
    let body = json!({ "passphrase": passphrase });
    app.put("/api/v1/backups/passphrase", body).await.assert_status(StatusCode::NO_CONTENT);
    finished_backup_tasks(app, before + 1).await;
}

async fn finished_backups(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'backup' AND status != 'running'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

/// The archives in the backup folder, by name.
fn archives(dir: &TempDir) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("backups"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Wait for the pass a passphrase change starts, the `finished`th backup task.
async fn finished_backup_tasks(app: &TestApp, finished: i64) {
    for _ in 0..200 {
        if finished_backups(app).await >= finished {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let jobs: Vec<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT kind, status, detail, error_message FROM jobs")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    panic!("the archives were not brought to the passphrase in time: {jobs:?}");
}

/// An archive copied away opens nothing: not the database, not the master key
/// it carries. Brought to another installation, which holds no passphrase, it
/// is restored once its own is given, and the wrong one is said to be wrong.
#[tokio::test]
async fn an_encrypted_archive_restores_with_its_passphrase_and_not_without() {
    let (app, dir) = app_with_files("sealed-taken").await;
    set_passphrase(&app, PASSPHRASE).await;

    let taken = app.post("/api/v1/backups", json!({})).await.assert_ok().clone();
    let name = taken["name"].as_str().unwrap().to_string();
    assert!(name.ends_with(".zip.enc"), "{taken}");
    assert_eq!(taken["encrypted"], true, "{taken}");
    let bytes = std::fs::read(dir.join("backups").join(&name)).unwrap();
    assert!(bytes.starts_with(b"RTRSEAL\x01"));
    let readable = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);
    assert!(!readable(b"SQLite format 3") && !readable(b"a-master-key"), "it is readable");

    let (elsewhere, other) = app_with_files("sealed-restored").await;
    std::fs::create_dir_all(other.join("backups")).unwrap();
    std::fs::write(other.join("backups").join(&name), &bytes).unwrap();
    let path = format!("/api/v1/backups/{name}/restore");
    let restore = |passphrase: Option<&str>| {
        let body = passphrase.map_or(json!({}), |p| json!({ "passphrase": p }));
        elsewhere.post(&path, body)
    };

    let asked = restore(None).await;
    assert_eq!(asked.status, StatusCode::BAD_REQUEST, "{}", asked.json);
    assert_eq!(asked.json["error"], "passphrase_required");
    assert!(asked.json["message"].as_str().unwrap().contains("Give the passphrase"));
    let wrong = restore(Some("not the passphrase at all")).await;
    assert_eq!(wrong.json["error"], "passphrase_required");
    assert!(wrong.json["message"].as_str().unwrap().contains("does not open"), "{}", wrong.json);

    let staged = restore(Some(PASSPHRASE)).await.assert_ok().clone();
    assert_eq!(staged["restart_required"], true, "{staged}");
    assert_eq!(archives(&other), [name], "the archive opened for the restore was left beside it");
}

/// The installation that took the archive holds its passphrase, and needs no
/// one to type it, with the server running or stopped.
#[tokio::test]
async fn an_archive_the_stored_passphrase_seals_restores_without_asking() {
    let (app, dir) = app_with_files("sealed-stored").await;
    set_passphrase(&app, PASSPHRASE).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    backup::stage_restore(&app.state, &file.name, None).await.unwrap();
    let config = (*app.state.config).clone();
    app.state.pool.close().await;
    backup::stage_offline(&config, &file.name, None).await.unwrap();
    assert_eq!(archives(&dir), [file.name]);
}

/// The archives on disk follow the passphrase: sealed when one is set, sealed
/// again when it changes, so the old one opens none of them, and opened when it
/// is removed.
#[tokio::test]
async fn the_archives_on_disk_follow_the_passphrase() {
    let (app, dir) = app_with_files("sealed-follow").await;
    let plain = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    assert!(!plain.encrypted);

    set_passphrase(&app, PASSPHRASE).await;
    let sealed_name = format!("{}.enc", plain.name);
    assert_eq!(archives(&dir), std::slice::from_ref(&sealed_name));
    let archive = dir.join("backups").join(&sealed_name);
    assert!(backup::sealed_with(&archive, PASSPHRASE), "the passphrase set does not open it");

    set_passphrase(&app, ANOTHER).await;
    assert_eq!(archives(&dir), std::slice::from_ref(&sealed_name));
    assert!(backup::sealed_with(&archive, ANOTHER), "the new passphrase does not open it");
    assert!(!backup::sealed_with(&archive, PASSPHRASE), "the old passphrase still opens it");

    set_passphrase(&app, "").await;
    assert_eq!(archives(&dir), std::slice::from_ref(&plain.name));
    let archive = dir.join("backups").join(&plain.name);
    assert!(backup::read_manifest(&archive).is_ok(), "the archive was not opened back");
}

/// A passphrase set and unreadable, as after the master key changed under
/// it, takes no archive at all: one in the clear in its place would carry
/// the credentials the operator asked to seal.
#[tokio::test]
async fn no_archive_is_taken_in_the_clear_when_the_passphrase_cannot_be_opened() {
    let (app, dir) = app_with_files("sealed-unreadable").await;
    app.store_setting("backup_passphrase", "enc:v1:not-anything-this-key-sealed").await;

    let refused = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await;

    assert!(refused.is_err(), "an archive was taken");
    let left: Vec<String> = std::fs::read_dir(dir.join("backups"))
        .map(|entries| {
            entries.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into()).collect()
        })
        .unwrap_or_default();
    assert_eq!(left, Vec::<String>::new());
}

/// The backup a start takes before it migrates is sealed as every other one,
/// with the passphrase the database holds and the master key on disk.
#[tokio::test]
async fn the_backup_before_a_migration_is_sealed_too() {
    let (app, _dir) = app_with_files("sealed-migrating").await;
    set_passphrase(&app, PASSPHRASE).await;

    let file = backup::before_migrating(&app.state.config, &app.state.pool).await.unwrap();

    assert!(file.encrypted && file.name.ends_with(".zip.enc"), "{file:?}");
}

/// A passphrase guessed offline is guessed at the copier's pace: a short one
/// is refused, it is set on its own route and never through the settings
/// form, and it is never read back.
#[tokio::test]
async fn a_short_passphrase_is_refused_and_none_is_read_back() {
    let (app, _dir) = app_with_files("sealed-short").await;
    let refused = app.put("/api/v1/backups/passphrase", json!({ "passphrase": "short" })).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.json["message"].as_str().unwrap().contains("at least 12"), "{}", refused.json);
    let generic =
        app.put("/api/v1/settings", json!({ "settings": { "backup_passphrase": PASSPHRASE } }));
    assert_eq!(generic.await.status, StatusCode::BAD_REQUEST);

    set_passphrase(&app, PASSPHRASE).await;
    let settings = app.get("/api/v1/settings").await.assert_ok().clone();
    assert_eq!(settings["backup_passphrase_configured"], true, "{settings}");
    assert!(!settings.to_string().contains(PASSPHRASE), "{settings}");
}

/// A start refused for a database a newer release migrated names a sealed
/// archive too, opened with the passphrase that database holds.
#[tokio::test]
async fn a_refused_start_names_a_sealed_archive_it_can_open() {
    let (app, _dir) = app_with_files("sealed-refused-start").await;
    set_passphrase(&app, PASSPHRASE).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    app.execute(&[super::backup::AHEAD]).await;
    let config = app.state.config.clone();
    app.state.pool.close().await;

    let refused = crate::db::init_pool(&config).await;

    let message = refused.expect_err("the database was opened").to_string();
    assert!(message.contains(&format!("routarr restore {}", file.name)), "{message}");
}

/// A sealed archive altered after it was taken is refused as damaged, before
/// anything is staged: its chunks are authenticated one by one.
#[tokio::test]
async fn a_sealed_archive_altered_on_disk_is_refused_as_damaged() {
    let (app, dir) = app_with_files("sealed-damaged").await;
    set_passphrase(&app, PASSPHRASE).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let archive = dir.join("backups").join(&file.name);
    let mut bytes = std::fs::read(&archive).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&archive, bytes).unwrap();

    let refused = app.post(&format!("/api/v1/backups/{}/restore", file.name), json!({})).await;

    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.json);
    assert!(refused.json["message"].as_str().unwrap().contains("damaged"), "{}", refused.json);
    assert_eq!(archives(&dir), [file.name], "something was left beside the archive");
}

/// An encrypted archive read by hand: written opened as a private zip where
/// it is asked, with the passphrase the database in place holds or the one
/// given, and never over a file that exists.
#[tokio::test]
async fn an_encrypted_archive_is_opened_by_hand_into_a_private_zip() {
    let (app, dir) = app_with_files("sealed-by-hand").await;
    set_passphrase(&app, PASSPHRASE).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let config = (*app.state.config).clone();
    app.state.pool.close().await;
    let out = dir.join("read-by-hand.zip").to_string_lossy().into_owned();

    let written = backup::decrypt_offline(&config, &file.name, &out, None).await.unwrap();
    assert!(backup::read_manifest(&written).unwrap().includes_master_key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&written).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the zip in clear is readable by others: {mode:o}");
    }
    let again = backup::decrypt_offline(&config, &file.name, &out, None).await;
    assert!(matches!(again, Err(crate::error::AppError::Conflict(_))), "{again:?}");

    let elsewhere = TempDir::new("sealed-by-hand-elsewhere");
    let mut other = config.clone();
    other.set_db_path(elsewhere.join("routarr.db"));
    let archive = dir.join("backups").join(&file.name).to_string_lossy().into_owned();
    let out = elsewhere.join("opened.zip").to_string_lossy().into_owned();
    let asked = backup::decrypt_offline(&other, &archive, &out, None).await;
    assert!(matches!(asked, Err(crate::error::AppError::PassphraseRequired(_))), "{asked:?}");
    let given = Some(backup::passphrase_of(PASSPHRASE.to_string()));
    backup::decrypt_offline(&other, &archive, &out, given).await.unwrap();
    assert!(backup::read_manifest(std::path::Path::new(&out)).is_ok());
}

/// Changing the passphrase asks the proof a key asks for: removing it opens
/// every archive, and one set by a session left open would seal them for
/// somebody else.
#[tokio::test]
async fn a_session_proves_itself_before_it_sets_or_removes_the_passphrase() {
    let (app, _dir) = super::security::forms_app("sealed-proof").await;
    let password = super::security::generated_password(&app);
    let cookie = super::security::sign_in(&app, &password).await;
    let path = "/api/v1/backups/passphrase";

    let (refused, _) = super::security::with_session(
        &app,
        "PUT",
        path,
        &cookie,
        json!({ "passphrase": PASSPHRASE }),
    )
    .await;
    assert_eq!(refused, StatusCode::FORBIDDEN);
    let proven = json!({ "passphrase": PASSPHRASE, "current_password": password });
    let (set, body) = super::security::with_session(&app, "PUT", path, &cookie, proven).await;
    assert_eq!(set, StatusCode::NO_CONTENT, "{body}");
}

/// Nothing in the clear stays behind a sealed backup: the snapshot and the zip
/// written before the sealing are in a work directory beside the database,
/// private, and gone once the archive is written. The backup folder, the one
/// a sync copies away, only ever holds the sealed archive.
#[tokio::test]
async fn a_sealed_backup_leaves_nothing_in_the_clear_anywhere() {
    let (app, dir) = app_with_files("sealed-clear").await;
    set_passphrase(&app, PASSPHRASE).await;

    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();

    assert_eq!(archives(&dir), [file.name]);
    let work = dir.join(".backup-work");
    let left: Vec<_> = std::fs::read_dir(&work).unwrap().filter_map(Result::ok).collect();
    assert!(left.is_empty(), "the work directory kept {} file(s)", left.len());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&work).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the work directory is open to others: {mode:o}");
    }
}

/// One archive the pass cannot convert does not leave the others as they
/// were, and what a stop interrupted is sealed at the next start.
#[tokio::test]
async fn the_conversion_goes_past_a_broken_archive_and_resumes_after_a_start() {
    let (app, dir) = app_with_files("sealed-resume").await;
    let plain = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let broken = dir.join("backups").join("routarr-backup-20200101-000000.zip.enc");
    std::fs::write(&broken, b"RTRSEAL\x01 and nothing a header holds").unwrap();

    set_passphrase(&app, PASSPHRASE).await;
    let sealed_name = format!("{}.enc", plain.name);
    assert!(archives(&dir).contains(&sealed_name), "{:?}", archives(&dir));
    let detail: String = sqlx::query_scalar(
        "SELECT detail FROM jobs WHERE kind = 'backup' ORDER BY started_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(detail.contains("Left as they were: 1"), "{detail}");

    // A stop between the passphrase and the pass: an archive in the clear.
    let opened = dir.join("backups").join(&plain.name);
    let config = (*app.state.config).clone();
    backup::decrypt_offline(&config, &sealed_name, &dir.join("opened.zip").to_string_lossy(), None)
        .await
        .unwrap();
    std::fs::rename(dir.join("opened.zip"), &opened).unwrap();
    std::fs::remove_file(dir.join("backups").join(&sealed_name)).unwrap();
    let before = finished_backups(&app).await;

    backup::resume_sealing(&app.state).await;

    finished_backup_tasks(&app, before + 1).await;
    assert!(archives(&dir).contains(&sealed_name), "{:?}", archives(&dir));
    assert!(!opened.exists(), "the archive in the clear stayed");
}

/// An archive sealed with a passphrase neither the old nor the new one is
/// stays as it is, counted, rather than being lost or taken for broken.
#[tokio::test]
async fn an_archive_sealed_with_an_older_passphrase_is_left_and_counted() {
    let (app, dir) = app_with_files("sealed-older").await;
    set_passphrase(&app, "the oldest passphrase of all").await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    // The passphrase changed without its pass, as when the pass failed.
    let between = app.state.secrets.seal(PASSPHRASE).unwrap();
    sqlx::query("UPDATE settings SET value = ? WHERE key = 'backup_passphrase'")
        .bind(&between)
        .execute(&app.state.pool)
        .await
        .unwrap();

    set_passphrase(&app, ANOTHER).await;

    assert_eq!(archives(&dir), std::slice::from_ref(&file.name));
    let archive = dir.join("backups").join(&file.name);
    assert!(backup::sealed_with(&archive, "the oldest passphrase of all"));
    let detail: String = sqlx::query_scalar(
        "SELECT detail FROM jobs WHERE kind = 'backup' ORDER BY started_at DESC, rowid DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(detail.contains("Left as they were: 1"), "{detail}");
}

/// A restore keeps today's passphrase, as it keeps today's password: an
/// archive taken before the passphrase was set would otherwise put every
/// later archive back in the clear.
#[tokio::test]
async fn a_restore_keeps_the_passphrase_of_today() {
    let (app, _dir) = app_with_files("sealed-restore-keeps").await;
    let before = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    set_passphrase(&app, PASSPHRASE).await;
    let sealed_name = format!("{}.enc", before.name);

    backup::stage_restore(&app.state, &sealed_name, None).await.unwrap();

    let pending = format!("{}.restore-pending", app.state.config.db_path.display());
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&pending).read_only(true);
    let mut staged = sqlx::ConnectOptions::connect(&options).await.unwrap();
    let kept: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'backup_passphrase'")
            .fetch_optional(&mut staged)
            .await
            .unwrap();
    let kept = kept.expect("the staged database holds no passphrase");
    assert_eq!(app.state.secrets.open(&kept).unwrap(), PASSPHRASE);
}

/// Whether an archive is encrypted is read from it, not from its name, and an
/// archive named as encrypted that is not is refused: in a folder of sealed
/// archives, a zip in the clear under a sealed name is one somebody put there.
#[tokio::test]
async fn an_archive_named_as_encrypted_and_not_is_listed_so_and_refused() {
    let (app, dir) = app_with_files("sealed-named").await;
    let plain = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let posing = format!("{}.enc", plain.name);
    std::fs::rename(dir.join("backups").join(&plain.name), dir.join("backups").join(&posing))
        .unwrap();

    let listed = app.get("/api/v1/backups").await.assert_ok().clone();
    assert_eq!(listed["backups"][0]["name"], posing.as_str(), "{listed}");
    assert_eq!(listed["backups"][0]["encrypted"], false, "{listed}");
    let refused = app.post(&format!("/api/v1/backups/{posing}/restore"), json!({})).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.json);
    assert!(refused.json["message"].as_str().unwrap().contains("named as encrypted"));
}

/// A passphrase the master key in place no longer opens takes no backup, and
/// the warnings say so rather than the settings showing one is set.
#[tokio::test]
async fn a_passphrase_the_master_key_cannot_open_is_a_warning() {
    let (app, _dir) = app_with_files("sealed-warning").await;
    app.store_setting("backup_passphrase", "enc:v1:not-anything-this-key-sealed").await;

    let status = app.get("/api/v1/status").await.assert_ok().clone();

    let codes: Vec<&str> =
        status["warnings"].as_array().unwrap().iter().filter_map(|w| w["code"].as_str()).collect();
    assert!(codes.contains(&"backup_passphrase_unreadable"), "{status}");
}

/// An archive opened by hand is never written into the backup folder, where
/// the server would list it and a sync would copy the master key away.
#[tokio::test]
async fn an_archive_opened_by_hand_is_not_written_into_the_backup_folder() {
    let (app, dir) = app_with_files("sealed-by-hand-folder").await;
    set_passphrase(&app, PASSPHRASE).await;
    let file = backup::create(&app.state, &crate::jobs::Attribution::manual(None)).await.unwrap();
    let config = (*app.state.config).clone();
    let inside = dir.join("backups").join("routarr-backup-20200101-000000.zip");

    let refused =
        backup::decrypt_offline(&config, &file.name, &inside.to_string_lossy(), None).await;

    assert!(matches!(refused, Err(crate::error::AppError::BadRequest(_))), "{refused:?}");
    assert!(!inside.exists());
}
