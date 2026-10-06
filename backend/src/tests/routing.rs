//! Simulation-level tests: preloading, actions, supersede and persistence.

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
    assert_eq!(result.decisions[0].action.as_str(), "move");
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
    assert_eq!(result.decisions[0].action.as_str(), "skip");
    assert!(result.decisions[0].target_root_folder.is_none());
}

/// The library pass loads what a rule can match on, and nothing else.
///
/// `MetadataField` names five fields. The status, the synopsis and the poster
/// are matchable by no condition and the pass never opens them. Read whole,
/// they would make up much of every cache row, held in memory for the duration
/// of every simulation.
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

    let cache = crate::services::metadata::load_cache(&mut app.state.pool.acquire().await.unwrap())
        .await
        .unwrap();
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
/// Read as absent, it would leave the category unmapped, so everything bound
/// for it would become "skip", indistinguishable on screen from a category
/// nobody mapped, and the rerun would supersede a plan built while the disk was
/// awake.
/// The question of whether the destination can be written to is asked at apply
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
        result.decisions[0].action.as_str(),
        "move",
        "a sleeping destination unmapped its category: {:?}",
        result.decisions[0]
    );
    assert_eq!(result.skipped_unmapped, 0, "and it was counted as an unmapped category");
}

/// The plan built while the disk was awake has to survive the nap.
///
/// Rerunning marks prior `pending` decisions `superseded`, which is what stops
/// two contradictory proposals both being applied. A destination that vanishes
/// from the map would turn that guard into one that destroys the good plan
/// instead of replacing it.
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
        "INSERT INTO overrides (id, media_id, target_category)
         VALUES ('o-1', 'm-1', 'standard')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let stored = SimulationOptions { persist_unchanged: true, ..persisting() };
    let result = simulate(&app, stored).await;

    assert_eq!(result.overrides_applied, 1);
    assert_eq!(result.already_correct, 1);
    assert!(result.decisions[0].is_override);
    assert_eq!(result.decisions[0].confidence, 1.0);
    // Stored as the exception it is, not as the rule it outranked.
    let (rule, reasons): (Option<String>, String) =
        sqlx::query_as("SELECT matched_rule_name, reasons FROM decisions")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    let localizer = app.state.localizer().await;
    assert_eq!(rule, Some(localizer.translate("ManualOverrideRuleName", &[])));
    let reasons: Vec<String> = serde_json::from_str(&reasons).unwrap();
    assert_eq!(reasons, [localizer.translate("ReasonManualOverride", &[])]);
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
    app.execute(&["INSERT INTO media (id, instance_id, arr_id, media_type, title,
                                      current_root_folder)
                   VALUES ('m-2', 'inst-1', 2, 'series', 'Trigun', '/tv/standard')"])
        .await;

    for (kind, titles) in [
        (Some("series"), ["Trigun"].as_slice()),
        (Some("movie"), &["My Neighbor Totoro"]),
        (None, &["My Neighbor Totoro", "Trigun"]),
    ] {
        let options =
            SimulationOptions { media_type: kind.map(str::to_string), ..Default::default() };
        let result = simulate(&app, options).await;
        let mut evaluated: Vec<&str> =
            result.decisions.iter().map(|d| d.media_title.as_str()).collect();
        evaluated.sort_unstable();
        assert_eq!(evaluated, titles, "{kind:?}");
    }
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

/// An empty media list is these zero items, not no filter. Read as no filter,
/// a webhook whose item has just been deleted would evaluate, persist and
/// automatically apply the whole instance.
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

/// Trimmed, the answer keeps what someone has to act on: the moves, then the
/// skips, then the items already in place. Cut in the order the library is
/// read, by title, the three items sorting first would crowd both out.
#[tokio::test]
async fn a_truncated_response_keeps_the_moves_then_the_skips() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    for statement in [
        // Routed to `kids`, which no folder is mapped to: a skip.
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('rule-kids', 'Kids', 5, 1, 'both',
                 '[{\"type\":\"title_contains\",\"value\":[\"Kids\"]}]', 'kids', 'all')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
         monitored, has_files)
         VALUES ('m-a1', 'inst-1', 101, 'movie', 'A Film 1', '/movies/standard', 1, 1),
                ('m-a2', 'inst-1', 102, 'movie', 'A Film 2', '/movies/standard', 1, 1),
                ('m-a3', 'inst-1', 103, 'movie', 'A Film 3', '/movies/standard', 1, 1),
                ('m-kids', 'inst-1', 104, 'movie', 'B Kids Film', '/movies/standard', 1, 1)",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }

    let result =
        simulate(&app, SimulationOptions { max_returned: Some(2), ..Default::default() }).await;

    let kept: Vec<(&str, &str)> =
        result.decisions.iter().map(|d| (d.media_id.as_str(), d.action.as_str())).collect();
    assert_eq!(kept, [("m-1", "move"), ("m-kids", "skip")]);
    assert_eq!(result.total_media, 5);
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

/// "Enabled (synced and routed)": switched off, an instance is not routed
/// either, so none of its items is proposed.
#[tokio::test]
async fn a_disabled_instance_is_not_simulated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    assert_eq!(simulate(&app, persisting()).await.moves_required, 1, "the rule moves the film");

    sqlx::query("UPDATE instances SET enabled = 0").execute(&app.state.pool).await.unwrap();

    let result = simulate(&app, persisting()).await;
    assert_eq!(
        (result.total_media, result.moves_required),
        (0, 0),
        "a disabled instance was routed"
    );
}

/// A rule "Anime, except Family" whose exclusions cannot be read is not the
/// rule "Anime": dropped in silence, the exclusions would let it move the very
/// titles they kept out. It matches nothing until they can be read.
#[tokio::test]
async fn an_unreadable_exclusion_keeps_the_rule_from_matching() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    assert_eq!(simulate(&app, persisting()).await.moves_required, 1, "the rule moves the film");

    sqlx::query("UPDATE rules SET exclusions = '[{\"type\":\"a_kind_this_build_lacks\"}]'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    assert_eq!(simulate(&app, persisting()).await.moves_required, 0, "the rule widened");
}

/// A scope that cannot be read is not every instance.
#[tokio::test]
async fn an_unreadable_instance_list_scopes_the_rule_to_nothing() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    assert_eq!(simulate(&app, persisting()).await.moves_required, 1, "the rule moves the film");

    sqlx::query("UPDATE rules SET instance_ids = 'not json'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    assert_eq!(simulate(&app, persisting()).await.moves_required, 0, "the rule widened");
}

/// Two library-wide passes at once evaluate the library twice for one result.
/// The webhook's single-item run must *not* queue behind a sweep:
/// `routing::store_run` touches only what it evaluated, and a season import
/// arrives as one delivery per episode.
#[tokio::test]
async fn a_full_simulation_refuses_a_second_one_and_never_blocks_the_webhook() {
    let arr = crate::tests::fake_arr::FakeArr::start().await;
    let app = crate::tests::TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let running = app.state.jobs.try_lock(crate::jobs::FULL_SIMULATION).expect("the key is free");

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true }))
        .await
        .assert_status(axum::http::StatusCode::CONFLICT);

    // The webhook takes no such lock, so a delivery still evaluates its item.
    let event = serde_json::json!({
        "eventType": "Download",
        "movie": { "id": 10, "tmdbId": 8392, "title": "Totoro" },
    });
    let delivery = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.post("/api/v1/webhook/inst-1/tok", event),
    )
    .await
    .expect("the delivery waited for the full simulation");
    assert!(!delivery.assert_ok()["media_id"].is_null(), "nothing evaluated: {}", delivery.json);

    drop(running);
    app.post("/api/v1/simulate", serde_json::json!({ "persist": true })).await.assert_ok();
}

/// The explanation panel names the category, the folder and the action the
/// simulation proposes, whatever road the item takes to get there.
///
/// A panel naming a folder the simulation does not propose breaks the promise
/// that every decision explains itself. Each item reaches its folder by a road
/// a second spelling of the decision could read differently: an exclusion
/// handing it to the next rule and a folder declared here rather than reported
/// by the Arr, a folder the Arr reports asleep, and an override onto the folder
/// the item is already in, stored with a trailing slash.
#[tokio::test]
async fn the_explanation_names_the_folder_the_simulation_proposes() {
    let app = TestApp::new().await;
    app.seed_library().await;
    for statement in [
        "INSERT INTO categories (id, name) VALUES ('cat-kids', 'kids')",
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category, origin)
         VALUES ('rf-kids', 'inst-1', NULL, '/movies/kids', 1, 'kids', 'declared')",
        "UPDATE root_folders SET accessible = 0 WHERE id = 'rf-2'",
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, exclusions,
         target_category, match_mode)
         VALUES ('rule-anime', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Family\"]}]', 'anime', 'all'),
                ('rule-family', 'Family', 20, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Family\"]}]', '[]', 'kids', 'all')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_path,
         current_root_folder, monitored, has_files, genres)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Akira', '/movies/standard/Akira',
                 '/movies/standard', 1, 1, '[\"Animation\"]'),
                ('m-3', 'inst-1', 12, 'movie', 'Paprika', '/movies/anime/Paprika',
                 '/movies/anime/', 1, 1, '[]')",
        "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-3', 'm-3', 'anime')",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }

    let simulated = simulate(&app, SimulationOptions::default()).await;

    for (media_id, road) in [
        ("m-1", ("kids", Some("/movies/kids"), "move")),
        ("m-2", ("anime", Some("/movies/anime"), "move")),
        ("m-3", ("anime", Some("/movies/anime"), "none")),
    ] {
        let decision =
            simulated.decisions.iter().find(|d| d.media_id == media_id).expect("a decision");
        let proposed = (
            decision.target_category.as_str(),
            decision.target_root_folder.as_deref(),
            decision.action.as_str(),
        );
        assert_eq!(proposed, road, "{media_id} does not take the road the fixture names");

        let explained = app.get(&format!("/api/v1/media/{media_id}/explain")).await;
        let panel = explained.assert_ok();
        let shown = (
            panel["target_category"].as_str().unwrap_or_default(),
            panel["target_root_folder"].as_str(),
            panel["action"].as_str().unwrap_or_default(),
        );
        assert_eq!(shown, proposed, "the panel and the simulation disagree on {media_id}");
    }
}

/// The plan weighs what it sends to each folder: bytes crossing from another
/// filesystem count against the free space, bytes from a folder reporting the
/// same free space are a rename and do not, and a folder that cannot take
/// what it is sent comes first.
#[tokio::test]
async fn the_plan_weighs_what_each_folder_receives() {
    let app = TestApp::new().await;
    app.execute(&[
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
        "INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime'), ('cat-kids', 'kids')",
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category, free_space)
         VALUES ('rf-s', 'inst-1', 1, '/movies/standard', 1, 'standard', 5000),
                ('rf-a', 'inst-1', 2, '/movies/anime', 1, 'anime', 2000),
                ('rf-k', 'inst-1', 3, '/movies/kids', 1, 'kids', 5000)",
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category,
                            match_mode)
         VALUES ('r-a', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]', 'anime', 'all'),
                ('r-k', 'Kids', 20, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Family\"]}]', 'kids', 'all')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
                            size_on_disk, genres, monitored, has_files)
         VALUES ('m-1', 'inst-1', 1, 'movie', 'One', '/movies/standard', 1500, '[\"Animation\"]', 1, 1),
                ('m-2', 'inst-1', 2, 'movie', 'Two', '/movies/standard', 1000, '[\"Animation\"]', 1, 1),
                ('m-3', 'inst-1', 3, 'movie', 'Three', '/movies/standard', 700, '[\"Family\"]', 1, 1)",
    ])
    .await;

    let plan = simulate(&app, SimulationOptions::default()).await;

    let weighed: Vec<(String, i64, i64, i64, usize, bool)> = plan
        .capacity
        .iter()
        .map(|c| {
            let (path, incoming, same, free) =
                (c.path.clone(), c.incoming_bytes, c.same_filesystem_bytes, c.free_bytes);
            (path, incoming, same, free, c.items, c.fits)
        })
        .collect();
    assert_eq!(
        weighed,
        [
            ("/movies/anime".into(), 2500, 0, 2000, 2, false),
            ("/movies/kids".into(), 0, 700, 5000, 1, true),
        ]
    );
}

/// Two destinations reporting the same free space are one volume, and the
/// forecast weighs what they receive together, as the apply's guard does.
#[tokio::test]
async fn two_destinations_on_one_volume_are_forecast_together() {
    let app = TestApp::new().await;
    app.execute(&[
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
        "INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime'), ('cat-kids', 'kids')",
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category, free_space)
         VALUES ('rf-s', 'inst-1', 1, '/movies/standard', 1, 'standard', 9000),
                ('rf-a', 'inst-1', 2, '/movies/anime', 1, 'anime', 2000),
                ('rf-k', 'inst-1', 3, '/movies/kids', 1, 'kids', 2000)",
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, target_category,
                            match_mode)
         VALUES ('r-a', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]', 'anime', 'all'),
                ('r-k', 'Kids', 20, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Family\"]}]', 'kids', 'all')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
                            size_on_disk, genres, monitored, has_files)
         VALUES ('m-1', 'inst-1', 1, 'movie', 'One', '/movies/standard', 1500, '[\"Animation\"]', 1, 1),
                ('m-2', 'inst-1', 2, 'movie', 'Two', '/movies/standard', 1000, '[\"Family\"]', 1, 1)",
    ])
    .await;

    let plan = simulate(&app, SimulationOptions::default()).await;

    let fits: Vec<(String, bool)> =
        plan.capacity.iter().map(|c| (c.path.clone(), c.fits)).collect();
    assert_eq!(fits, [("/movies/anime".into(), false), ("/movies/kids".into(), false)]);
}

/// Until the first run after an upgrade, the library shows each title what it
/// showed before: its latest standing decision.
#[tokio::test]
async fn an_upgrade_shows_each_title_its_latest_standing_decision() {
    let pool = crate::tests::database_through("019_instance_reads").await;
    for statement in [
        crate::tests::AN_INSTANCE,
        "INSERT INTO media (id, instance_id, arr_id, media_type, title)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro'),
                ('m-2', 'inst-1', 11, 'movie', 'Heat')",
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                target_category, matched_rule_id, action, status, decided_at,
                                superseded)
         VALUES ('d-1', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'r-1', 'move', 'applied',
                 '2026-09-01 10:00:00', 0),
                ('d-2', 'm-1', 'Totoro', 'movie', 'inst-1', 'kids', 'r-2', 'move', 'pending',
                 '2026-09-02 10:00:00', 0),
                ('d-3', 'm-2', 'Heat', 'movie', 'inst-1', 'anime', 'r-1', 'move', 'pending',
                 '2026-09-02 10:00:00', 1)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    let kept: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT media_id, category, matched_rule_id FROM media_routing")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kept, [("m-1".to_string(), "kids".to_string(), Some("r-2".to_string()))]);
}

/// "Added within the last 0 days" meant the last 24 hours, and 0 is refused
/// now: an upgrade turns it into 1, which means the same, wherever it stands.
#[tokio::test]
async fn an_upgrade_turns_a_day_count_of_zero_into_one() {
    let pool = crate::tests::database_through("020_media_routing").await;
    sqlx::query(
        r#"INSERT INTO rules (id, name, priority, media_type, conditions, exclusions,
                              target_category)
           VALUES ('r-1', 'New', 10, 'both',
                   '[{"type":"genre_contains","value":["Drama"]},{"type":"added_within_days","value":0}]',
                   '[{"type":"added_within_days","value":0}]', 'standard'),
                  ('r-2', 'Recent', 20, 'both', '[{"type":"added_within_days","value":30}]',
                   '[]', 'standard')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    crate::db::run_migrations(&pool).await.unwrap();

    let rules = crate::services::routing::load_rules(&pool).await.unwrap();
    let days = |id: &str| -> Vec<crate::models::Condition> {
        let rule = rules.iter().find(|rule| rule.id == id).unwrap();
        rule.conditions.iter().chain(&rule.exclusions).cloned().collect()
    };
    assert_eq!(
        days("r-1"),
        [
            crate::models::Condition::GenreContains(vec!["Drama".into()]),
            crate::models::Condition::AddedWithinDays(1),
            crate::models::Condition::AddedWithinDays(1),
        ]
    );
    assert_eq!(days("r-2"), [crate::models::Condition::AddedWithinDays(30)]);
}

/// An upgrade gives every title's added date the shape every stored timestamp
/// has, as the Arr wrote it in its own.
#[tokio::test]
async fn an_upgrade_gives_each_added_date_the_stored_shape() {
    let pool = crate::tests::database_through("024_opened_by_schema").await;
    for statement in [
        crate::tests::AN_INSTANCE,
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, added_at)
         VALUES ('m-1', 'inst-1', 10, 'movie', 'Totoro', '2026-08-19T08:30:00Z'),
                ('m-2', 'inst-1', 11, 'movie', 'Heat', '2026-08-19 08:30:00'),
                ('m-3', 'inst-1', 12, 'movie', 'Akira', 'not a date')",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }

    crate::db::run_migrations(&pool).await.unwrap();

    let added: Vec<String> = sqlx::query_scalar("SELECT added_at FROM media ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(added, ["2026-08-19 08:30:00", "2026-08-19 08:30:00", "not a date"]);
}
