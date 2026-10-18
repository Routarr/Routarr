//! One filter for the decisions a person reviews, the moves an apply makes and those a revert
//! undoes.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;
use super::fake_arr::FakeArr;

/// The titles `/decisions` lists for `query`, in order of title.
async fn titles(app: &TestApp, query: &str) -> Vec<String> {
    let listed = app.get(&format!("/api/v1/decisions?{query}")).await;
    let mut titles: Vec<String> = listed.assert_ok()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|decision| decision["media_title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    titles
}

/// An hour from now or an hour ago, as a client sends a time.
fn hour(sign: i64) -> String {
    let at = chrono::Utc::now() + chrono::Duration::hours(sign);
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Every film of [`TestApp::films_to_move`] applied, as an apply of its whole simulation.
async fn all_applied(arr: &FakeArr, count: usize) -> TestApp {
    let app = TestApp::films_to_move(arr, count).await;
    let simulation = app.simulate().await;
    let body = json!({ "simulation_id": simulation, "confirm": ["batch"] });
    let applied = app.post("/api/v1/decisions/apply-all", body).await;
    assert_eq!(applied.assert_ok()["applied"], count);
    app
}

#[tokio::test]
async fn the_list_narrows_by_current_folder_rule_and_time() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    app.execute(&[
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('m-other', 'inst-1', 150, 'movie', 'Elsewhere', 8392, '/movies/other/Elsewhere',
                 '/movies/other', 1, 1)",
    ])
    .await;
    app.simulate().await;
    let every = ["Elsewhere", "Film 000", "Film 001"];

    assert_eq!(titles(&app, "current_root_folder=%2Fmovies%2Fother%2F").await, ["Elsewhere"]);
    assert_eq!(titles(&app, "rule_id=rule-anime").await, every);
    assert!(titles(&app, "rule_id=rule-gone").await.is_empty());
    assert_eq!(titles(&app, &format!("since={}", hour(-1))).await, every);
    assert!(titles(&app, &format!("since={}", hour(1))).await.is_empty());
    assert_eq!(titles(&app, &format!("until={}", hour(1))).await, every);
    assert!(titles(&app, &format!("until={}", hour(-1))).await.is_empty());
    assert!(titles(&app, "revertible=true").await.is_empty());
    assert_eq!(titles(&app, "revertible=false").await, every);

    let unreadable = app.get("/api/v1/decisions?since=yesterday").await;
    let refused = unreadable.assert_status(StatusCode::BAD_REQUEST);
    assert!(refused["message"].as_str().unwrap().contains("since"), "{refused}");
}

#[tokio::test]
async fn an_apply_of_a_whole_simulation_applies_only_the_moves_its_filter_keeps() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 4).await;
    let simulation = app.simulate().await;

    let body = json!({
        "simulation_id": simulation,
        "filter": { "search": "film 002" },
        "confirm": ["batch"],
    });
    let report = app.post("/api/v1/decisions/apply-all", body).await;

    let report = report.assert_ok();
    assert_eq!((&report["candidates"], &report["applied"]), (&json!(1), &json!(1)), "{report}");
    let applied: Vec<String> =
        sqlx::query_scalar("SELECT media_id FROM decisions WHERE status = 'applied'")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(applied, ["m-2"]);
}

/// The filter reads what the reviewed simulation proposed, and a newer pass only takes titles
/// out: its proposals made by another rule still apply, and a title it alone proposes does not.
#[tokio::test]
async fn a_filter_reads_the_reviewed_simulation_so_a_newer_pass_only_shrinks_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let reviewed = app.simulate().await;
    arr.hold_film(102);
    app.execute(&[
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category,
                            match_mode)
         SELECT 'rule-anime-2', 'Anime again', priority, 1, media_type, conditions,
                target_category, match_mode
           FROM rules WHERE id = 'rule-anime'",
        "UPDATE rules SET enabled = 0 WHERE id = 'rule-anime'",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id, current_path,
                            current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 102, 'movie', 'Film 002', 8392, '/movies/standard/Film 102',
                 '/movies/standard', 1, 1)",
    ])
    .await;
    app.simulate().await;

    let body = json!({
        "simulation_id": reviewed,
        "filter": { "rule_id": "rule-anime" },
        "confirm": ["batch"],
    });
    let report = app.post("/api/v1/decisions/apply-all", body).await;

    assert_eq!(report.assert_ok()["applied"], 2, "{}", report.json);
    let pending: Vec<String> = sqlx::query_scalar(
        "SELECT media_id FROM decisions WHERE status = 'pending' AND superseded = 0",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, ["m-2"], "a title the reviewed simulation never proposed was moved");
}

#[tokio::test]
async fn the_batch_question_names_the_filter() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 3).await;
    let simulation = app.simulate().await;
    let localizer = app.state.localizer().await;
    let term = |label: &str, value: &str| {
        localizer.translate(
            "FilterTerm",
            &[("label", &localizer.translate(label, &[])), ("value", value)],
        )
    };

    let body = json!({
        "simulation_id": simulation,
        "filter": {
            "instance_id": "inst-1",
            "media_type": "movie",
            "category": "anime",
            "current_root_folder": "/movies/standard",
            "rule_id": "rule-anime",
            "search": "film",
        },
    });
    let asked = app.post("/api/v1/decisions/apply-all", body).await;

    let message = asked.assert_status(StatusCode::CONFLICT)["message"].as_str().unwrap();
    let terms = [
        term("Instance", "Fake radarr"),
        term("Type", &localizer.translate("Movies", &[])),
        term("Category", "anime"),
        term("CurrentRootFolder", "/movies/standard"),
        term("Rule", "Anime"),
        term("TitleContains", "film"),
    ];
    let filtered = localizer.translate(
        "ConfirmFilteredBy",
        &[("terms", &terms.join(&localizer.translate("ListSeparator", &[])))],
    );
    assert!(message.contains(&filtered), "{message}");

    let whole = json!({ "simulation_id": simulation });
    let asked = app.post("/api/v1/decisions/apply-all", whole).await;
    let message = asked.assert_status(StatusCode::CONFLICT)["message"].as_str().unwrap();
    let named = localizer.translate("ConfirmFilteredBy", &[("terms", "")]);
    assert!(!message.contains(named.trim_end_matches('.').trim_end()), "{message}");
}

#[tokio::test]
async fn a_revert_of_what_a_filter_matches_undoes_those_moves_alone_in_slices() {
    let arr = FakeArr::start().await;
    let app = all_applied(&arr, 5).await;
    app.store_setting("batch_limit", "2").await;

    let one = json!({ "filter": { "search": "film 001" }, "confirm": ["batch"] });
    let report = app.post("/api/v1/decisions/revert-all", one).await;
    assert_eq!(report.assert_ok()["applied"], 1, "{}", report.json);
    let back: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media WHERE current_root_folder = '/movies/standard' ORDER BY id",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(back, ["m-1"]);

    let rest = app.post("/api/v1/decisions/revert-all", json!({ "confirm": ["batch"] })).await;
    let rest = rest.assert_ok();
    assert_eq!((&rest["applied"], &rest["batches_planned"]), (&json!(4), &json!(2)), "{rest}");
    assert_eq!(app.last_job_status("revert").await, "success");
    assert_eq!(
        app.count("SELECT COUNT(*) FROM media WHERE current_root_folder = '/movies/standard'")
            .await,
        5
    );
}

#[tokio::test]
async fn a_revert_of_what_a_filter_matches_asks_first_and_refuses_an_empty_match() {
    let arr = FakeArr::start().await;
    let unapplied = TestApp::films_to_move(&arr, 2).await;
    let nothing = json!({ "confirm": ["batch"] });
    unapplied
        .post("/api/v1/decisions/revert-all", nothing)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let arr = FakeArr::start().await;
    let app = all_applied(&arr, 2).await;
    let writes = arr.recorded().writes.len();
    let asked = app.post("/api/v1/decisions/revert-all", json!({ "move_files": true })).await;
    let message = asked.assert_status(StatusCode::CONFLICT)["message"].as_str().unwrap();
    let localizer = app.state.localizer().await;
    let expected = localizer.translate("ConfirmRevertAllItemsWithFiles", &[("count", "2")]);
    assert!(message.starts_with(&expected), "{message}");
    assert_eq!(asked.json["error"], "confirmation_required");
    assert_eq!(arr.recorded().writes.len(), writes, "the revert wrote before asking");

    let later = json!({ "since": hour(1), "confirm": ["batch"] });
    app.post("/api/v1/decisions/revert-all", later).await.assert_status(StatusCode::BAD_REQUEST);
    let unreadable = json!({ "until": "soon", "confirm": ["batch"] });
    app.post("/api/v1/decisions/revert-all", unreadable)
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    app.store_setting("global_dry_run", "true").await;
    let held = app.post("/api/v1/decisions/revert-all", json!({ "confirm": ["batch"] })).await;
    assert_eq!(held.assert_status(StatusCode::CONFLICT)["reason"], "dry_run", "{}", held.json);
    assert_eq!(arr.recorded().writes.len(), writes);
}

/// The simulation and the times narrow a revert as they narrow the list, and the question
/// names a bound already past.
#[tokio::test]
async fn a_revert_is_narrowed_by_its_simulation_and_its_times() {
    let arr = FakeArr::start().await;
    let app = all_applied(&arr, 3).await;
    app.execute(&[
        "UPDATE decisions SET decided_at = '2026-01-01 00:00:00' WHERE media_id = 'm-0'",
    ])
    .await;
    let simulation: String =
        sqlx::query_scalar("SELECT simulation_id FROM decisions WHERE media_id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();

    let elsewhere = json!({ "simulation_id": "another", "confirm": ["batch"] });
    let refused = app.post("/api/v1/decisions/revert-all", elsewhere).await;
    refused.assert_status(StatusCode::BAD_REQUEST);

    let older = json!({
        "simulation_id": simulation,
        "since": "2025-01-01T00:00:00Z",
        "until": "2026-06-01T00:00:00Z",
    });
    let asked = app.post("/api/v1/decisions/revert-all", older.clone()).await;
    let message = asked.assert_status(StatusCode::CONFLICT)["message"].as_str().unwrap();
    let localizer = app.state.localizer().await;
    assert!(message.contains(&localizer.translate("DecidedFrom", &[])), "{message}");
    assert!(message.contains(&localizer.translate("DecidedBefore", &[])), "{message}");
    assert!(message.contains(&localizer.translate("Simulation", &[])), "{message}");

    let mut confirmed = older;
    confirmed["confirm"] = json!(["batch"]);
    let report = app.post("/api/v1/decisions/revert-all", confirmed).await;
    assert_eq!(report.assert_ok()["applied"], 1, "{}", report.json);
    let back: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media WHERE current_root_folder = '/movies/standard' ORDER BY id",
    )
    .fetch_all(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(back, ["m-0"]);
}
