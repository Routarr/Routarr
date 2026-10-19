//! The guardrails around writing to an Arr with nobody watching.

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await
    .unwrap();

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await
    .unwrap();

    // Still proposed, and left to a person: a move would strand the file or move its bytes.
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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await
    .unwrap();

    let AutoApplyOutcome::Applied { report, .. } = outcome else {
        panic!("expected the move to be applied, got {outcome:?}");
    };
    assert_eq!(report.applied, 1);
    assert_eq!(report.failed, 0);

    let recorded = arr.recorded();
    assert_eq!(recorded.writes.len(), 1, "{:?}", recorded.writes);
    assert_eq!(recorded.writes[0]["rootFolderPath"], "/movies/anime");

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert_eq!(app.count("SELECT COUNT(*) FROM decisions WHERE status = 'pending'").await, 1);
}

/// A destination that is not answering is left for the person who can answer: the manual
/// path asks about a sleeping NAS, and nobody is there to ask at three in the morning.
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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

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

    auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_WEBHOOK),
    )
    .await
    .unwrap();

    // Recorded as the automation's, so the Tasks screen and the move log tell it apart.
    let trigger: String = sqlx::query_scalar(
        "SELECT trigger FROM jobs WHERE kind = 'apply' ORDER BY started_at DESC",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(trigger, "auto");

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
        auto_apply::apply_simulation(
            &state,
            &simulation,
            &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_WEBHOOK),
        )
        .await
    });
    let jobs = &app.state.jobs;
    super::until(move || async move { jobs.queued_for("apply") == 1 }).await;
    drop(running);
    let outcome = waiting.await.unwrap();

    assert!(
        matches!(&outcome, Ok(AutoApplyOutcome::Applied { report, .. }) if report.applied == 1),
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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

    // Half a reorganisation is worse than none: it is all or nothing.
    let AutoApplyOutcome::OverCap(held) = &outcome else {
        panic!("expected every move held, got {outcome:?}");
    };
    assert_eq!((held.len(), held[0].candidates, held[0].cap), (1, 3, 2));
    assert!(arr.recorded().writes.is_empty(), "not one of them may be applied");

    // At the cap it goes ahead: the fake holds Totoro alone, so the two others are left.
    app.store_setting("batch_limit", "3").await;
    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();
    assert!(
        matches!(&outcome, AutoApplyOutcome::Applied { report, .. } if report.applied == 1),
        "got {outcome:?}"
    );
}

/// An instance kept to proposals keeps its moves for a person while another routes on its own,
/// as a 4K Radarr beside a 1080p one.
#[tokio::test]
async fn an_instance_kept_to_proposals_is_left_while_another_is_applied() {
    let arr = FakeArr::with_unimported_movie().await;
    let other = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.seed_second_film_to_move(&other).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.execute(&["UPDATE instances SET auto_apply = 0 WHERE id = 'inst-2'"]).await;
    let simulation = app.simulate().await;

    auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

    assert_eq!(arr.recorded().writes.len(), 1, "the instance left on was not applied");
    assert!(other.recorded().writes.is_empty(), "the instance kept to proposals was written to");
    let pending: String = sqlx::query_scalar(
        "SELECT status FROM decisions WHERE media_id = 'm-2' AND superseded = 0",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, "pending");
}

/// The batch limit is counted per instance: one instance's backlog holds its own moves, not
/// another's, and the warning names the instance held.
#[tokio::test]
async fn one_instance_over_the_batch_limit_holds_only_its_own_moves() {
    let arr = FakeArr::with_unimported_movie().await;
    let other = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.seed_second_film_to_move(&other).await;
    for (id, arr_id, title) in [("m-3", 11, "Akira"), ("m-4", 12, "Perfect Blue")] {
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
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.store_setting("batch_limit", "2").await;
    let simulation = app.simulate().await;

    auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

    assert!(arr.recorded().writes.is_empty(), "the instance over the limit was written to");
    assert_eq!(other.recorded().writes.len(), 1, "the instance within the limit was held too");
    let answer = app.get("/api/v1/status").await;
    let status = answer.assert_ok();
    let held: Vec<String> = status["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|warning| warning["code"] == "auto_apply_held")
        .map(|warning| warning["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(held.len(), 1, "{status}");
    assert!(held[0].contains("Fake radarr") && !held[0].contains("inst-2"), "{}", held[0]);
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

    // Superseded by the later run: a pass writes only what its own run just proposed.
    let outcome = auto_apply::apply_simulation(
        &app.state,
        &earlier,
        &crate::jobs::Attribution::unattended("schedule"),
    )
    .await
    .unwrap();

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

    let outcome = auto_apply::apply_simulation(
        &app.state,
        &simulation,
        &crate::jobs::Attribution::unattended("webhook"),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, AutoApplyOutcome::NothingToApply), "got {outcome:?}");
    assert!(arr.recorded().writes.is_empty());
}

/// An automatic move waits for an apply already running, which can take
/// minutes, while an Arr gives a delivery far less. The delivery is answered
/// in time, and the move is made once that apply ends.
#[tokio::test]
async fn a_webhook_answers_before_the_apply_it_waits_for() {
    use std::time::Duration;

    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let app = app.with_webhook_answer_wait(Duration::from_millis(200));
    let running = app.state.jobs.try_lock("apply").expect("the apply lock is free");

    let delivery = serde_json::json!({ "eventType": "MovieAdded", "movie": { "id": 10 } });
    let answered = tokio::time::timeout(
        Duration::from_secs(3),
        app.post("/api/v1/webhook/inst-1/tok", delivery),
    )
    .await
    .expect("the delivery waited for the apply before answering");

    assert_eq!(answered.status, 202, "{}", answered.json);
    assert!(arr.recorded().writes.is_empty(), "moved while another apply ran");
    drop(running);
    super::webhook_settled(&app, "inst-1").await;
    let applied = app
        .count(
            "SELECT COUNT(*) FROM decisions WHERE media_id = 'm-inst-1-10' AND status = 'applied'",
        )
        .await;
    assert_eq!(applied, 1, "the move was dropped with the delivery's answer");
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

    // Scoped so the recording lock is released before the query below awaits.
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

/// A title whose move failed within the day is left to a person rather than retried, failed
/// and notified every quarter of an hour, and is tried again the day after.
#[tokio::test]
async fn a_title_whose_move_failed_is_not_retried_unattended_within_the_day() {
    let arr = FakeArr::refusing_unimported_movie(500).await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let pass = || async {
        let simulation = app.simulate().await;
        let by = crate::jobs::Attribution::unattended("schedule");
        auto_apply::apply_simulation(&app.state, &simulation, &by).await.unwrap();
        arr.recorded().query_strings.len()
    };

    let tried = pass().await;
    assert!(tried > 0, "the first pass did not try the move");
    assert_eq!(pass().await, tried, "a move that just failed was tried again unattended");

    app.execute(&["UPDATE decisions SET decided_at = datetime('now', '-2 days')
                   WHERE status = 'failed'"])
        .await;
    assert!(pass().await > tried, "a day later the move is not tried again");
}

/// A failing rule test says a rule change broke a routing someone pinned, and a case that
/// cannot be read fails: nothing moves unattended until the suite passes. Removed, the same
/// delivery routes the film.
#[tokio::test]
async fn a_failing_rule_test_holds_every_unattended_move() {
    let arr = FakeArr::with_unimported_movie().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    app.seed_route_to_anime().await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    sqlx::query(
        "INSERT INTO rule_tests (id, name, media_type, media_json, evaluated_at, expected_category)
         VALUES ('t-1', 'Unreadable', 'movie', '{}', '2026-01-01 00:00:00', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let added = serde_json::json!({ "eventType": "MovieAdded", "movie": { "id": 10 } });

    let held = app.post("/api/v1/webhook/inst-1/tok", added.clone()).await;

    assert_eq!(held.assert_ok()["moves_required"], 1);
    assert_eq!(held.json["auto_applied"], 0, "a failing case holds the move");
    assert!(arr.recorded().writes.is_empty());

    app.delete("/api/v1/rule-tests/t-1").await.assert_ok();
    let routed = app.post("/api/v1/webhook/inst-1/tok", added).await;
    assert_eq!(routed.assert_ok()["auto_applied"], 1, "the positive control");
}

/// An instance takes automatic application unless told otherwise, says so, and each change of it
/// goes to the security log, as the global switch's does: it decides where Routarr writes alone.
#[tokio::test]
async fn an_instance_s_automatic_application_is_read_written_and_audited() {
    let app = TestApp::new().await;
    let body = |auto_apply: Option<bool>| {
        let mut body = serde_json::json!({
            "name": "Radarr 4K", "instance_type": "radarr",
            "base_url": "http://radarr:7878", "api_key": "k",
        });
        if let Some(on) = auto_apply {
            body["auto_apply"] = serde_json::json!(on);
        }
        body
    };

    let created = app.post("/api/v1/instances", body(None)).await;
    let created = created.assert_ok().clone();
    assert_eq!(created["auto_apply"], true);
    let id = created["id"].as_str().unwrap();
    for _ in 0..2 {
        let updated = app.put(&format!("/api/v1/instances/{id}"), body(Some(false))).await;
        assert_eq!(updated.assert_ok()["auto_apply"], false);
    }
    let read = app.get(&format!("/api/v1/instances/{id}")).await;
    assert_eq!(read.assert_ok()["auto_apply"], false);

    app.state.audit.flush().await;
    let logged: Vec<(String, String)> = sqlx::query_as(
        "SELECT message, params FROM security_events WHERE message LIKE 'AuditInstanceAutoApply%'",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(logged.len(), 1, "one change, one entry: {logged:?}");
    assert_eq!(logged[0].0, "AuditInstanceAutoApplyOff");
    assert!(logged[0].1.contains("Radarr 4K"), "{logged:?}");
}

/// An upgrade leaves every instance to automatic application, so nobody's routing changes: the
/// global switch still decides as it did.
#[tokio::test]
async fn an_upgrade_leaves_every_instance_to_automatic_application() {
    let pool = super::database_of_0_1_5().await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key)
         VALUES ('i', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k')",
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let auto_apply: bool =
        sqlx::query_scalar("SELECT auto_apply FROM instances").fetch_one(&pool).await.unwrap();
    assert!(auto_apply);
}

/// A pass over one instance, as a webhook's, settles that instance's hold alone: the other
/// instance's backlog is still waiting and still warned of.
#[tokio::test]
async fn a_pass_over_one_instance_leaves_another_s_hold_standing() {
    let arr = FakeArr::with_unimported_movie().await;
    let other = FakeArr::with_unimported_movie().await;
    let app = TestApp::one_film_to_move(&arr, false).await;
    app.seed_second_film_to_move(&other).await;
    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    app.store_setting("batch_limit", "1").await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('m-3', 'inst-1', 11, 'movie', 'Akira', 8392, '/movies/standard/Akira',
                 '/movies/standard', 1, 0)",
    ])
    .await;
    let by = crate::jobs::Attribution::unattended("schedule");
    let library = app.simulate().await;
    auto_apply::apply_simulation(&app.state, &library, &by).await.unwrap();
    let held = || app.state.auto_apply_held.lock().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(held(), ["inst-1"]);

    // A webhook simulates its own instance alone.
    let options = crate::services::routing::SimulationOptions {
        persist: true,
        instance_ids: vec!["inst-2".to_string()],
        ..Default::default()
    };
    let scoped = crate::services::routing::run_simulation(
        &app.state.pool,
        &app.state.library_passes,
        options,
    )
    .await
    .unwrap()
    .simulation_id;
    let webhook = crate::jobs::Attribution::unattended("webhook");
    auto_apply::apply_instance_simulation(&app.state, &scoped, "inst-2", &webhook).await.unwrap();
    assert_eq!(held(), ["inst-1"], "a pass over inst-2 settled inst-1's hold");
}
