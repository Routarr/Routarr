//! Applying and reverting decisions against a live fake Arr.

use crate::jobs::Attribution;
use crate::services::executor;
use crate::services::routing::{self, SimulationOptions};
use crate::services::sync;

use super::fake_arr::FakeArr;
use super::{TestApp, TestResponse};

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
    let (app, decision_id) = ready(&arr).await;

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
    let job: String = sqlx::query_scalar(
        "SELECT status FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(job, "success", "the Tasks screen must see the job end");
}

/// An apply that cannot load its moves ends its job as failed. A `?` between
/// `start` and the outcome would leave the row `running` until the next restart
/// fails it as an orphan, and the Tasks screen would show a job in progress,
/// with nothing to say why the apply answered an error.
#[tokio::test]
async fn an_apply_that_cannot_load_its_moves_reports_a_failed_job() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    // The one column only the loader reads, so the guards before the job pass.
    sqlx::query("ALTER TABLE media DROP COLUMN current_path")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let outcome =
        executor::apply_unattended(&app.state, &[decision_id], &Attribution::manual(None)).await;
    assert!(outcome.is_err(), "the loader was meant to fail: {outcome:?}");

    let (status, error): (String, Option<String>) = sqlx::query_as(
        "SELECT status, error_message FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed", "the Tasks screen must see the job end");
    assert!(error.unwrap_or_default().contains("current_path"), "the failure names its cause");
    assert!(app.state.jobs.try_lock("apply").is_some(), "the apply lock was left held");
}

/// A decision names the folder the item was in when it was proposed. Moved
/// since, by hand in Radarr or by an earlier apply the row already reflects,
/// the proposal describes an item that no longer exists, and sending it to the
/// Arr again is a second write nobody asked for.
#[tokio::test]
async fn a_decision_whose_item_has_moved_since_is_skipped_rather_than_reapplied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    sqlx::query(
        "UPDATE media SET current_root_folder = '/movies/anime',
                current_path = '/movies/anime/Totoro (1988)' WHERE id = 'm-1'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let report = executor::apply_unattended(&app.state, &[decision_id], &Attribution::manual(None))
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
    let (app, decision_id) = ready(&arr).await;
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
    let (app, decision_id) = ready(&arr).await;
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
    let (app, _) = ready(&arr).await;
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
    let (app, decision_id) = ready(&arr).await;
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
    let (app, decision_id) = ready(&arr).await;
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
    let (app, _) = ready(&arr).await;
    // A second film, pinned to `anime`, so it keeps its destination when the
    // rules go.
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
    let (app, decision_id) = ready(&arr).await;
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

/// A library wired to a fake Radarr, with dry-run off and one pending move.
async fn ready(arr: &FakeArr) -> (TestApp, String) {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro', 8392,
                 '/movies/standard/Totoro (1988)', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    app.store_setting("global_dry_run", "false").await;

    let result = routing::run_simulation(
        &app.state.pool,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(result.moves_required, 1);
    let decision_id = result.decisions[0].id.clone();

    (app, decision_id)
}

#[tokio::test]
async fn applying_moves_the_media_and_records_it() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

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

    let write = arr.recorded().writes[0].clone();
    assert_eq!(write["rootFolderPath"], "/movies/anime");
    assert_eq!(write["moveFiles"], true);

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
    let (app, decision_id) = ready(&arr).await;

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
    assert_eq!(path, "/movies/anime/Totoro (1988)", "the folder name must be preserved");

    // The whole point: a fresh simulation now agrees the media is in place.
    let again =
        routing::run_simulation(&app.state.pool, SimulationOptions::default()).await.unwrap();
    assert_eq!(again.moves_required, 0);
    assert_eq!(again.already_correct, 1);
}

#[tokio::test]
async fn applying_triggers_a_rescan() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let recorded = arr.recorded();
    assert!(
        recorded.writes.iter().any(|w| w["name"] == "RefreshMovie"),
        "the Arr must be told to rescan the new location"
    );
}

#[tokio::test]
async fn a_rescan_can_be_turned_off() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'refresh_after_move'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert!(!arr.recorded().writes.iter().any(|w| w["name"] == "RefreshMovie"));
}

#[tokio::test]
async fn an_upstream_failure_marks_the_decision_failed_and_logs_it() {
    let arr = FakeArr::failing(500).await;
    let (app, decision_id) = ready(&arr).await;

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
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE kind = 'apply'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

/// A proposal left from before its instance was switched off is not applied:
/// the switch reads "Enabled (synced and routed)", and the revalidation finds
/// the item no longer routed.
#[tokio::test]
async fn a_proposal_for_a_disabled_instance_is_not_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;
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
    let (app, decision_id) = ready(&arr).await;
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
    let (app, decision_id) = ready(&arr).await;
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

#[tokio::test]
async fn a_superseded_decision_is_never_applied() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    sqlx::query("UPDATE decisions SET superseded = 1").execute(&app.state.pool).await.unwrap();

    let report = executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped, 1);
    assert!(arr.recorded().writes.is_empty(), "nothing may be sent upstream");
}

/// `ready`, with its proposal applied: the film sits in `/movies/anime` and
/// came from `/movies/standard`, which a revert writes into.
async fn applied(arr: &FakeArr) -> (TestApp, String) {
    let (app, decision_id) = ready(arr).await;
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

/// Records the folder a revert goes back to, as the last sync saw it.
async fn origin_folder(app: &TestApp, accessible: bool, free_space: i64) {
    sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, free_space,
         last_accessible_at)
         VALUES ('rf-1', 'inst-1', 1, '/movies/standard', ?, ?, '2026-09-20 08:00:00')",
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

/// Reverting is the one operation a user reaches for when something has
/// already gone wrong, so it goes through the route: a handler that never
/// receives the ids it is given fails in exactly the moment nobody wants a
/// surprise.
#[tokio::test]
async fn reverting_puts_the_media_back() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

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
    assert_eq!(path, "/movies/standard/Totoro (1988)");

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
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    executor::revert_decisions(
        &app.state,
        std::slice::from_ref(&decision_id),
        false,
        &executor::Confirmed::none(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
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
    let (app, decision_id) = ready(&arr).await;

    let report = executor::revert_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::none(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0);
    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn moves_to_the_same_folder_are_batched_into_one_call() {
    let arr = FakeArr::start().await;
    let (app, _) = ready(&arr).await;

    // A second movie on the same instance heading for the same folder.
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Akira', 8392,
                 '/movies/standard/Akira', '/movies/standard', 1, 1)",
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
    assert_eq!(ids.len(), 2);

    let report = executor::apply_decisions(
        &app.state,
        &ids,
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();
    assert_eq!(report.applied, 2);

    let recorded = arr.recorded();
    // The refresh command also carries `movieIds`, so match on the editor payload.
    let edits: Vec<_> =
        recorded.writes.iter().filter(|w| w["rootFolderPath"].is_string()).collect();
    assert_eq!(edits.len(), 1, "both movies share one editor call");
    assert_eq!(edits[0]["movieIds"].as_array().unwrap().len(), 2);
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

/// Answering one question must not answer the others.
///
/// Three guardrails ask through the same mechanism, and each asks under its own
/// name and lifts only that name. Read as a single boolean, confirming a
/// capacity shortfall would lift the batch threshold as well, silently, and the
/// operator would never be shown the second fact.
#[tokio::test]
async fn confirming_one_guardrail_does_not_lift_another() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;
    // Any count at all now exceeds the threshold, so both guardrails have
    // something to say about the same single decision.
    sqlx::query("UPDATE settings SET value = '0' WHERE key = 'confirmation_threshold'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let first = apply(&app, &[]).await;
    assert_eq!(first.status, 409);
    assert_eq!(
        first.json["confirm"], "capacity",
        "the refusal must name which guardrail asked: {}",
        first.json
    );

    // The capacity question is answered, and the threshold has not been asked
    // yet.
    let second = apply(&app, &["capacity"]).await;
    assert_eq!(second.status, 409, "confirming capacity applied the plan: {}", second.json);
    assert_eq!(
        second.json["confirm"], "threshold",
        "confirming one guardrail waved the other through: {}",
        second.json
    );

    // And answering a question nobody asked lifts nothing.
    let unrelated = apply(&app, &["batch"]).await;
    assert_eq!(unrelated.status, 409, "an unrelated name lifted a guardrail: {}", unrelated.json);

    let applied = apply(&app, &["capacity", "threshold"]).await;
    assert_eq!(applied.status, 200, "both answered, and it still refused: {}", applied.json);
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
async fn the_revert_route_refuses_an_unknown_decision_without_a_panic() {
    let app = TestApp::new().await;
    let body = app
        .post(
            "/api/v1/decisions/revert",
            serde_json::json!({ "decision_ids": ["nope"], "move_files": false }),
        )
        .await;
    // A refusal that says so: not a 500, and not a success over nothing.
    assert_eq!(body.status, axum::http::StatusCode::BAD_REQUEST, "got {}", body.message());
    assert!(!body.message().is_empty(), "the refusal says nothing");
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
    let (app, decision_id) = ready(&arr).await;

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

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let (root, path): (String, String) =
        sqlx::query_as("SELECT current_root_folder, current_path FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime", "the sync undid the move it did not know about");
    assert_eq!(path, "/movies/anime/Totoro (1988)");
}

/// The other half, and the one that stops the guard becoming a permanent veto:
/// once a read is newer than the move, the Arr is authoritative again. Someone
/// moving a film in Radarr's own interface must still be followed: Routarr
/// having moved it once is not a claim of ownership over it.
#[tokio::test]
async fn a_sync_that_read_after_the_move_still_follows_the_arr() {
    let arr = FakeArr::start().await;
    let (app, decision_id) = ready(&arr).await;

    executor::apply_decisions(
        &app.state,
        &[decision_id],
        false,
        &executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE media SET moved_at = datetime('now', '-1 minute') WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    sync::sync_instance(&app.state, "inst-1", "manual").await.unwrap();

    let root: String = sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(root, "/movies/standard", "upstream stopped being authoritative");
}
