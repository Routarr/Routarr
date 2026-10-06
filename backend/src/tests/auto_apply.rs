//! The guardrails around writing to an Arr with nobody watching.
//!
//! Every test here is a claim about something auto-apply must *refuse* to do,
//! except the ones that prove it does the one thing it exists for.

use crate::services::auto_apply::{self, AutoApplyOutcome};

use super::TestApp;
use super::fake_arr::FakeArr;

#[tokio::test]
async fn a_fresh_install_never_applies_on_its_own() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    // Only dry-run is turned off: auto-apply itself is left at its default.
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::Held(_)));
    assert!(arr.recorded().writes.is_empty(), "nothing may reach the Arr");
}

#[tokio::test]
async fn global_dry_run_outranks_auto_apply() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    // global_dry_run is left at its default of true.
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "webhook").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::Held(_)));
    assert!(arr.recorded().writes.is_empty(), "the master switch must win");
}

#[tokio::test]
async fn a_media_that_already_has_files_is_left_to_a_human() {
    let arr = FakeArr::start().await;
    let app = TestApp::one_film_to_move(&arr, true).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "webhook").await.unwrap();

    // The move is still proposed. It is just not applied unattended, because
    // applying it would strand the file or start a real disk move.
    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply));
    assert!(arr.recorded().writes.is_empty());

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending, 1, "the decision stays in the human queue");
}

#[tokio::test]
async fn a_media_with_no_files_yet_is_routed_without_asking() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "webhook").await.unwrap();

    let AutoApplyOutcome::Applied(report) = outcome else {
        panic!("expected the move to be applied, got {outcome:?}");
    };
    assert_eq!(report.applied, 1);
    assert_eq!(report.failed, 0);

    // It really went over the wire, to the right folder.
    let recorded = arr.recorded();
    assert_eq!(recorded.writes.len(), 1, "{:?}", recorded.writes);
    assert_eq!(recorded.writes[0]["rootFolderPath"], "/movies/anime");

    // And never asks the Arr to move files: there are none, and auto-apply is
    // defined as the case where no bytes move.
    assert_eq!(recorded.query_strings, ["moveFiles=false"], "auto-apply requested a disk move");
}

/// A film downloaded between the sync and the pass has a file the database
/// does not know about yet. Written with `moveFiles: false`, the file would stay
/// in the old folder and Radarr would report the film missing.
#[tokio::test]
async fn a_film_that_got_its_file_since_the_sync_is_not_moved_unattended() {
    // The sync saw no file, and Radarr has imported one since.
    let arr = FakeArr::start().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert!(arr.recorded().writes.is_empty(), "the film was moved without its file");
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE status = 'pending'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(pending, 1, "the decision stays in the human queue");
}

/// An item that cannot be read again may have got its file since the sync,
/// so it is left to a person, as one the Arr reports with a file is.
#[tokio::test]
async fn a_film_that_cannot_be_read_again_is_not_moved_unattended() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;
    app.execute(&["UPDATE instances SET base_url = 'http://127.0.0.1:1'"]).await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE status = 'pending'").await, 1);
}

/// A destination that is not answering is left for the person who can answer.
///
/// The routing map keeps a folder the Arr reports unreachable, since a NAS that
/// spins down is unknown rather than gone, and the manual path asks about it at
/// apply time. There is nobody to ask at three in the morning, so the
/// unattended pass holds those decisions back rather than writing into a
/// destination that is not there.
#[tokio::test]
async fn an_unattended_pass_leaves_a_sleeping_destination_alone() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    // The destination stopped answering between the simulation and the pass.
    sqlx::query("UPDATE root_folders SET accessible = 0 WHERE rtrim(path, '/') = '/movies/anime'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();

    assert!(
        matches!(outcome, AutoApplyOutcome::NothingToApply),
        "an unattended pass wrote into a folder the Arr cannot reach: {outcome:?}"
    );
    assert!(
        arr.recorded().writes.iter().all(|w| w.get("rootFolderPath").is_none()),
        "the bulk editor was called anyway"
    );
}

#[tokio::test]
async fn an_auto_applied_move_is_auditable_and_revertible() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;

    auto_apply::apply_simulation(&app.state, &simulation, crate::jobs::TRIGGER_WEBHOOK)
        .await
        .unwrap();

    // The job records who set it off, so the Tasks screen can tell an
    // unattended write apart from one the user asked for.
    let trigger: String = sqlx::query_scalar(
        "SELECT trigger FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(trigger, "webhook");

    // The decision carries the origin folder, which is what revert needs.
    let (decision_id, status, from): (String, String, Option<String>) = sqlx::query_as(
        "SELECT id, status, current_root_folder FROM decisions WHERE simulation_id = ?",
    )
    .bind(&simulation)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(status, "applied");
    assert_eq!(from.as_deref(), Some("/movies/standard"));

    let reverted = crate::services::executor::revert_decisions(
        &app.state,
        &[decision_id],
        false,
        &crate::services::executor::Confirmed::none(),
        &crate::jobs::Attribution::manual(None),
    )
    .await
    .unwrap();
    assert_eq!((reverted.applied, reverted.failed), (1, 0));
    let last_write =
        arr.recorded().writes.iter().rev().find(|w| w["rootFolderPath"].is_string()).cloned();
    assert_eq!(last_write.unwrap()["rootFolderPath"], "/movies/standard", "not moved back");
}

/// An automatic apply that finds another apply running waits its turn rather
/// than being dropped: the film would otherwise download into the folder the
/// rules do not want, the correction waiting for a sweep that may never come.
#[tokio::test]
async fn an_unattended_apply_waits_for_the_apply_already_running() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let simulation = app.simulate().await;
    let running = app.state.jobs.try_lock("apply").expect("the lock is free");

    let state = app.state.clone();
    let waiting = tokio::spawn(async move {
        auto_apply::apply_simulation(&state, &simulation, crate::jobs::TRIGGER_WEBHOOK).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(running);
    let outcome = waiting.await.unwrap();

    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied(report)) if report.applied == 1),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_sweep_larger_than_the_batch_limit_applies_nothing_at_all() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;

    // Two more films the same rule wants to move, for three candidates total.
    for (id, arr_id, title) in [("m-2", 11, "Akira"), ("m-3", 12, "Perfect Blue")] {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
             current_root_folder, monitored, has_files)
             VALUES (?, 'inst-1', ?, 'movie', ?, 8392, ?, '/movies/standard', 1, 0)",
        )
        .bind(id)
        .bind(arr_id)
        .bind(title)
        .bind(format!("/movies/standard/{title}"))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }
    app.store_setting("batch_limit", "2").await;
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();

    // Half a reorganisation is worse than none: it is all or nothing.
    assert!(
        matches!(outcome, AutoApplyOutcome::OverCap { candidates: 3, cap: 2 }),
        "got {outcome:?}"
    );
    assert!(arr.recorded().writes.is_empty(), "not one of them may be applied");

    // At the cap exactly, the run goes ahead. The fake holds Totoro alone, so
    // the two others are left to a person when they are read again.
    app.store_setting("batch_limit", "3").await;
    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "schedule").await.unwrap();
    assert!(
        matches!(&outcome, AutoApplyOutcome::Applied(report) if report.applied == 1),
        "got {outcome:?}"
    );
}

#[tokio::test]
async fn a_proposal_from_an_earlier_run_is_out_of_scope() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;

    let earlier = app.simulate().await;
    let later = app.simulate().await;
    assert_ne!(earlier, later);

    // Asking for the earlier run must apply nothing: its decision has been
    // superseded by the later one, and an unattended pass only ever writes what
    // its own run just proposed.
    let outcome = auto_apply::apply_simulation(&app.state, &earlier, "schedule").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn a_media_pointing_at_an_unmapped_category_is_not_applied() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;

    // Unmap the target: the decision becomes `skip`, not `move`.
    sqlx::query("UPDATE root_folders SET category = NULL WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let simulation = app.simulate().await;

    let outcome = auto_apply::apply_simulation(&app.state, &simulation, "webhook").await.unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert!(arr.recorded().writes.is_empty());
}

/// The scenario the whole feature exists for, driven through the real HTTP
/// route: Radarr announces a film it has just added, nothing is downloaded yet,
/// and the root folder is corrected before any file exists to move.
#[tokio::test]
async fn a_newly_added_film_is_routed_before_its_file_arrives() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;

    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "MovieAdded", "movie": { "id": 10 } }),
        )
        .await;

    let body = response.assert_ok();
    assert_eq!(body["moves_required"], 1);
    assert_eq!(body["auto_applied"], 1, "the film must be routed on the spot");

    // Radarr was told to move it, and told not to move any files. Scoped so the
    // recording lock is released before the query below awaits.
    {
        let recorded = arr.recorded();
        assert_eq!(recorded.writes[0]["rootFolderPath"], "/movies/anime");
        assert_eq!(recorded.query_strings, ["moveFiles=false"]);
    }

    // Routarr's own view followed, so the next simulation does not repropose it.
    let root: String =
        sqlx::query_scalar("SELECT current_root_folder FROM media WHERE arr_id = 10")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(root, "/movies/anime");
}

/// The same event with auto-apply left off must change nothing upstream, while
/// the webhook still does its job of proposing.
#[tokio::test]
async fn the_same_event_only_proposes_when_auto_apply_is_off() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    app.store_setting("global_dry_run", "false").await;

    let response = app
        .post(
            "/api/v1/webhook/inst-1/tok",
            serde_json::json!({ "eventType": "MovieAdded", "movie": { "id": 10 } }),
        )
        .await;

    let body = response.assert_ok();
    assert_eq!(body["moves_required"], 1, "the move is still proposed");
    assert_eq!(body["auto_applied"], 0);
    assert!(arr.recorded().writes.is_empty(), "nothing may be written while auto-apply is off");
}
