//! The copy of each archive into `ROUTARR_BACKUP_COPY_DIR`: whole, private, level with the
//! backup folder, and never on the way of a backup when the share sleeps or hangs.

use axum::http::StatusCode;
use serde_json::json;

use super::backup::app_with_files;
use super::{TempDir, TestApp};
use crate::jobs::Attribution;
use crate::services::backup;

const MARKER: &str = ".routarr-backup-copy";

/// A harness whose archives are copied into a folder of the test's own, which `copy_folder`
/// creates first when asked.
async fn app_copying(label: &str, copy_folder: bool) -> (TestApp, TempDir, TempDir) {
    let (app, data) = app_with_files(&format!("copy-{label}")).await;
    let copies = TempDir::new(&format!("copies-{label}"));
    if !copy_folder {
        std::fs::remove_dir(&copies.0).unwrap();
    }
    let mut config = (*app.state.config).clone();
    config.backup_copy_dir = Some(copies.0.clone());
    (TestApp::around(app.state.clone().with_config(config)), data, copies)
}

/// The copy tasks once `count` have ended, newest first, with their detail keys.
async fn copies_ended(app: &TestApp, count: i64) -> Vec<(String, Option<String>)> {
    let ended = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'backup_copy' AND status != 'running'",
        )
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
            >= count
    };
    super::until(ended).await;
    sqlx::query_as(
        "SELECT status, detail_key FROM jobs WHERE kind = 'backup_copy'
          ORDER BY started_at DESC, rowid DESC",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap()
}

/// The archives in `dir`, by name, the marker and leftovers left out.
fn archives_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| backup::is_valid_backup_name(name))
        .collect();
    names.sort();
    names
}

async fn take_backup(app: &TestApp) -> backup::BackupFile {
    backup::create(&app.state, &Attribution::manual(None)).await.unwrap()
}

/// An archive of `name` holding `bytes`, written where a backup would be.
fn archive(dir: &std::path::Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

#[tokio::test]
async fn each_archive_is_copied_whole_and_private_under_its_own_name() {
    let (app, data, copies) = app_copying("whole", true).await;

    let file = take_backup(&app).await;
    let ended = copies_ended(&app, 1).await;

    assert_eq!(ended[0], ("success".to_string(), Some("JobDetailBackupsCopied".to_string())));
    assert_eq!(archives_in(&copies), std::slice::from_ref(&file.name));
    let (original, copy) = (data.join("backups").join(&file.name), copies.join(&file.name));
    assert_eq!(std::fs::read(&original).unwrap(), std::fs::read(&copy).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&copy).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the copy carries the master key and is open: {mode:o}");
    }
    assert!(copies.join(MARKER).exists(), "the first copy left no marker");

    let listed = app.get("/api/v1/backups").await;
    let listed = listed.assert_ok();
    assert_eq!(listed["backups"][0]["copied"], true, "{listed}");
    assert_eq!(listed["copy"]["state"], "copied", "{listed}");
    assert_eq!(listed["copy"]["folder"], copies.0.display().to_string(), "{listed}");
}

/// Without a copy folder nothing is copied, and the list says there is none.
#[tokio::test]
async fn without_a_copy_folder_nothing_is_copied_or_shown() {
    let (app, _data) = app_with_files("copy-none").await;

    take_backup(&app).await;

    let listed = app.get("/api/v1/backups").await;
    let listed = listed.assert_ok();
    assert!(listed["copy"].is_null(), "{listed}");
    assert_eq!(listed["backups"][0]["copied"], false, "{listed}");
    assert_eq!(app.count("SELECT COUNT(*) FROM jobs WHERE kind = 'backup_copy'").await, 0);
}

/// A share not there fails the copy and says so, and the archive is taken all the same.
#[tokio::test]
async fn a_missing_folder_fails_the_copy_and_never_the_backup() {
    let (app, data, copies) = app_copying("missing", false).await;

    let file = take_backup(&app).await;
    let ended = copies_ended(&app, 1).await;

    assert!(data.join("backups").join(&file.name).exists());
    assert_eq!(ended[0], ("failed".to_string(), Some("JobDetailBackupCopyMissing".to_string())));
    assert!(!copies.0.exists(), "the copy folder was created where no share was mounted");
    assert_eq!(app.last_job_status("backup").await, "success");
    let status = app.get("/api/v1/status").await;
    let codes: Vec<&str> = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning["code"].as_str())
        .collect();
    assert!(codes.contains(&"backup_copy_failed"), "{codes:?}");
    assert!(!codes.contains(&"backup_failed"), "{codes:?}");
}

/// Once a folder has had a copy, losing its marker is the empty mount point of a share not
/// mounted: nothing goes there until the owner takes the folder as it is.
#[tokio::test]
async fn a_folder_that_lost_its_marker_takes_nothing_until_taken_as_it_is() {
    let (app, _data, copies) = app_copying("unmounted", true).await;
    take_backup(&app).await;
    copies_ended(&app, 1).await;
    // The share gone: its mount point is an empty folder of the local disk.
    std::fs::remove_dir_all(&copies.0).unwrap();
    std::fs::create_dir(&copies.0).unwrap();

    backup::copy_in_background(&app.state, Attribution::manual(None));
    let ended = copies_ended(&app, 2).await;

    assert_eq!(ended[0], ("failed".to_string(), Some("JobDetailBackupCopyUnmounted".to_string())));
    assert!(archives_in(&copies).is_empty(), "an archive went to the local disk");
    let listed = app.get("/api/v1/backups").await;
    assert_eq!(listed.assert_ok()["copy"]["state"], "unmounted", "{}", listed.json);

    app.post("/api/v1/backups/copy/adopt", json!({})).await.assert_status(StatusCode::ACCEPTED);
    let ended = copies_ended(&app, 3).await;

    assert_eq!(ended[0].0, "success", "{ended:?}");
    assert_eq!(archives_in(&copies).len(), 1);
    assert!(copies.join(MARKER).exists());
}

/// Taking a folder as it is needs one to take.
#[tokio::test]
async fn a_folder_is_taken_as_it_is_only_where_there_is_one() {
    let (app, _data) = app_with_files("copy-adopt-unset").await;
    app.post("/api/v1/backups/copy/adopt", json!({})).await.assert_status(StatusCode::BAD_REQUEST);

    let (app, _data, _copies) = app_copying("adopt-missing", false).await;
    let refused = app.post("/api/v1/backups/copy/adopt", json!({})).await;
    refused.assert_status(StatusCode::CONFLICT);
    assert_eq!(refused.json["reason"], "backup_copy_unreachable", "{}", refused.json);
}

/// A copy cut short leaves a partial file, swept by the next, which also copies every archive
/// a share asleep missed.
#[tokio::test]
async fn the_next_copy_sweeps_a_partial_one_and_catches_up_on_what_was_missed() {
    let (app, data, copies) = app_copying("catch-up", true).await;
    let backups = data.join("backups");
    archive(&backups, "routarr-backup-20260901-020000.zip", b"first");
    archive(&backups, "routarr-backup-20260902-020000.zip", b"second");
    std::fs::write(copies.join(MARKER), b"").unwrap();
    std::fs::write(copies.join(".routarr-backup-20260901-020000.zip.partial"), b"fir").unwrap();

    backup::copy_in_background(&app.state, Attribution::manual(None));
    copies_ended(&app, 1).await;

    assert_eq!(
        archives_in(&copies),
        ["routarr-backup-20260901-020000.zip", "routarr-backup-20260902-020000.zip"]
    );
    assert!(!copies.join(".routarr-backup-20260901-020000.zip.partial").exists());
    assert_eq!(std::fs::read(copies.join("routarr-backup-20260901-020000.zip")).unwrap(), b"first");
}

/// The retention count prunes the copies as it prunes the backups, those application keys took
/// counted apart, and the newest copies stay whatever the backup folder holds.
#[tokio::test]
async fn the_retention_count_prunes_the_copies_as_it_prunes_the_backups() {
    let (app, _data, copies) = app_copying("retention", true).await;
    app.store_setting("backup_retention_count", "1").await;
    std::fs::write(copies.join(MARKER), b"").unwrap();
    for name in [
        "routarr-backup-20260901-020000.zip",
        "routarr-backup-20260902-020000.zip",
        "routarr-backup-20260901-030000-app.zip",
        "routarr-backup-20260902-030000-app.zip",
    ] {
        archive(&copies, name, b"an archive");
    }

    backup::copy_in_background(&app.state, Attribution::manual(None));
    copies_ended(&app, 1).await;

    assert_eq!(
        archives_in(&copies),
        ["routarr-backup-20260902-020000.zip", "routarr-backup-20260902-030000-app.zip"]
    );
}

/// Sealed since its copy, an archive goes by another name: the sealed copy replaces the one in
/// the clear, which a copy folder must not keep once the owner asked for a passphrase.
#[tokio::test]
async fn a_passphrase_set_later_seals_the_copies_too() {
    let (app, _data, copies) = app_copying("sealed-later", true).await;
    let file = take_backup(&app).await;
    copies_ended(&app, 1).await;

    let body = json!({ "passphrase": "a long passphrase for the archives" });
    app.put("/api/v1/backups/passphrase", body).await.assert_status(StatusCode::NO_CONTENT);
    copies_ended(&app, 2).await;

    assert_eq!(archives_in(&copies), [format!("{}.enc", file.name)]);
    assert!(backup::is_sealed_file(&copies.join(format!("{}.enc", file.name))));
}

#[tokio::test]
async fn deleting_an_archive_deletes_its_copy() {
    let (app, _data, copies) = app_copying("delete", true).await;
    let file = take_backup(&app).await;
    copies_ended(&app, 1).await;

    app.delete(&format!("/api/v1/backups/{}", file.name)).await.assert_ok();

    super::until(|| async { !copies.join(&file.name).exists() }).await;
    assert!(archives_in(&copies).is_empty());
}

/// A pipe nobody writes to, named as an archive of the copy folder: listing the folder opens it
/// and waits, as a share that hangs does. Let go once dropped, failed test or not, since a
/// runtime waits for its blocking threads.
#[cfg(target_os = "linux")]
struct Hanging(std::path::PathBuf);

#[cfg(target_os = "linux")]
impl Hanging {
    fn at(path: std::path::PathBuf) -> Self {
        let made = std::process::Command::new("mkfifo").arg(&path).status().unwrap();
        assert!(made.success());
        Self(path)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Hanging {
    fn drop(&mut self) {
        // Opened for reading and writing, which Linux never holds, the pipe lets its reader go.
        // A plain file takes its name meanwhile, so the reader never meets it again.
        let pipe = std::fs::OpenOptions::new().read(true).write(true).open(&self.0);
        let plain = self.0.with_extension("plain");
        if std::fs::write(&plain, b"").is_ok() {
            std::fs::rename(&plain, &self.0).ok();
        }
        drop(pipe);
    }
}

/// A share that hangs holds its copy, which is reported once its time is out, and nothing
/// else: a backup taken meanwhile answers at once.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_copy_that_hangs_holds_back_nothing_but_itself() {
    let (app, _data, copies) = app_copying("hangs", true).await;
    let _hanging = Hanging::at(copies.join("routarr-backup-20260901-020000.zip"));

    backup::copy_in_background(&app.state, Attribution::manual(None));
    super::until(|| async {
        app.count("SELECT COUNT(*) FROM jobs WHERE kind = 'backup_copy'").await == 1
    })
    .await;
    let started = std::time::Instant::now();
    take_backup(&app).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "the backup waited");

    super::until(|| async {
        app.count(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'backup_copy'
              AND detail_key = 'JobDetailBackupCopyTimedOut'",
        )
        .await
            == 1
    })
    .await;
    let ended = copies_ended(&app, 2).await;
    assert_eq!(
        ended.last().unwrap(),
        &("failed".to_string(), Some("JobDetailBackupCopyTimedOut".to_string())),
        "{ended:?}"
    );
}
