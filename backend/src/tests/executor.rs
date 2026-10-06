//! Applying and reverting decisions against a live fake Arr.

use crate::jobs::Attribution;
use crate::services::executor;
use crate::services::routing::{self, SimulationOptions};
use crate::services::sync;

use super::fake_arr::FakeArr;
use super::{TestApp, TestResponse, one_move_ready};

/// The client hanging up mid-apply (a browser navigating away, an Arr whose
/// webhook timed out) must not leave a move done at the Arr and unknown here.
/// Awaited in the request future, the work would be dropped with the connection
/// at its next await: the Arr would have moved the item, the decision would
/// stay `pending`, the row would keep its old path, and the next pass would
/// propose the move again.
#[tokio::test]
async fn hanging_up_mid_apply_still_records_the_move() {
    use std::time::Duration;

    // Long enough to hang up while the Arr is at work, and well under the
    // test client's own timeout, or the move fails on its own and proves
    // nothing about the hang-up.
    let arr = FakeArr::holding_edits(Duration::from_millis(100)).await;
    let (app, decision_id) = one_move_ready(&arr).await;

    // Driven until the Arr has the edit in hand, then dropped, which is what a
    // request future undergoes when its connection goes away.
    {
        let mut applying = Box::pin(app.post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": [decision_id.clone()], "move_files": false, "confirm": ["capacity", "threshold"] }),
        ));
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            while arr.recorded().writes.is_empty() {
                let _ = futures::poll!(applying.as_mut());
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(reached.is_ok(), "the edit never reached the Arr");
    }

    // Bounded, not timed: the work either finishes and records, or it was
    // dropped with the request and never will.
    let recorded = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM decisions WHERE id = ?")
                .bind(&decision_id)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
            if status == "applied" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(recorded.is_ok(), "the Arr moved the item and Routarr never recorded it");

    let path: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(path, "/movies/anime", "the row was not brought in line with the Arr");
    // Bounded, not immediate: the lock falls when the detached future ends,
    // which is after `job.succeed`, itself after the status this test has just
    // observed. Asserting on it straight away asserts an ordering the executor
    // never promised, and only holds on a machine fast enough to lose the race
    // by microseconds, which a busy CI runner is not.
    let released = tokio::time::timeout(Duration::from_secs(5), async {
        while app.state.jobs.try_lock("apply").is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(released.is_ok(), "the apply lock was left held");
    assert_eq!(
        app.last_job_status("apply").await,
        "success",
        "the Tasks screen must see the job end"
    );
}

/// An apply that cannot load its moves ends its job as failed. A `?` between
/// `start` and the outcome would leave the row `running` until the next restart
/// fails it as an orphan, and the Tasks screen would show a job in progress.
/// The cause, a database error, goes to the log: any key reads the row.
#[tokio::test]
async fn an_apply_that_cannot_load_its_moves_reports_a_failed_job() {
    use tracing_subscriber::layer::SubscriberExt;

    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    // The one column only the loader reads, so the guards before the job pass.
    sqlx::query("ALTER TABLE media DROP COLUMN current_path")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    // For the thread, not the future: the job fails on a task of its own,
    // which the test's single-threaded runtime runs on this same thread.
    let _logging = tracing::subscriber::set_default(subscriber);
    let outcome = executor::apply_unattended(
        &app.state,
        &[decision_id],
        &Attribution::manual(None),
        executor::unattended_turn(&app.state).await.expect("the lock is free"),
    )
    .await;
    assert!(outcome.is_err(), "the loader was meant to fail: {outcome:?}");
    assert!(capture.contents().contains("current_path"), "the log does not name the cause");

    let (status, error): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_message FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed", "the Tasks screen must see the job end");
    let error = error.unwrap_or_default();
    assert!(!error.is_empty() && !error.contains("current_path"), "the row reads {error}");
    assert!(app.state.jobs.try_lock("apply").is_some(), "the apply lock was left held");
}

/// A decision names the folder the item was in when it was proposed. Moved
/// since, by hand in Radarr or by an earlier apply the row already reflects,
/// the proposal describes an item that no longer exists, and sending it to the
/// Arr again is a second write nobody asked for.
#[tokio::test]
async fn a_decision_whose_item_has_moved_since_is_skipped_rather_than_reapplied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query(
        "UPDATE media SET current_root_folder = '/movies/anime',
                current_path = '/movies/anime/My Neighbor Totoro (1988)' WHERE id = 'm-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = executor::apply_unattended(
        &app.state,
        &[decision_id],
        &Attribution::manual(None),
        executor::unattended_turn(&app.state).await.expect("the lock is free"),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert!(arr.recorded().writes.is_empty(), "the Arr was written to for a stale decision");
}

/// A proposal records what the rules said when the simulation ran. Nothing
/// retires it when a rule is deleted, edited or reordered, so the plan the
/// Simulation screen reloads after the edit is still pending, and applied as it
/// stands it sends the item where no rule sends it any more.
#[tokio::test]
async fn a_proposal_the_rules_no_longer_justify_is_skipped_at_apply_time() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert!(arr.recorded().writes.is_empty(), "the Arr was written to for a stale proposal");
    // Left pending, it is still counted as awaiting review and offered again,
    // to be skipped again on every apply.
    let superseded: bool = sqlx::query_scalar("SELECT superseded FROM decisions WHERE id = ?")
        .bind(&decision_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(superseded, "a proposal found stale is still offered");
}

/// Remapping a category moves its destination, and the proposal still names
/// the old one: the files would go to a folder no category points at, and the
/// next simulation would propose moving every one of them back.
#[tokio::test]
async fn a_proposal_whose_category_was_remapped_is_skipped_at_apply_time() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query("UPDATE root_folders SET category = NULL WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-3', 'inst-1', 3, '/data/anime', 1, 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert!(arr.recorded().writes.is_empty(), "the files went to the folder the category left");
}

/// Apply all loads its moves slice by slice through the same loader, and a
/// plan reloaded after an edit is exactly what it is pointed at.
#[tokio::test]
async fn apply_all_skips_a_proposal_the_rules_no_longer_justify() {
    let arr = FakeArr::start().await;
    let (app, _) = one_move_ready(&arr).await;
    let simulation_id: String = sqlx::query_scalar("SELECT simulation_id FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_simulation_in_batches(
        &app.state,
        &simulation_id,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert!(arr.recorded().writes.is_empty(), "the Arr was written to for a stale proposal");
}

/// What is revalidated is where the item goes, not which rule sent it there.
/// Another rule sending the item to the same category changes the winner and
/// nothing the move depends on.
#[tokio::test]
async fn a_proposal_another_rule_now_justifies_is_still_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('rule-ja', 'Japanese', 5, 1, 'both',
                 '[{\"type\":\"original_language\",\"value\":[\"ja\"]}]', 'anime', 'all')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    // The rule that proposed the move is gone, so only the new one can
    // justify it.
    sqlx::query("DELETE FROM rules WHERE id = 'rule-anime'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (1, 0), "{report:?}");
    assert_eq!(arr.recorded().writes[0]["rootFolderPath"], "/movies/anime");
}

/// A sync can spell the same folder with a trailing slash, and the proposal
/// still names the folder the rules send the item to.
#[tokio::test]
async fn a_folder_respelt_with_a_trailing_slash_does_not_make_a_proposal_stale() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query("UPDATE root_folders SET path = '/movies/anime/' WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (1, 0), "{report:?}");
}

/// One selection, one proposal still justified and one not: only the second
/// is retired. A retired decision is hidden from the history and the metrics,
/// so an applied one carrying the flag disappears from both.
#[tokio::test]
async fn only_the_stale_proposal_of_a_selection_is_retired() {
    let arr = FakeArr::start().await;
    let (app, _) = one_move_ready(&arr).await;
    // A second film, pinned to `anime`, so it keeps its destination when the
    // rules go.
    arr.hold_film(11);
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Akira', 8392,
                 '/movies/standard/Akira', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-2', 'm-2', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    let ids: Vec<String> = result.decisions.iter().map(|d| d.id.clone()).collect();
    assert_eq!(ids.len(), 2, "precondition: both films are proposed");
    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &ids,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (1, 1), "{report:?}");
    let rows: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT media_id, status, superseded FROM decisions WHERE simulation_id = ?
          ORDER BY media_id",
    )
    .bind(&result.simulation_id)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ("m-1".to_string(), "pending".to_string(), true),
            ("m-2".to_string(), "applied".to_string(), false)
        ],
        "the retirement took the wrong proposal"
    );
}

/// A simulation can land between the apply reading a proposal and retiring
/// it, and write the item's new proposal. What the apply found stale is the
/// proposal it read, not every proposal the item has.
#[tokio::test]
async fn retiring_a_stale_proposal_leaves_a_newer_one_for_the_same_item() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
         current_root_folder, target_category, target_root_folder, action, status, reasons,
         alternatives, confidence)
         VALUES ('d-newer', 'm-1', 'Totoro', 'movie', 'inst-1', '/movies/standard', 'standard',
                 NULL, 'skip', 'pending', '[]', '[]', 0.0)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.skipped, 1, "{report:?}");
    let newer: bool = sqlx::query_scalar("SELECT superseded FROM decisions WHERE id = 'd-newer'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(!newer, "the item's newer proposal was retired with the stale one");
}

/// An Arr reports a root folder with or without its trailing slash, so a sync
/// writing the same folder with a slash leaves the title where its proposal
/// found it, and the proposal stands.
#[tokio::test]
async fn a_folder_reported_again_with_a_trailing_slash_keeps_its_proposal() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    app.execute(&["UPDATE media SET current_root_folder = '/movies/standard/'"]).await;

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.skipped), (1, 0), "{report:?}");
}

/// Radarr's editor names a folder moved with its files from Radarr's naming
/// format, which renames it on disk under every tool reading the library by
/// path. The film's own record is sent instead, its folder name kept.
#[tokio::test]
async fn a_radarr_move_with_its_files_keeps_the_folder_name() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let kept = "/movies/anime/My Neighbor Totoro (1988)";
    let sent = arr.recorded().writes[0]["path"].clone();
    assert_eq!(sent, kept, "Radarr was not sent the folder name");
    let path: String = sqlx::query_scalar("SELECT current_path FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(path, kept);
}

/// Radarr's editor refuses a root folder Radarr does not list, and a
/// declared destination is one it never lists: each film goes through its
/// own record, with its files or without.
#[tokio::test]
async fn a_declared_destination_on_radarr_is_reached_film_by_film() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    sqlx::query("UPDATE root_folders SET category = NULL WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, path, accessible, category, origin)
         VALUES ('rf-declared', 'inst-1', '/movies/anime/films', 1, 'anime', 'declared')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let mut proposed = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap()
    .decisions;
    proposed.sort_by(|a, b| a.media_title.cmp(&b.media_title));

    for (decision, move_files) in proposed.iter().zip([true, false]) {
        let report = executor::apply_decisions(
            &app.state,
            std::slice::from_ref(&decision.id),
            move_files,
            &executor::Confirmed::all(),
            &Attribution::manual(None),
        )
        .await
        .unwrap();
        assert_eq!(report.applied, 1, "{report:?}");
    }

    let recorded = arr.recorded();
    let sent: Vec<&serde_json::Value> = recorded.writes.iter().map(|w| &w["path"]).collect();
    assert_eq!(sent, ["/movies/anime/films/Film 100", "/movies/anime/films/Film 101"]);
    assert_eq!(recorded.query_strings, ["moveFiles=true", "moveFiles=false"]);
}

/// A title moved twice can only have its latest move undone. Undoing the older
/// one would send it back to its first folder and skip the one between, and in
/// History two buttons would answer to one name. The list says which move a
/// revert may undo, and the server refuses the other without writing.
#[tokio::test]
async fn only_the_latest_move_of_a_title_can_be_reverted() {
    let arr = FakeArr::start().await;
    let (app, first) = one_move_ready(&arr).await;
    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&first),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                current_root_folder, target_root_folder, target_category,
                                action, status, applied_at)
         VALUES ('d-later', 'm-1', 'Totoro', 'movie', 'inst-1', '/movies/anime',
                 '/movies/standard', 'standard', 'move', 'applied',
                 datetime('now', '+1 minute'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE media SET current_root_folder = '/movies/standard' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let listed = app.get("/api/v1/decisions?status=applied").await;
    let revertible: std::collections::HashMap<String, bool> = listed.assert_ok()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["id"].as_str().unwrap().to_string(), d["revertible"] == true))
        .collect();
    assert_eq!(revertible.get(&first), Some(&false), "{revertible:?}");
    assert_eq!(revertible.get("d-later"), Some(&true), "{revertible:?}");

    let writes = arr.recorded().writes.len();
    let refused = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": [first], "confirm": ["capacity", "threshold"] }),
        )
        .await;
    assert_eq!(refused.assert_ok()["applied"], 0);
    assert_eq!(arr.recorded().writes.len(), writes, "the older move was sent to the Arr");
}

#[tokio::test]
async fn applying_moves_the_media_and_records_it() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (1, 0));

    assert_eq!(arr.recorded().writes[0]["rootFolderPath"], "/movies/anime");
    assert_eq!(arr.recorded().query_strings, ["moveFiles=true"]);

    let (status, applied_at): (String, Option<String>) =
        sqlx::query_as("SELECT status, applied_at FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "applied");
    assert!(applied_at.is_some());
}

#[tokio::test]
async fn applying_rewrites_the_local_path_so_the_move_is_not_reproposed() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime");
    assert_eq!(
        path, "/movies/anime/My Neighbor Totoro (1988)",
        "the folder name must be preserved"
    );

    // The whole point: a fresh simulation now agrees the media is in place.
    let again =
        routing::run_simulation(&app.state.pool, SimulationOptions::default()).await.unwrap();
    assert_eq!(again.moves_required, 0);
    assert_eq!(again.already_correct, 1);
}

/// The Arr moves the files in a command of its own after answering, and a
/// rescan running beside it deletes the file records of a title whose files
/// have not arrived yet. Nothing upstream rescans after its own move.
#[tokio::test]
async fn a_move_asks_the_arr_for_no_rescan() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        true,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let recorded = arr.recorded();
    assert!(!recorded.writes.is_empty(), "nothing reached the Arr");
    let commands: Vec<&serde_json::Value> =
        recorded.writes.iter().filter(|w| w.get("name").is_some()).collect();
    assert!(commands.is_empty(), "a command was posted beside the move: {commands:?}");
}

#[tokio::test]
async fn an_upstream_failure_marks_the_decision_failed_and_logs_it() {
    let arr = FakeArr::failing(500).await;
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (0, 1));
    assert_eq!(report.errors.len(), 1);

    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_message FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("500"));

    let (success, logged): (bool, i64) =
        sqlx::query_as("SELECT success, COUNT(*) FROM execution_logs")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(logged, 1);
    assert!(!success);

    // The library must be left describing reality, not the attempted move.
    let root: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(root, "/movies/standard");
}

/// "0 applied, 1 failed" in green on the Tasks screen, beside a failed-moves
/// count that says otherwise: nothing done and something failed is a failure.
#[tokio::test]
async fn an_apply_in_which_every_move_failed_is_a_failed_job() {
    let arr = FakeArr::failing(500).await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(app.last_job_status("apply").await, "failed");
    // Failed, and still holding the report of what it attempted.
    let tasks = app.get("/api/v1/jobs?kind=apply").await;
    let task = &tasks.assert_ok()["data"][0];
    assert_eq!(task["result"]["failed"], 1, "{task}");
}

/// The same holds for a revert, whose job closes on its own path.
#[tokio::test]
async fn a_revert_in_which_every_move_failed_is_a_failed_job() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    let refusing = FakeArr::failing(500).await;
    sqlx::query("UPDATE instances SET base_url = ?")
        .bind(&refusing.base_url)
        .execute(&app.state.pool)
        .await
        .unwrap();

    let report = executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (0, 1), "{report:?}");
    assert_eq!(app.last_job_status("revert").await, "failed");
}

/// A proposal left from before its instance was switched off is not applied:
/// the switch reads "Enabled (synced and routed)", and the revalidation finds
/// the item no longer routed.
#[tokio::test]
async fn a_proposal_for_a_disabled_instance_is_not_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    sqlx::query("UPDATE instances SET enabled = 0").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0);
    assert!(arr.recorded().writes.is_empty(), "the disabled instance was written to");
}

/// A revert reads no rule, so the switch itself has to stop it: a move made
/// while the instance was on is not undone once it is off.
#[tokio::test]
async fn a_move_on_a_disabled_instance_is_not_reverted() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    let by = Attribution::manual(None);
    let applied = executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &by,
    )
    .await
    .unwrap();
    assert_eq!(applied.applied, 1);
    let writes = arr.recorded().writes.len();

    sqlx::query("UPDATE instances SET enabled = 0").execute(&app.state.pool).await.unwrap();
    let reverted = executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::none(),
        &by,
    )
    .await
    .unwrap();

    assert_eq!((reverted.applied, reverted.failed), (0, 1));
    assert_eq!(arr.recorded().writes.len(), writes, "the disabled instance was written to");
}

/// An Arr restarting refuses a revert once. The move it tried to undo is still
/// in place, so the decision stays applied and the next Revert finds it.
#[tokio::test]
async fn a_revert_the_arr_refuses_leaves_the_decision_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    let by = Attribution::manual(None);
    let nothing_answered = executor::Confirmed::none();
    let revert = || {
        executor::revert_decisions(
            &app.state,
            std::slice::from_ref(&decision_id),
            false,
            &nothing_answered,
            &by,
        )
    };
    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    async fn point_at(app: &TestApp, url: &str) {
        sqlx::query("UPDATE instances SET base_url = ? WHERE id = 'inst-1'")
            .bind(url)
            .execute(&app.state.pool)
            .await
            .unwrap();
    }
    let refusing = FakeArr::failing(503).await;
    point_at(&app, &refusing.base_url).await;
    assert_eq!(revert().await.unwrap().failed, 1, "the refusing Arr accepted the revert");
    let status: String = sqlx::query_scalar("SELECT status FROM decisions WHERE id = ?")
        .bind(&decision_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "applied", "a refused revert rewrote the move it undid as failed");

    point_at(&app, &arr.base_url).await;
    assert_eq!(revert().await.unwrap().applied, 1, "the second Revert could not find the move");
}

/// The proposals a person reviewed, written again by a scheduled pass in the
/// meantime: the one still proposing the same move is applied through its
/// successor, and the one now going elsewhere is counted as replaced.
#[tokio::test]
async fn a_selection_replaced_by_the_same_proposal_is_still_applied() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let reviewed = app.simulate().await;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM decisions WHERE simulation_id = ? AND action = 'move' ORDER BY media_title",
    )
    .bind(&reviewed)
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    app.execute(&[
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-1', 'm-1', 'standard')",
    ])
    .await;
    app.simulate().await;

    let report = executor::apply_decisions(
        &app.state,
        &ids,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.requested, report.applied, report.superseded), (2, 1, 1), "{report:?}");
    assert_eq!(moved(&arr), ["/movies/anime/Film 100"]);
}

/// A selection whose every proposal now goes elsewhere is refused with the
/// reason, rather than answered as a success that moved nothing.
#[tokio::test]
async fn a_selection_whose_proposals_all_go_elsewhere_now_is_refused() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    app.execute(&[
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-1', 'm-1', 'standard')",
    ])
    .await;
    app.simulate().await;

    let refused = app
        .post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": [decision_id], "move_files": false }),
        )
        .await;

    assert_eq!(refused.status, 409, "{}", refused.json);
    assert!(refused.message().contains("newer simulation"), "{}", refused.message());
    assert!(arr.recorded().writes.is_empty());
}

/// An id sent twice names one decision: one move, counted once, and asked
/// about once by the guards.
#[tokio::test]
async fn a_decision_named_twice_is_applied_once_and_counted_once() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id.clone(), decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.requested, report.applied, report.skipped), (1, 1, 0), "{report:?}");
}

/// `ready`, with its proposal applied: the film sits in `/movies/anime` and
/// came from `/movies/standard`, which a revert writes into.
async fn applied(arr: &FakeArr) -> (TestApp, String) {
    let (app, decision_id) = one_move_ready(arr).await;
    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    (app, decision_id)
}

/// What the caller chose about the files reaches the Arr, on every route
/// that moves and for both answers: a flag fixed anywhere on the way moves
/// files nobody asked to move, or leaves behind files somebody did.
#[tokio::test]
async fn every_route_that_moves_sends_the_arr_the_files_choice_it_was_given() {
    let every_question = ["batch", "capacity", "threshold", "unreachable"];
    for move_files in [false, true] {
        for route in ["apply", "apply-all", "revert"] {
            let arr = FakeArr::start().await;
            let (app, decision_id) =
                if route == "revert" { applied(&arr).await } else { one_move_ready(&arr).await };
            let simulation: String =
                sqlx::query_scalar("SELECT simulation_id FROM decisions WHERE id = ?")
                    .bind(&decision_id)
                    .fetch_one(&app.state.pool)
                    .await
                    .unwrap();
            let before = arr.recorded().query_strings.len();
            let body = match route {
                "apply-all" => serde_json::json!({
                    "simulation_id": simulation, "move_files": move_files, "confirm": every_question
                }),
                _ => serde_json::json!({
                    "decision_ids": [decision_id], "move_files": move_files, "confirm": every_question
                }),
            };

            let answer = app.post(&format!("/api/v1/decisions/{route}"), body).await;

            assert_eq!(answer.assert_ok()["applied"], 1, "{route} moving files {move_files}");
            let sent = arr.recorded().query_strings[before..].to_vec();
            assert_eq!(
                sent,
                [format!("moveFiles={move_files}")],
                "{route} sent the wrong files choice"
            );
        }
    }
}

/// One move at a time: an apply, an apply-all and a revert each refuse while
/// another holds the lock, before writing anything, and go ahead once it is
/// released.
#[tokio::test]
async fn every_route_that_moves_waits_for_the_move_already_running() {
    let every_question = ["batch", "capacity", "threshold", "unreachable"];
    for route in ["apply", "apply-all", "revert"] {
        let arr = FakeArr::start().await;
        let (app, decision_id) =
            if route == "revert" { applied(&arr).await } else { one_move_ready(&arr).await };
        let simulation: String =
            sqlx::query_scalar("SELECT simulation_id FROM decisions WHERE id = ?")
                .bind(&decision_id)
                .fetch_one(&app.state.pool)
                .await
                .unwrap();
        let body = match route {
            "apply-all" => {
                serde_json::json!({ "simulation_id": simulation, "confirm": every_question })
            }
            _ => serde_json::json!({ "decision_ids": [decision_id], "confirm": every_question }),
        };
        let path = format!("/api/v1/decisions/{route}");
        let writes = arr.recorded().writes.len();

        let running = app.state.jobs.try_lock("apply").expect("the lock is free");
        let refused = app.post(&path, body.clone()).await;
        assert_eq!(refused.status, axum::http::StatusCode::CONFLICT, "{route}: {:?}", refused.json);
        assert!(refused.message().contains("already"), "{route}: {}", refused.message());
        assert_eq!(arr.recorded().writes.len(), writes, "{route} wrote while another move ran");

        drop(running);
        assert_eq!(app.post(&path, body).await.assert_ok()["applied"], 1, "{route} once free");
    }
}

/// A revert holds the guards an apply holds: the global dry run, an empty
/// selection and the batch limit each refuse it before it writes.
#[tokio::test]
async fn a_revert_is_held_by_the_dry_run_and_the_batch_limit() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    let writes = arr.recorded().writes.len();
    let by = Attribution::manual(None);
    let all = executor::Confirmed::all();
    let localizer = crate::localization::Localizer::new("en");
    let refusal = |outcome: crate::error::AppResult<executor::ApplyReport>| match outcome {
        Err(crate::error::AppError::BadRequest(message)) => message,
        other => panic!("not refused: {other:?}"),
    };

    app.store_setting("global_dry_run", "true").await;
    let ids = [decision_id.clone()];
    let dry = executor::revert_decisions(&app.state, &ids, false, &all, &by);
    assert_eq!(refusal(dry.await), localizer.translate("ErrorDryRunEnabled", &[]));

    app.store_setting("global_dry_run", "false").await;
    let none = executor::revert_decisions(&app.state, &[], false, &all, &by);
    assert_eq!(refusal(none.await), localizer.translate("ErrorNoSelection", &[]));

    app.store_setting("batch_limit", "1").await;
    let two = [decision_id, "d-elsewhere".to_string()];
    let over = executor::revert_decisions(&app.state, &two, false, &all, &by);
    assert!(refusal(over.await).contains('1'), "the batch limit was not named");

    assert_eq!(arr.recorded().writes.len(), writes, "a refused revert wrote");
}

/// An apply stamps its move in the shape every stored timestamp has, which is
/// what a sync compares its read against: dropped, or written in another
/// shape, a sync that read before the move puts the old path back.
#[tokio::test]
async fn an_apply_stamps_its_move_in_the_shape_a_sync_compares() {
    let arr = FakeArr::start().await;
    let (app, _decision_id) = applied(&arr).await;

    let stamp: Option<String> = sqlx::query_scalar("SELECT moved_at FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();

    let stamp = stamp.expect("the apply left no stamp");
    let moved = crate::services::routing::parse_timestamp(&stamp).expect("an unreadable stamp");
    let age = chrono::Utc::now() - moved;
    assert!(age.num_seconds().abs() < 60, "{stamp} is not the moment of the apply");
    assert_eq!(stamp, crate::services::routing::format_timestamp(moved), "{stamp}");
}

/// Each series is its own call, settled on its own answer: a refused series
/// fails alone, and the other is applied under the path it was sent.
#[tokio::test]
async fn a_sonarr_batch_settles_each_series_on_its_own_answer() {
    let arr = FakeArr::start().await;
    arr.refuse_series(21);
    let app = TestApp::new().await;
    app.seed_instance_at("inst-s", "sonarr", &arr.base_url).await;
    app.execute(&[
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
         VALUES ('rf-ts', 'inst-s', 1, '/tv/standard', 1, 'standard'),
                ('rf-ta', 'inst-s', 2, '/tv/anime', 1, 'anime')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('s-20', 'inst-s', 20, 'series', 'Cowboy Bebop',
                 '/tv/standard/Cowboy Bebop (1998)', '/tv/standard', 1, 1),
                ('s-21', 'inst-s', 21, 'series', 'Trigun', '/tv/standard/Trigun (1998)',
                 '/tv/standard', 1, 1)",
        "INSERT INTO overrides (id, media_id, target_category)
         VALUES ('o-20', 's-20', 'anime'), ('o-21', 's-21', 'anime')",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                current_root_folder, target_root_folder, target_category,
                                action, status)
         VALUES ('d-20', 's-20', 'Cowboy Bebop', 'series', 'inst-s', '/tv/standard',
                 '/tv/anime', 'anime', 'move', 'pending'),
                ('d-21', 's-21', 'Trigun', 'series', 'inst-s', '/tv/standard', '/tv/anime',
                 'anime', 'move', 'pending')",
    ])
    .await;
    app.store_setting("global_dry_run", "false").await;
    let ids = ["d-20".to_string(), "d-21".to_string()];

    let report = executor::apply_decisions(
        &app.state,
        &ids,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!((report.applied, report.failed), (1, 1), "{report:?}");
    // Something was done, so the job succeeded: its counts say what failed.
    assert_eq!(app.last_job_status("apply").await, "success");
    let statuses: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM decisions ORDER BY id")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(statuses, [("d-20".into(), "applied".into()), ("d-21".into(), "failed".into())]);
    let path: String = sqlx::query_scalar("SELECT current_path FROM media WHERE id = 's-20'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(path, "/tv/anime/Cowboy Bebop (1998)");
}

/// Records the state of the folder a revert goes back to, as the last sync
/// saw it.
async fn origin_folder(app: &TestApp, accessible: bool, free_space: i64) {
    sqlx::query(
        "UPDATE root_folders SET accessible = ?, free_space = ?,
                                 last_accessible_at = '2026-09-20 08:00:00'
         WHERE path = '/movies/standard'",
    )
    .bind(accessible)
    .bind(free_space)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

fn asked(outcome: &crate::error::AppResult<executor::ApplyReport>) -> Option<&str> {
    match outcome {
        Err(crate::error::AppError::ConfirmationRequired { kind, .. }) => Some(kind),
        _ => None,
    }
}

/// A revert writes into the folder a move came from, as an apply writes into
/// its destination, and the same question stands before it: a NAS that sleeps
/// is unknown rather than gone, and only a person can tell which.
#[tokio::test]
async fn a_revert_onto_a_folder_that_is_not_answering_asks_first() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    origin_folder(&app, false, 1 << 40).await;
    let writes = arr.recorded().writes.len();
    let ids = std::slice::from_ref(&decision_id);
    let by = Attribution::manual(None);

    let first =
        executor::revert_decisions(&app.state, ids, false, &executor::Confirmed::none(), &by).await;

    assert_eq!(asked(&first), Some(executor::confirm::UNREACHABLE), "{first:?}");
    assert_eq!(arr.recorded().writes.len(), writes, "the revert wrote before asking");
    let answered = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": [decision_id], "confirm": ["unreachable"] }),
        )
        .await;
    assert_eq!(answered.assert_ok()["applied"], 1, "answering the question did not let it through");
}

/// Moved back with its files, a film needs room where it goes.
#[tokio::test]
async fn a_revert_moving_files_onto_a_full_folder_asks_first() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    origin_folder(&app, true, 1024).await;
    sqlx::query("UPDATE media SET size_on_disk = 8589934592 WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let writes = arr.recorded().writes.len();
    let ids = std::slice::from_ref(&decision_id);
    let by = Attribution::manual(None);

    let first =
        executor::revert_decisions(&app.state, ids, true, &executor::Confirmed::none(), &by).await;

    assert_eq!(asked(&first), Some(executor::confirm::CAPACITY), "{first:?}");
    assert_eq!(arr.recorded().writes.len(), writes, "the revert wrote before asking");
}

/// More reverts at once than the threshold is the same large change as an
/// apply of that size, and asks the same question.
#[tokio::test]
async fn a_revert_larger_than_the_threshold_asks_first() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    sqlx::query("UPDATE settings SET value = '1' WHERE key = 'confirmation_threshold'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let writes = arr.recorded().writes.len();
    let ids = [decision_id, "d-elsewhere".to_string()];
    let by = Attribution::manual(None);

    let first =
        executor::revert_decisions(&app.state, &ids, false, &executor::Confirmed::none(), &by)
            .await;

    assert_eq!(asked(&first), Some(executor::confirm::THRESHOLD), "{first:?}");
    assert_eq!(arr.recorded().writes.len(), writes, "the revert wrote before asking");
}

/// A title moved since the apply, by hand in the Arr, sits where the operator
/// put it, and a revert would pull it out of that folder, files included.
#[tokio::test]
async fn a_revert_leaves_alone_a_title_moved_since() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    sqlx::query(
        "UPDATE media SET current_root_folder = '/movies/kept',
                          current_path = '/movies/kept/My Neighbor Totoro (1988)'
         WHERE id = 'm-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let writes = arr.recorded().writes.len();

    let listed = app.get("/api/v1/decisions?status=applied").await;
    assert_eq!(listed.assert_ok()["data"][0]["revertible"], false, "{:?}", listed.json);
    let ids = std::slice::from_ref(&decision_id);
    let by = Attribution::manual(None);
    let report =
        executor::revert_decisions(&app.state, ids, true, &executor::Confirmed::all(), &by)
            .await
            .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert_eq!(arr.recorded().writes.len(), writes, "the title was pulled out of where it was put");
}

/// A folder the Arr no longer reports has no row for the guards to weigh, so
/// a revert into it would be asked nothing before writing there.
#[tokio::test]
async fn a_revert_into_a_folder_the_instance_no_longer_has_is_skipped() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = applied(&arr).await;
    sqlx::query("DELETE FROM root_folders WHERE path = '/movies/standard'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let writes = arr.recorded().writes.len();
    let ids = std::slice::from_ref(&decision_id);
    let by = Attribution::manual(None);

    let report =
        executor::revert_decisions(&app.state, ids, true, &executor::Confirmed::none(), &by)
            .await
            .unwrap();

    assert_eq!((report.applied, report.skipped), (0, 1), "{report:?}");
    assert_eq!(arr.recorded().writes.len(), writes, "the revert wrote into a folder nobody offers");
}

/// Reverting is the one operation a user reaches for when something has
/// already gone wrong, so it goes through the route: a handler that never
/// receives the ids it is given fails in exactly the moment nobody wants a
/// surprise.
#[tokio::test]
async fn reverting_puts_the_media_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    let body = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": [decision_id], "move_files": false }),
        )
        .await;

    let report = body.assert_ok();
    assert_eq!((report["applied"].as_i64(), report["failed"].as_i64()), (Some(1), Some(0)));

    let last_write =
        arr.recorded().writes.iter().rev().find(|w| w["rootFolderPath"].is_string()).cloned();
    assert_eq!(last_write.unwrap()["rootFolderPath"], "/movies/standard");

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/standard");
    assert_eq!(path, "/movies/standard/My Neighbor Totoro (1988)");

    let (status, reverted): (String, Option<String>) =
        sqlx::query_as("SELECT status, reverted_at FROM decisions WHERE id = ?")
            .bind(&decision_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "skipped");
    assert!(reverted.is_some());
}

#[tokio::test]
async fn a_decision_cannot_be_reverted_twice() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    let first = executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::none(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    assert_eq!((first.requested, first.applied), (1, 1), "the first revert did nothing");
    let second = executor::revert_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::none(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(second.applied, 0);
    assert_eq!(second.skipped, 1);
}

#[tokio::test]
async fn a_pending_decision_cannot_be_reverted() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    let ids = std::slice::from_ref(&decision_id);
    let by = Attribution::manual(None);

    let report =
        executor::revert_decisions(&app.state, ids, false, &executor::Confirmed::none(), &by)
            .await
            .unwrap();

    assert_eq!((report.requested, report.applied, report.skipped), (1, 0, 1));
    assert!(arr.recorded().writes.is_empty());
    // The control: applied, the same decision reverts.
    let applied =
        executor::apply_decisions(&app.state, ids, false, &executor::Confirmed::all(), &by)
            .await
            .unwrap();
    assert_eq!((applied.requested, applied.applied), (1, 1));
    let reverted =
        executor::revert_decisions(&app.state, ids, false, &executor::Confirmed::none(), &by)
            .await
            .unwrap();
    assert_eq!(reverted.applied, 1, "the fixture cannot revert at all");
}

/// The unattended path holds the global dry run as an apply does: it is the
/// one an operator is not watching.
#[tokio::test]
async fn an_unattended_apply_is_held_by_the_dry_run() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;
    app.store_setting("global_dry_run", "true").await;
    let turn = executor::unattended_turn(&app.state).await.expect("the lock is free");

    let refused =
        executor::apply_unattended(&app.state, &[decision_id], &Attribution::manual(None), turn)
            .await;

    assert!(matches!(refused, Err(crate::error::AppError::BadRequest(_))), "{refused:?}");
    assert!(arr.recorded().writes.is_empty(), "the dry run let an unattended move through");
}

/// One run sends its moves, and reports what failed, in the order the
/// decisions were given, from one run to the next.
#[tokio::test]
async fn errors_are_reported_in_the_order_the_moves_were_given() {
    let arr = FakeArr::failing(500).await;
    let app = TestApp::films_to_move(&arr, 4).await;
    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    let mut given: Vec<(String, String)> =
        result.decisions.iter().map(|d| (d.media_title.clone(), d.id.clone())).collect();
    given.sort();
    let given: Vec<String> = given.into_iter().rev().map(|(_, id)| id).collect();

    let report = executor::apply_decisions(
        &app.state,
        &given,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let reported: Vec<&str> = report.errors.iter().map(|e| e.decision_id.as_str()).collect();
    assert_eq!(reported, given);
}

// ------------------------------------------------------- asking before a move

/// Seed one pending move of `size` bytes from `/movies/standard` to
/// `/movies/anime`, with the two folders reporting the free space given.
async fn pending_move(app: &TestApp, size: i64, source_free: i64, target_free: i64) -> String {
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('i1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k', 1, 't')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    for (id, arr_id, path, free, category) in [
        ("rf-src", 1, "/movies/standard", source_free, "standard"),
        ("rf-dst", 2, "/movies/anime", target_free, "anime"),
    ] {
        sqlx::query(
            "INSERT INTO root_folders (id, instance_id, arr_id, path, free_space, accessible, category)
             VALUES (?, 'i1', ?, ?, ?, 1, ?)",
        )
        .bind(id)
        .bind(arr_id)
        .bind(path)
        .bind(free)
        .bind(category)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
         current_root_folder, monitored, has_files, size_on_disk)
         VALUES ('m1', 'i1', 10, 'movie', 'Big', '/movies/standard/Big', '/movies/standard', 1, 1, ?)",
    )
    .bind(size)
    .execute(&app.state.pool)
    .await
    .unwrap();
    // What sends the item to `anime`. An apply decides each item again, and a
    // decision nothing justifies is skipped before it reaches the Arr.
    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o1', 'm1', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id, current_root_folder,
         target_category, target_root_folder, action, status, reasons, alternatives, confidence)
         VALUES ('d1', 'm1', 'Big', 'movie', 'i1', '/movies/standard', 'anime', '/movies/anime',
                 'move', 'pending', '[]', '[]', 1.0)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.store_setting("global_dry_run", "false").await;
    "d1".to_string()
}

const SIX_GIB: i64 = 6 << 30;

/// A second 6 GiB film beside `pending_move`'s, moving to `folder` on `i1`.
async fn second_pending_move(app: &TestApp, category: &'static str) {
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
         current_root_folder, monitored, has_files, size_on_disk)
         VALUES ('m2', 'i1', 11, 'movie', 'Bigger', '/movies/standard/Bigger',
                 '/movies/standard', 1, 1, ?)",
    )
    .bind(SIX_GIB)
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO overrides (id, media_id, target_category) VALUES ('o2', 'm2', ?)")
        .bind(category)
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
         current_root_folder, target_category, target_root_folder, action, status, reasons,
         alternatives, confidence)
         SELECT 'd2', 'm2', 'Bigger', 'movie', 'i1', '/movies/standard', ?, path, 'move',
                'pending', '[]', '[]', 1.0
         FROM root_folders WHERE instance_id = 'i1' AND category = ?",
    )
    .bind(category)
    .bind(category)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

async fn apply_both(app: &TestApp) -> TestResponse {
    app.post(
        "/api/v1/decisions/apply",
        serde_json::json!({
            "decision_ids": ["d1", "d2"], "move_files": true, "confirm": ["batch", "threshold"]
        }),
    )
    .await
}

/// Each film fits the destination alone, the two together do not: the guard
/// weighs what the whole apply writes there.
#[tokio::test]
async fn moves_that_each_fit_but_not_together_ask_about_capacity() {
    let app = TestApp::new().await;
    pending_move(&app, SIX_GIB, 1 << 40, 10 << 30).await;
    second_pending_move(&app, "anime").await;

    let asked = apply_both(&app).await;

    assert_eq!(asked.status, axum::http::StatusCode::CONFLICT, "{:?}", asked.json);
    assert_eq!(asked.json["confirm"], "capacity", "{:?}", asked.json);
}

/// Two folders reporting the same free space are one volume: what they
/// receive together is weighed against it, each fitting alone.
#[tokio::test]
async fn moves_into_two_folders_of_one_volume_ask_about_their_sum() {
    let app = TestApp::new().await;
    pending_move(&app, SIX_GIB, 1 << 40, 10 << 30).await;
    app.execute(&[
        "INSERT INTO root_folders (id, instance_id, arr_id, path, free_space, accessible, category)
         VALUES ('rf-kids', 'i1', 3, '/movies/kids', 10737418240, 1, 'kids')",
    ])
    .await;
    second_pending_move(&app, "kids").await;

    let asked = apply_both(&app).await;

    assert_eq!(asked.status, axum::http::StatusCode::CONFLICT, "{:?}", asked.json);
    assert_eq!(asked.json["confirm"], "capacity", "{:?}", asked.json);
    let message = asked.message();
    assert!(message.contains("/movies/anime, /movies/kids"), "{message}");
}

/// Every destination is weighed, not the first the query returns: a roomy
/// folder first does not let a short one through after it.
#[tokio::test]
async fn the_capacity_question_names_a_short_destination_after_a_roomy_one() {
    let app = TestApp::new().await;
    pending_move(&app, SIX_GIB, 1 << 40, 1 << 40).await;
    app.execute(&["INSERT INTO root_folders (id, instance_id, arr_id, path, free_space,
                                             accessible, category)
                   VALUES ('rf-kids', 'i1', 3, '/movies/kids', 4294967296, 1, 'kids')"])
        .await;
    second_pending_move(&app, "kids").await;

    let asked = apply_both(&app).await;

    assert_eq!(asked.json["confirm"], "capacity", "{:?}", asked.json);
    assert!(asked.message().contains("/movies/kids"), "{}", asked.message());
    assert!(!asked.message().contains("/movies/anime"), "{}", asked.message());
}

/// Two Radarrs, each with one film bound for a folder of the same path, in one
/// apply. Folders, moves and Arr ids belong to an instance, so each Arr hears
/// only of its own film, and each folder weighs only its own instance's moves.
async fn two_instances(app: &TestApp, first: &FakeArr, second: &FakeArr) {
    app.seed_instance_at("inst-a", "radarr", &first.base_url).await;
    app.seed_instance_at("inst-b", "radarr", &second.base_url).await;
    second.hold_film(11);
    for (instance, arr_id) in [("inst-a", 10), ("inst-b", 11)] {
        for (folder, path, category, free) in [
            ("src", "/movies/standard", "standard", 1_i64 << 40),
            ("dst", "/movies/anime", "anime", 10 << 30),
        ] {
            sqlx::query(
                "INSERT INTO root_folders (id, instance_id, arr_id, path, free_space,
                                           accessible, category)
                 VALUES (?, ?, ?, ?, ?, 1, ?)",
            )
            .bind(format!("rf-{folder}-{instance}"))
            .bind(instance)
            .bind(if folder == "src" { 1 } else { 2 })
            .bind(path)
            .bind(free)
            .bind(category)
            .execute(&app.state.pool)
            .await
            .unwrap();
        }
        let media = format!("m-{instance}");
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
             current_root_folder, monitored, has_files, size_on_disk)
             VALUES (?, ?, ?, 'movie', 'Film', '/movies/standard/Film', '/movies/standard',
                     1, 1, ?)",
        )
        .bind(&media)
        .bind(instance)
        .bind(arr_id)
        .bind(SIX_GIB)
        .execute(&app.state.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO overrides (id, media_id, target_category) VALUES (?, ?, 'anime')")
            .bind(format!("o-{instance}"))
            .bind(&media)
            .execute(&app.state.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
             current_root_folder, target_category, target_root_folder, action, status,
             reasons, alternatives, confidence)
             VALUES (?, ?, 'Film', 'movie', ?, '/movies/standard', 'anime', '/movies/anime',
                     'move', 'pending', '[]', '[]', 1.0)",
        )
        .bind(format!("d-{instance}"))
        .bind(&media)
        .bind(instance)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    app.store_setting("global_dry_run", "false").await;
}

/// The path each update sent the Arr.
fn moved(arr: &FakeArr) -> Vec<serde_json::Value> {
    arr.recorded().writes.iter().map(|w| w["path"].clone()).collect()
}

#[tokio::test]
async fn one_apply_across_two_instances_sends_each_arr_only_its_own_film() {
    let (first, second) = (FakeArr::start().await, FakeArr::start().await);
    let app = TestApp::new().await;
    two_instances(&app, &first, &second).await;

    let applied = app
        .post(
            "/api/v1/decisions/apply",
            serde_json::json!({
                "decision_ids": ["d-inst-a", "d-inst-b"], "move_files": true,
                "confirm": ["batch", "threshold"]
            }),
        )
        .await;

    assert_eq!(applied.assert_ok()["applied"], 2, "a folder weighed the other instance's film");
    assert_eq!(moved(&first), ["/movies/anime/My Neighbor Totoro (1988)"]);
    assert_eq!(moved(&second), ["/movies/anime/Film 11"]);
}

/// `confirmed` names the guardrail the caller looked at, so a test that answers
/// the capacity question does not also answer the batch threshold.
async fn apply(app: &TestApp, confirmed: &[&str]) -> TestResponse {
    app.post(
        "/api/v1/decisions/apply",
        serde_json::json!({ "decision_ids": ["d1"], "move_files": true, "confirm": confirmed }),
    )
    .await
}

/// `free_space` is synced on every pass and `size_on_disk` sits on every row.
/// Uncompared, a batch that overruns its destination fails partway at the Arr.
#[tokio::test]
async fn a_move_larger_than_the_destination_is_refused_with_both_figures() {
    let app = TestApp::new().await;
    // 100 GB moving onto a volume with 10 GB free, from a different volume.
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;

    let refused = apply(&app, &[]).await;
    assert_eq!(refused.status, 409, "a plan that cannot fit was applied: {}", refused.json);
    let message = refused.message();
    assert!(message.contains("/movies/anime"), "the refusal must name the folder: {message}");
    // Both figures, in the interface's own vocabulary: the root folders table
    // says `GB` off the same division by 1024, and one figure named two ways
    // on two screens read together contradicts itself.
    assert!(message.contains("93.1 GB"), "the refusal must say what is moving: {message}");
    assert!(message.contains("9.3 GB"), "and what the destination has: {message}");
}

/// With `move_files` off no byte moves, so free space is not asked about: a
/// question with no stake teaches people to answer yes to the ones that have.
#[tokio::test]
async fn a_move_that_leaves_its_files_asks_nothing_about_free_space() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;

    let response = app
        .post(
            "/api/v1/decisions/apply",
            serde_json::json!({ "decision_ids": ["d1"], "move_files": false, "confirm": [] }),
        )
        .await;
    // Past every guard the move reaches the Arr, which is unreachable here.
    assert_eq!(response.status, 200, "a move leaving its files was asked about: {}", response.json);
    assert_eq!(response.json["failed"], 1, "the move never reached the Arr: {}", response.json);
}

/// A destination the Arr cannot reach is asked about, not silently written to.
///
/// The routing map keeps a sleeping folder on purpose, so the question of
/// whether it can be written to has to be asked here, and asked rather than
/// refused, because a NAS that wakes on access cannot be told from a dead disk.
#[tokio::test]
async fn a_sleeping_destination_is_asked_about_before_anything_is_written() {
    let app = TestApp::new().await;
    pending_move(&app, 1_000, 500_000_000_000, 400_000_000_000).await;
    sqlx::query(
        "UPDATE root_folders SET accessible = 0, last_accessible_at = '2026-09-05 03:00:00'
         WHERE rtrim(path, '/') = '/movies/anime'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let refused = apply(&app, &[]).await;
    assert_eq!(refused.status, 409, "it wrote into a folder that is not answering");
    assert_eq!(refused.json["confirm"], "unreachable");
    let message = refused.message();
    assert!(message.contains("/movies/anime"), "the refusal must name the folder: {message}");
    // The date, not a verdict: twenty minutes reads as a nap and three days as
    // a fault, and the operator is the one who knows their hardware.
    assert!(message.contains("2026-09-05"), "and say when it last answered: {message}");

    // Answered, it gets out of the way, and answers only itself.
    let allowed = apply(&app, &["unreachable"]).await;
    assert_eq!(allowed.status, 200, "confirming did not get past the guard: {}", allowed.json);
    assert_eq!(allowed.json["failed"], 1, "the move never reached the Arr: {}", allowed.json);
}

/// Every guardrail against every other: with all three asking about one move,
/// answering the two others never answers the third.
#[tokio::test]
async fn no_guardrail_is_answered_by_the_others() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;
    app.execute(&[
        "UPDATE settings SET value = '0' WHERE key = 'confirmation_threshold'",
        "UPDATE root_folders SET accessible = 0, last_accessible_at = '2026-09-20 08:00:00'
          WHERE id = 'rf-dst'",
    ])
    .await;
    let names = ["unreachable", "capacity", "threshold"];

    // Asked in a fixed order, and a name no guardrail here goes by lifts none.
    for unrelated in [&[][..], &["batch"][..]] {
        let first = apply(&app, unrelated).await;
        assert_eq!(first.json["confirm"], "unreachable", "{unrelated:?}: {}", first.json);
    }
    for asked in names {
        let others: Vec<&str> = names.iter().copied().filter(|name| *name != asked).collect();
        let answer = apply(&app, &others).await;
        assert_eq!(answer.status, 409, "{others:?} applied the plan: {}", answer.json);
        assert_eq!(answer.json["confirm"], asked, "{others:?} answered {asked}: {}", answer.json);
    }
    assert_eq!(apply(&app, &names).await.status, 200, "all three answered, and it still refused");
}

/// Applying a whole simulation asks the same question, scoped to the run
/// rather than to a list of ids: a library-sized run must not be spelled out
/// as bound parameters to be weighed.
#[tokio::test]
async fn a_whole_simulation_is_weighed_against_its_destination_too() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;
    sqlx::query("UPDATE decisions SET simulation_id = 'sim-1' WHERE id = 'd1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let refused = app
        .post(
            "/api/v1/decisions/apply-all",
            serde_json::json!({ "simulation_id": "sim-1", "move_files": true, "confirm": [] }),
        )
        .await;
    assert_eq!(refused.status, 409, "{}", refused.json);
    let message = refused.message();
    assert!(message.contains("/movies/anime"), "the shortfall must be carried in: {message}");
}

/// Evidence, not proof, so being wrong costs a click and never a block.
#[tokio::test]
async fn the_refusal_can_be_confirmed_through() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;

    assert_eq!(apply(&app, &[]).await.status, 409);
    // Past the guard the move reaches the Arr, which is unreachable here: the
    // apply answers, and reports the one move as failed. Anything else (a
    // refusal, or a success against a host that does not exist) is not the
    // guard letting go.
    let confirmed = apply(&app, &["capacity"]).await;
    assert_eq!(confirmed.status, 200, "confirming did not get past the guard: {}", confirmed.json);
    assert_eq!(confirmed.json["failed"], 1, "the move never reached the Arr: {}", confirmed.json);
}

/// The common homelab shape: two folders on one disk. A move there is a rename
/// and consumes nothing, and warning about it would make the guardrail noise on
/// the most ordinary setup there is. Identical free space is the evidence.
#[tokio::test]
async fn a_move_within_one_filesystem_is_not_weighed() {
    let app = TestApp::new().await;
    // Same figure on both folders, and far more bytes than either has free.
    pending_move(&app, 900_000_000_000, 10_000_000_000, 10_000_000_000).await;

    let response = apply(&app, &[]).await;
    assert_eq!(
        response.status, 200,
        "a rename on one volume was refused for want of space it does not need: {}",
        response.json
    );
    assert_eq!(response.json["failed"], 1, "the move never reached the Arr: {}", response.json);
}

/// A declared destination sits under no root folder an Arr reports, so no
/// sync brings it a free-space figure and nothing upstream would notice it
/// filling. With no figure, a move of files onto it is asked about rather
/// than waved through as an Arr's silent folder is.
#[tokio::test]
async fn a_declared_destination_with_no_figure_is_asked_about_its_space() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 0).await;
    sqlx::query(
        "UPDATE root_folders SET free_space = NULL, origin = 'declared', arr_id = NULL
         WHERE id = 'rf-dst'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let refused = apply(&app, &[]).await;
    assert_eq!(refused.status, 409, "a move onto an unweighed disk went ahead: {}", refused.json);
    assert_eq!(refused.json["confirm"], "capacity");
    let message = refused.message();
    assert!(message.contains("/movies/anime"), "the question must name the folder: {message}");
    assert!(message.contains("93.1 GB"), "and say what is moving: {message}");

    let confirmed = apply(&app, &["capacity"]).await;
    assert_eq!(confirmed.status, 200, "confirming did not get past the guard: {}", confirmed.json);
}

/// Nothing is invented where the Arr said nothing.
#[tokio::test]
async fn a_destination_reporting_no_free_space_is_not_guessed_at() {
    let app = TestApp::new().await;
    pending_move(&app, 900_000_000_000, 500_000_000_000, 0).await;
    sqlx::query("UPDATE root_folders SET free_space = NULL WHERE id = 'rf-dst'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let response = apply(&app, &[]).await;
    assert_eq!(response.status, 200, "{}", response.json);
    assert_eq!(response.json["failed"], 1, "the move never reached the Arr: {}", response.json);
}

#[tokio::test]
async fn the_moving_routes_refuse_an_unknown_decision() {
    let app = TestApp::new().await;
    // Off, or the dry-run refusal would answer for the unknown id.
    app.store_setting("global_dry_run", "false").await;
    for route in ["apply", "revert"] {
        let body = app
            .post(
                &format!("/api/v1/decisions/{route}"),
                serde_json::json!({ "decision_ids": ["nope"], "move_files": false }),
            )
            .await;
        // A refusal that names the id: not a 500, and not a success over nothing.
        assert_eq!(body.status, axum::http::StatusCode::BAD_REQUEST, "{route}: {}", body.json);
        assert!(body.message().contains("nope"), "{route}: {}", body.message());
    }
    assert_eq!(app.count("SELECT COUNT(*) FROM jobs").await, 0, "a task ran over nothing");
}

/// `apply` and `sync:{instance}` are different job locks, so an application and
/// a synchronisation of the same instance can run at once, and both write
/// `current_root_folder`. The losing interleaving is a sync that read the Arr
/// *before* the move landed and commits *after* it: it puts the old path back,
/// and the next simulation reproposes a move that already happened.
///
/// Made deterministic rather than raced: a sync whose read predates the move is
/// the same statement as a move stamped after the read. The fake still answers
/// `/movies/standard`, which is precisely the stale reply a real Arr gives while
/// the editor call is still in flight.
#[tokio::test]
async fn a_sync_that_read_before_the_move_does_not_put_the_old_path_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE media SET moved_at = datetime('now', '+1 minute') WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime", "the sync undid the move it did not know about");
    assert_eq!(path, "/movies/anime/My Neighbor Totoro (1988)");
}

/// The other half, and the one that stops the guard becoming a permanent veto:
/// once a read is newer than the move, the Arr is authoritative again. Someone
/// moving a film in Radarr's own interface must still be followed: Routarr
/// having moved it once is not a claim of ownership over it.
#[tokio::test]
async fn a_sync_that_read_after_the_move_still_follows_the_arr() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = one_move_ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    // Moved back in Radarr's own interface, after the apply.
    arr.edit_movie(serde_json::json!({
        "path": "/movies/standard/My Neighbor Totoro (1988)",
        "rootFolderPath": "/movies/standard",
    }));
    sqlx::query("UPDATE media SET moved_at = datetime('now', '-1 minute') WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    sync::sync_instance(&app.state, "inst-1", &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/standard", "upstream stopped being authoritative");
    assert_eq!(path, "/movies/standard/My Neighbor Totoro (1988)", "the path stayed behind");
}
