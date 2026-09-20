//! Simulation-level tests: preloading, actions, supersede and persistence.

use crate::jobs::Attribution;
use crate::services::routing::{self, SimulationOptions};

use super::TestApp;

async fn simulate(app: &TestApp, options: SimulationOptions) -> crate::models::SimulationResult {
    routing::run_simulation(&app.state.pool, options).await.unwrap()
}

fn persisting() -> SimulationOptions {
    SimulationOptions { persist: true, ..Default::default() }
}

#[tokio::test]
async fn a_matching_rule_produces_a_move() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let result = simulate(&app, persisting()).await;

    assert_eq!(result.total_media, 1);
    assert_eq!(result.moves_required, 1);
    assert_eq!(result.decisions[0].action, "move");
    assert_eq!(result.decisions[0].target_root_folder.as_deref(), Some("/movies/anime"));
    assert_eq!(result.decisions[0].matched_rule_name.as_deref(), Some("Anime"));
    assert!(result.decisions[0].confidence > 0.5);
}

#[tokio::test]
async fn media_already_in_place_needs_no_move() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // With no rule the item falls back to `standard`, which is where it lives.
    let result = simulate(&app, persisting()).await;

    assert_eq!(result.already_correct, 1);
    assert_eq!(result.moves_required, 0);
    assert_eq!(result.no_category_match, 1);
}

#[tokio::test]
async fn unchanged_decisions_are_not_persisted_by_default() {
    let app = TestApp::new().await;
    app.seed_library().await;

    simulate(&app, persisting()).await;

    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, 0, "'nothing to do' rows would flood the table on every run");
}

#[tokio::test]
async fn unchanged_decisions_can_be_persisted_on_request() {
    let app = TestApp::new().await;
    app.seed_library().await;

    simulate(
        &app,
        SimulationOptions { persist: true, persist_unchanged: true, ..Default::default() },
    )
    .await;

    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn a_category_without_a_root_folder_is_skipped_not_moved() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query("UPDATE root_folders SET category = NULL WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = simulate(&app, persisting()).await;

    assert_eq!(result.skipped_unmapped, 1);
    assert_eq!(result.moves_required, 0);
    assert_eq!(result.decisions[0].action, "skip");
    assert!(result.decisions[0].target_root_folder.is_none());
}

/// The library pass loads what a rule can match on, and nothing else.
///
/// `MetadataField` names five fields; the status, the synopsis and the poster
/// are matchable by no condition and the pass never opens them. Read whole they
/// were 53% of a cache row on a development library, held in memory for the
/// duration of every simulation — and a TMDb overview is several times longer
/// than the ones measured.
#[tokio::test]
async fn the_library_pass_does_not_carry_what_no_rule_can_read() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "INSERT INTO metadata_cache
            (source, external_id, media_type, genres, keywords, original_language,
             origin_countries, certification, status, overview, poster_path, cached_at, expires_at)
         VALUES ('tmdb', '129', 'movie', '[\"Animation\"]', '[]', 'ja', '[\"JP\"]',
                 'PG', 'released', 'A very long synopsis nobody can match on.',
                 '/poster.jpg', datetime('now'), datetime('now', '+7 days'))",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let cache = crate::services::metadata::load_cache(&app.state.pool).await.unwrap();
    let answer = cache
        .get(&("tmdb".to_string(), "129".to_string(), "movie".to_string()))
        .expect("the row is in the cache");

    // What a condition reads is there...
    assert_eq!(answer.genres, vec!["Animation"]);
    assert_eq!(answer.original_language.as_deref(), Some("ja"));
    assert_eq!(answer.certification.as_deref(), Some("PG"));
    // ...and what none of them can read was never fetched. The explanation
    // panel gets these from the per-item path instead.
    assert!(answer.overview.is_none(), "the pass carried a synopsis it cannot match on");
    assert!(answer.status.is_none());
    assert!(answer.poster_path.is_none());
}

/// A root folder on a NAS that has spun down is reported inaccessible, and
/// that is *unknown*, not gone.
///
/// Read as absent it left the category unmapped, so everything bound for it
/// became "skip" — indistinguishable on screen from a category nobody mapped —
/// and the rerun then superseded a plan built while the disk was awake. The
/// question of whether the destination can be written to is asked at apply
/// time instead, where it can be answered.
#[tokio::test]
async fn a_root_folder_that_is_asleep_still_routes() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query("UPDATE root_folders SET accessible = 0 WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(
        result.decisions[0].action, "move",
        "a sleeping destination unmapped its category: {:?}",
        result.decisions[0]
    );
    assert_eq!(result.skipped_unmapped, 0, "and it was counted as an unmapped category");
}

/// The plan built while the disk was awake has to survive the nap.
///
/// Rerunning marks prior `pending` decisions `superseded`, which is what stops
/// two contradictory proposals both being applied. When the destination
/// vanished from the map, that guard destroyed the good plan instead of
/// replacing it.
#[tokio::test]
async fn a_pass_run_while_the_disk_sleeps_keeps_the_plan() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let awake = simulate(&app, persisting()).await;
    assert_eq!(awake.moves_required, 1, "the fixture has to propose something to lose");

    sqlx::query("UPDATE root_folders SET accessible = 0 WHERE id = 'rf-2'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let asleep = simulate(&app, persisting()).await;

    assert_eq!(asleep.moves_required, 1, "the nap threw the plan away");
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "nothing is left to apply once the disk wakes");
}

#[tokio::test]
async fn a_trailing_slash_does_not_create_a_phantom_move() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query("UPDATE media SET current_root_folder = '/movies/standard/' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(result.already_correct, 1, "'/x' and '/x/' are the same folder");
}

#[tokio::test]
async fn an_override_wins_over_the_rules() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, locked)
         VALUES ('o-1', 'm-1', 'standard', 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let result = simulate(&app, persisting()).await;

    assert_eq!(result.overrides_applied, 1);
    assert_eq!(result.already_correct, 1);
    assert!(result.decisions[0].is_override);
    assert_eq!(result.decisions[0].confidence, 1.0);
}

#[tokio::test]
async fn rerunning_supersedes_the_previous_proposal() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    simulate(&app, persisting()).await;
    simulate(&app, persisting()).await;

    let (total, current): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), SUM(CASE WHEN superseded = 0 THEN 1 ELSE 0 END) FROM decisions",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    assert_eq!(total, 2, "history is kept");
    assert_eq!(current, 1, "only one proposal may be actionable at a time");
}

#[tokio::test]
async fn a_media_that_becomes_correct_loses_its_pending_proposal() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let first = simulate(&app, persisting()).await;
    assert_eq!(first.moves_required, 1);

    // The rule that justified the move is switched off, so the media is now
    // exactly where it belongs and produces no decision worth storing.
    sqlx::query("UPDATE rules SET enabled = 0").execute(&app.state.pool).await.unwrap();
    let second = simulate(&app, persisting()).await;
    assert_eq!(second.moves_required, 0);
    assert_eq!(second.already_correct, 1);

    let actionable: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();

    assert_eq!(actionable, 0, "a proposal no rule justifies any more must not stay applicable");
}

#[tokio::test]
async fn a_stale_proposal_cannot_be_applied_after_the_rules_change() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let proposal = simulate(&app, persisting()).await.decisions[0].id.clone();

    sqlx::query("DELETE FROM rules").execute(&app.state.pool).await.unwrap();
    simulate(&app, persisting()).await;

    let report = crate::services::executor::apply_decisions(
        &app.state,
        &[proposal],
        false,
        &crate::services::executor::Confirmed::all(),
        &Attribution::manual(None),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0, "the move must be refused");
    assert_eq!(report.skipped, 1);

    let folder: String =
        sqlx::query_scalar("SELECT current_root_folder FROM media WHERE id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(folder, "/movies/standard", "the media must not have moved");
}

#[tokio::test]
async fn simulations_are_grouped_by_id() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let first = simulate(&app, persisting()).await;
    let second = simulate(&app, persisting()).await;
    assert_ne!(first.simulation_id, second.simulation_id);

    let stored: String =
        sqlx::query_scalar("SELECT simulation_id FROM decisions WHERE superseded = 0")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, second.simulation_id);
}

#[tokio::test]
async fn filtering_by_media_type_happens_in_sql() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let result = simulate(
        &app,
        SimulationOptions { media_type: Some("series".into()), ..Default::default() },
    )
    .await;
    assert_eq!(result.total_media, 0);
}

#[tokio::test]
async fn filtering_by_instance_happens_in_sql() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let result = simulate(
        &app,
        SimulationOptions { instance_ids: vec!["other".into()], ..Default::default() },
    )
    .await;
    assert_eq!(result.total_media, 0);

    let all = simulate(
        &app,
        SimulationOptions { instance_ids: vec!["inst-1".into()], ..Default::default() },
    )
    .await;
    assert_eq!(all.total_media, 1);
}

/// An empty instance list is every instance, the way `Rule::covers_instance`
/// reads the same shape. Read as "these zero instances", `POST /simulate` with
/// `instance_ids: []` evaluated nothing and reported a green run.
#[tokio::test]
async fn an_empty_instance_list_evaluates_every_instance() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let result =
        simulate(&app, SimulationOptions { instance_ids: vec![], ..Default::default() }).await;
    assert_eq!(result.total_media, 1);
}

/// An empty media list is these zero items, not no filter. Read as no filter,
/// a webhook whose item had just been deleted evaluated — and persisted, and
/// automatically applied — the whole instance.
#[tokio::test]
async fn an_empty_media_list_evaluates_nothing() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let result =
        simulate(&app, SimulationOptions { media_ids: Some(vec![]), ..Default::default() }).await;
    assert_eq!(result.total_media, 0);
}

#[tokio::test]
async fn a_media_id_filter_scopes_the_run() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
         monitored, has_files) VALUES ('m-2', 'inst-1', 99, 'movie', 'Akira', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let all = simulate(&app, SimulationOptions::default()).await;
    assert_eq!(all.total_media, 2);

    let scoped = simulate(
        &app,
        SimulationOptions { media_ids: Some(vec!["m-2".into()]), ..Default::default() },
    )
    .await;
    assert_eq!(scoped.total_media, 1);
    assert_eq!(scoped.decisions[0].media_id, "m-2");
}

#[tokio::test]
async fn the_response_can_be_truncated_without_losing_the_counters() {
    let app = TestApp::new().await;
    app.seed_library().await;

    for i in 2..12 {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
             monitored, has_files) VALUES (?, 'inst-1', ?, 'movie', ?, '/movies/standard', 1, 1)",
        )
        .bind(format!("m-{i}"))
        .bind(100 + i as i64)
        .bind(format!("Movie {i}"))
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    let result =
        simulate(&app, SimulationOptions { max_returned: Some(3), ..Default::default() }).await;

    assert_eq!(result.total_media, 11);
    assert_eq!(result.returned, 3);
    assert_eq!(result.decisions.len(), 3);
}

#[tokio::test]
async fn excluded_rules_are_reported_as_alternatives() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, exclusions,
         target_category, match_mode)
         VALUES ('r-1', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Family\"]}]',
                 'anime', 'all')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let result = simulate(&app, persisting()).await;

    assert_eq!(result.excluded_by_rule, 1);
    assert_eq!(result.no_category_match, 1, "the excluded rule must not win");
    assert_eq!(result.decisions[0].target_category, "standard");
    assert!(result.decisions[0].alternatives[0].excluded_by.is_some());
}

#[tokio::test]
async fn media_without_metadata_still_gets_a_decision() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    // Every source silenced: no fetched answer, and no genres from the Arr
    // either. The item is then genuinely unknown, not merely un-enriched.
    sqlx::query("DELETE FROM metadata_cache").execute(&app.state.pool).await.unwrap();
    sqlx::query("UPDATE media SET genres = NULL, original_language = NULL, certification = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = simulate(&app, persisting()).await;

    assert_eq!(result.total_media, 1);
    assert_eq!(result.no_category_match, 1);
    assert!(result.decisions[0].reasons[0].contains("No rule matched"));
}

#[tokio::test]
async fn the_default_category_setting_is_honoured() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query("UPDATE settings SET value = 'anime' WHERE key = 'default_category'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(result.decisions[0].target_category, "anime");
    assert_eq!(result.moves_required, 1);
}

#[tokio::test]
async fn a_disabled_rule_does_not_route() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    sqlx::query("UPDATE rules SET enabled = 0").execute(&app.state.pool).await.unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(result.no_category_match, 1);
}

#[tokio::test]
async fn a_corrupted_condition_payload_disables_the_rule_instead_of_failing_the_run() {
    let app = TestApp::new().await;
    app.seed_library().await;

    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category)
         VALUES ('broken', 'Broken', 1, 1, 'both', 'not json', 'anime')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(result.total_media, 1, "one bad row must not abort the whole simulation");
    assert_eq!(result.no_category_match, 1);
}

/// Two library-wide passes both supersede the other's pending decisions, and
/// the later commit wins — so the survivor may have been computed from a rule
/// set that changed in between. The webhook's single-item run must *not* queue
/// behind a sweep: `store_decisions` supersedes only what it evaluated, and a
/// season import arrives as one delivery per episode.
#[tokio::test]
async fn a_full_simulation_refuses_a_second_one_and_never_blocks_the_webhook() {
    let app = crate::tests::TestApp::new().await;
    app.seed_library().await;

    let running = app.state.jobs.try_lock(crate::jobs::FULL_SIMULATION).expect("the key is free");

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true }))
        .await
        .assert_status(axum::http::StatusCode::CONFLICT);

    // The webhook path takes no such lock, so it still evaluates its one item.
    let scoped = routing::run_simulation(
        &app.state.pool,
        routing::SimulationOptions {
            trigger: crate::jobs::TRIGGER_WEBHOOK.to_string(),
            media_ids: Some(vec!["m-1".to_string()]),
            persist: true,
            ..Default::default()
        },
    )
    .await;
    assert!(scoped.is_ok(), "a scoped run was blocked by a full one");

    drop(running);
    app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await.assert_ok();
}
