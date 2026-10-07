//! Archives sealed with the backup passphrase: worth nothing without it, and
//! restored with it, by the interface and with the server stopped.

use age::secrecy::SecretString;
use axum::http::StatusCode;
use serde_json::json;

use super::backup::app_with_files;
use super::{TempDir, TestApp};
use crate::services::backup;

const PASSPHRASE: &str = "a long passphrase for the archives";
const ANOTHER: &str = "another passphrase, chosen later";

async fn set_passphrase(app: &TestApp, passphrase: &str) {
    let body = json!({ "settings": { "backup_passphrase": passphrase } });
    app.put("/api/v1/settings", body).await.assert_ok();
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
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'backup' AND status != 'running'",
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
        if done >= finished {
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

/// Whether `passphrase` opens `archive`, as the `age` tool would.
fn opens(archive: &std::path::Path, passphrase: &str) -> bool {
    let file = std::io::BufReader::new(std::fs::File::open(archive).unwrap());
    let decryptor = age::Decryptor::new_buffered(file).unwrap();
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_string()));
    decryptor.decrypt(std::iter::once(&identity as _)).is_ok()
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
    assert!(name.ends_with(".zip.age"), "{taken}");
    assert_eq!(taken["encrypted"], true, "{taken}");
    let bytes = std::fs::read(dir.join("backups").join(&name)).unwrap();
    assert!(bytes.starts_with(b"age-encryption.org/v1\n"));
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
    finished_backup_tasks(&app, 2).await;
    let sealed_name = format!("{}.age", plain.name);
    assert_eq!(archives(&dir), std::slice::from_ref(&sealed_name));
    let archive = dir.join("backups").join(&sealed_name);
    assert!(opens(&archive, PASSPHRASE), "the passphrase set does not open it");

    set_passphrase(&app, ANOTHER).await;
    finished_backup_tasks(&app, 3).await;
    assert_eq!(archives(&dir), std::slice::from_ref(&sealed_name));
    assert!(opens(&archive, ANOTHER), "the new passphrase does not open it");
    assert!(!opens(&archive, PASSPHRASE), "the old passphrase still opens it");

    set_passphrase(&app, "").await;
    finished_backup_tasks(&app, 4).await;
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

    assert!(file.encrypted && file.name.ends_with(".zip.age"), "{file:?}");
}

/// A passphrase guessed offline is guessed at the copier's pace: a short one
/// is refused, in the words of the screen it is typed on, and it is never
/// read back.
#[tokio::test]
async fn a_short_passphrase_is_refused_and_none_is_read_back() {
    let (app, _dir) = app_with_files("sealed-short").await;
    let short =
        app.put("/api/v1/settings", json!({ "settings": { "backup_passphrase": "short" } }));
    let refused = short.await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.json["message"].as_str().unwrap().contains("Backup passphrase"));

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
