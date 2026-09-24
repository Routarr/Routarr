//! Which rules are unreachable, and the capacity a plan needs.
//!
//! Both answer questions nothing else does: validation looks inside one rule,
//! and the guardrails weigh a move without ever weighing the plan.

use super::TestApp;

async fn add_rule(app: &TestApp, id: &str, name: &str, priority: i64, value: &str, category: &str) {
    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES (?, ?, ?, 1, 'both', ?, ?, 'all')",
    )
    .bind(id)
    .bind(name)
    .bind(priority)
    .bind(format!(r#"[{{"type":"title_contains","value":["{value}"]}}]"#))
    .bind(category)
    .execute(&app.state.pool)
    .await
    .unwrap();
}

fn rule<'a>(body: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    body["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|r| r["rule_name"] == name)
        .unwrap_or_else(|| panic!("{name} missing from the report: {body}"))
}

/// The trap this exists for: a rule below a broader one can never fire, and
/// the preview reports that as "0 changes" — the same thing it reports for a
/// rule that correctly changes nothing.
#[tokio::test]
async fn a_rule_that_never_wins_names_the_rule_taking_its_items() {
    let app = TestApp::new().await;
    app.seed_library().await;
    // Both match the same film; the lower priority number wins.
    add_rule(&app, "r-wide", "Everything", 10, "totoro", "standard").await;
    add_rule(&app, "r-narrow", "Anime", 50, "totoro", "anime").await;

    let response = app.get("/api/v1/rules/health").await;
    let body = response.assert_ok();

    let smothered = rule(body, "Anime");
    assert_eq!(smothered["won"], 0, "the shadowed rule should decide nothing");
    assert!(smothered["shadowed"].as_u64().unwrap() > 0);
    assert_eq!(
        smothered["shadowed_by"], "Everything",
        "a bare zero is not actionable; the report has to name the culprit: {smothered}"
    );

    let winner = rule(body, "Everything");
    assert!(winner["won"].as_u64().unwrap() > 0);
    assert!(winner["shadowed_by"].is_null(), "the rule that wins is nobody's victim");
}

/// A rule that decides some items and loses others is doing its job. Naming a
/// culprit there would turn a working rule set into a page of warnings.
#[tokio::test]
async fn a_rule_that_wins_sometimes_is_not_reported_as_shadowed() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-1", "Totoro", 10, "totoro", "anime").await;

    let response = app.get("/api/v1/rules/health").await;
    let winner = rule(response.assert_ok(), "Totoro");
    assert!(winner["won"].as_u64().unwrap() > 0);
    assert!(winner["shadowed_by"].is_null());
}

/// Matching nothing is a different fault from being shadowed — a condition too
/// narrow rather than a priority too low — and the two want different fixes.
#[tokio::test]
async fn a_rule_matching_nothing_is_reported_apart_from_a_shadowed_one() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-none", "Nothing at all", 10, "zzzzz-no-such-title", "anime").await;

    let response = app.get("/api/v1/rules/health").await;
    let orphan = rule(response.assert_ok(), "Nothing at all");
    assert_eq!(orphan["matched_nothing"], true);
    assert_eq!(orphan["shadowed"], 0);
    assert!(orphan["shadowed_by"].is_null());
}

// --------------------------------------------------------------- capacity

/// Seed one pending move of `size` bytes from `/movies/standard` to
/// `/movies/anime`, with the two folders reporting the free space given.
async fn pending_move(app: &TestApp, size: i64, source_free: i64, target_free: i64) -> String {
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('i1', 'Radarr', 'radarr', 'http://arr', 'k', 1, 't')",
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
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    "d1".to_string()
}

/// `confirmed` names the guardrail the caller looked at, so a test that answers
/// the capacity question does not also answer the batch threshold.
async fn apply(app: &TestApp, confirmed: &[&str]) -> super::TestResponse {
    app.post(
        "/api/v1/decisions/apply",
        serde_json::json!({ "decision_ids": ["d1"], "move_files": true, "confirm": confirmed }),
    )
    .await
}

/// `free_space` is synced on every pass and `size_on_disk` sits on every row.
/// Uncompared, a batch that overruns its destination fails partway at the Arr.
#[tokio::test]
async fn a_move_larger_than_the_destination_is_refused_until_confirmed() {
    let app = TestApp::new().await;
    // 100 GB moving onto a volume with 10 GB free, from a different volume.
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;

    let refused = apply(&app, &[]).await;
    assert_eq!(refused.status, 409, "a plan that cannot fit was applied: {}", refused.json);
    let message = refused.message();
    assert!(message.contains("/movies/anime"), "the refusal must name the folder: {message}");
    // Both figures, in the interface's own vocabulary. `GiB` was pinned here
    // while the root-folders table said `GB` off the same division by 1024 —
    // one figure named two ways, on two screens read together.
    assert!(message.contains("93.1 GB"), "the refusal must say what is moving: {message}");
    assert!(message.contains("9.3 GB"), "and what the destination has: {message}");
}

/// A destination the Arr cannot reach is asked about, not silently written to.
///
/// The routing map keeps a sleeping folder on purpose, so the question of
/// whether it can be written to has to be asked here — and asked rather than
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

    // Answered, it gets out of the way — and answers only itself.
    let allowed = apply(&app, &["unreachable"]).await;
    assert_ne!(allowed.status, 409, "confirming did not get past the guard: {}", allowed.json);
}

/// Answering one question must not answer the others.
///
/// Three guardrails ask through the same mechanism, and they used to read a
/// single boolean: confirming a capacity shortfall lifted the batch threshold
/// as well, silently, and the operator was never shown the second fact. Each
/// asks under its own name now, and lifts only that name.
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

    // The capacity question is answered; the threshold has not been asked yet.
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

/// Evidence, not proof — so being wrong costs a click and never a block.
#[tokio::test]
async fn the_refusal_can_be_confirmed_through() {
    let app = TestApp::new().await;
    pending_move(&app, 100_000_000_000, 500_000_000_000, 10_000_000_000).await;

    assert_eq!(apply(&app, &[]).await.status, 409);
    // Past the guard the move reaches the Arr, which is unreachable here: the
    // apply answers, and reports the one move as failed. Anything else — a
    // refusal, or a success against a host that does not exist — is not the
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

// ----------------------------------------------------------------- collisions

/// Two rules with one name and one priority leave the winner to whichever id
/// SQLite returns first — the engine breaks ties on priority, then name, then
/// id, and that last step exists precisely because this happens.
#[tokio::test]
async fn two_rules_sharing_a_name_and_a_priority_are_reported() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-a", "Anime", 10, "totoro", "anime").await;
    add_rule(&app, "r-b", "Anime", 10, "akira", "kids").await;

    let response = app.get("/api/v1/rules/health").await;
    let entries = response.assert_ok()["rules"].as_array().unwrap().clone();
    assert!(
        entries.iter().all(|r| r["ambiguous_with"] == "Anime"),
        "both sides of the ambiguity have to say so: {entries:?}"
    );
}

/// Identical conditions and target: one of the two decides nothing whatever
/// the priorities say.
#[tokio::test]
async fn a_rule_duplicated_in_every_respect_is_reported() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-1", "First", 10, "totoro", "anime").await;
    add_rule(&app, "r-2", "Second", 20, "totoro", "anime").await;

    let response = app.get("/api/v1/rules/health").await;
    let body = response.assert_ok();
    assert_eq!(rule(body, "First")["duplicate_of"], "Second");
    assert_eq!(rule(body, "Second")["duplicate_of"], "First");
}

/// And rules that merely resemble each other are left alone, or the report
/// becomes a page of warnings about a working rule set.
#[tokio::test]
async fn rules_differing_in_target_or_conditions_are_not_called_duplicates() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-1", "First", 10, "totoro", "anime").await;
    add_rule(&app, "r-2", "Second", 20, "totoro", "kids").await;
    add_rule(&app, "r-3", "Third", 30, "akira", "anime").await;

    let body = app.get("/api/v1/rules/health").await;
    let body = body.assert_ok();
    for name in ["First", "Second", "Third"] {
        assert!(rule(body, name)["duplicate_of"].is_null(), "{name} was called a duplicate");
        assert!(rule(body, name)["ambiguous_with"].is_null());
    }
}

/// Two rules alike in every respect but the instances they apply to decide
/// different things; two alike in every respect but the order of their
/// conditions decide the same thing. The comparison has to see both.
#[tokio::test]
async fn twins_on_different_instances_are_not_duplicates_and_reordered_conditions_are() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let two = r#"[{"type":"title_contains","value":["totoro"]},{"type":"genre_contains","value":["Animation"]}]"#;
    let swapped = r#"[{"type":"genre_contains","value":["Animation"]},{"type":"title_contains","value":["totoro"]}]"#;
    for (id, name, priority, conditions, instances) in [
        ("r-1", "Here", 10, two, Some(r#"["inst-1"]"#)),
        ("r-2", "There", 20, two, Some(r#"["inst-2"]"#)),
        ("r-3", "Everywhere", 30, two, None),
        ("r-4", "Swapped", 40, swapped, None),
    ] {
        sqlx::query(
            "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
             target_category, match_mode, instance_ids)
             VALUES (?, ?, ?, 1, 'both', ?, 'anime', 'all', ?)",
        )
        .bind(id)
        .bind(name)
        .bind(priority)
        .bind(conditions)
        .bind(instances)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    let body = app.get("/api/v1/rules/health").await;
    let body = body.assert_ok();
    assert!(rule(body, "Here")["duplicate_of"].is_null(), "restricted to another instance");
    assert!(rule(body, "There")["duplicate_of"].is_null(), "restricted to another instance");
    assert_eq!(rule(body, "Everywhere")["duplicate_of"], "Swapped");
    assert_eq!(rule(body, "Swapped")["duplicate_of"], "Everywhere");
}

// -------------------------------------------------------------------- facets

/// `U` and `TV-PG` say nothing to most readers, and this panel exists to show
/// what the library holds.
///
/// The code stays the value — it is what a rule matches on — and the meaning is
/// added beside it. Only where the systems agree: `12` is twelve-and-over for
/// the BBFC, the FSK and the CNC alike, while `M` is fifteen-and-over in
/// Australia and something else in the United States, so `M` is left bare. A
/// wrong name on a right value is worse than no name.
#[tokio::test]
async fn a_certification_is_shown_with_what_it_means() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET certification = 'TV-PG' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let body = app.get("/api/v1/media/facets").await.assert_ok().clone();
    let named = body["certifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["value"] == "TV-PG")
        .expect("the fixture must carry it");
    assert_eq!(named["label"], "TV-PG (parental guidance)");

    // A code the systems disagree about carries no name at all.
    sqlx::query("UPDATE media SET certification = 'M' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let body = app.get("/api/v1/media/facets").await.assert_ok().clone();
    let bare = body["certifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["value"] == "M")
        .expect("the fixture must carry it");
    assert!(bare.get("label").is_none(), "a guessed name reached the screen: {bare}");
}

/// `/media/facets` is a literal segment sitting beside `/media/{id}`; if the
/// dynamic route swallowed it the endpoint would answer "media facets not
/// found" and look like a missing item rather than a routing mistake.
#[tokio::test]
async fn the_facets_route_is_not_swallowed_by_the_media_id_route() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();
    assert!(body["genres"].is_array(), "got {body}");
}

/// What the library holds, per axis a condition reads. Without it, a rule is
/// written by guessing what the library carries.
#[tokio::test]
async fn the_facets_count_what_the_library_actually_carries() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "UPDATE media SET genres = '[\"Animation\",\"Family\"]', original_language = 'ja',
         certification = 'PG'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();

    let values = |axis: &str| -> Vec<String> {
        body[axis]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["value"].as_str().unwrap().to_string())
            .collect()
    };
    let genres = values("genres");
    assert!(
        genres.iter().any(|g| g == "Animation") && genres.iter().any(|g| g == "Family"),
        "got {genres:?}"
    );
    assert!(values("original_languages").iter().any(|l| l == "ja"));
    // Both sources answer, and both answers are offered: `PG` is the Arr's,
    // `G` the cached TMDb one. A list holding only the first would not contain
    // every value the engine can match.
    let certifications = values("certifications");
    assert!(
        certifications.iter().any(|c| c == "PG") && certifications.iter().any(|c| c == "G"),
        "got {certifications:?}"
    );
    assert_eq!(body["without_metadata"], 0);
}

/// A language rule is written against an ISO code, and the library holds five
/// of them at most. Offering only those would hide the rest of the table — and
/// leave the code, which nobody guesses from a name, to be typed blind.
#[tokio::test]
async fn a_coded_axis_offers_its_whole_vocabulary_not_the_synced_part() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();

    let languages = body["vocabularies"]["original_languages"].as_array().unwrap();
    assert!(languages.len() > 40, "got {} entries", languages.len());

    // The value is the code the engine compares; the label is the only part a
    // reader recognises, and neither is derivable from the other.
    let korean = languages
        .iter()
        .find(|entry| entry["value"] == "ko")
        .unwrap_or_else(|| panic!("no Korean in {languages:?}"));
    assert_eq!(korean["label"], "Korean (ko)");
    assert_eq!(korean["count"], 0, "a vocabulary entry is not an observation");

    // Countries the same way, and both are offered whatever the library holds.
    let countries = body["vocabularies"]["origin_countries"].as_array().unwrap();
    assert!(countries.iter().any(|entry| entry["value"] == "KR"));
}

/// A genre is its own label: the library defines what it means, so there is
/// nothing to translate and no vocabulary to add to it.
#[tokio::test]
async fn an_open_axis_carries_no_vocabulary() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();

    assert!(body["vocabularies"].get("genres").is_none());
    let genres = body["genres"].as_array().unwrap();
    assert!(genres.iter().all(|facet| facet.get("label").is_none()));
}

/// A source the user switched off answers nothing, so it offers nothing either:
/// a value in the list that the engine cannot match is a rule that silently
/// never fires.
#[tokio::test]
async fn a_disabled_source_contributes_nothing_to_the_lists() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET certification = 'PG'").execute(&app.state.pool).await.unwrap();
    app.put("/api/v1/settings", serde_json::json!({ "settings": { "metadata_providers": "arr" } }))
        .await
        .assert_ok();

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();
    let certifications: Vec<&str> = body["certifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["value"].as_str().unwrap())
        .collect();
    assert_eq!(certifications, vec!["PG"], "TMDb is off, so its `G` must not be offered");
}

/// The count that explains a rule matching nothing for a reason no condition
/// can express: the items carry nothing to match on.
#[tokio::test]
async fn items_carrying_no_metadata_at_all_are_counted_apart() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET genres = '[]', original_language = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();
    // The cached TMDb answer has to go too: an item a source still describes is
    // not invisible to the conditions, and counting it as such would contradict
    // the lists this endpoint returns beside the figure.
    sqlx::query("DELETE FROM metadata_cache").execute(&app.state.pool).await.unwrap();

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();
    assert!(body["without_metadata"].as_i64().unwrap() > 0, "got {body}");
    assert!(body["genres"].as_array().unwrap().is_empty());
}

/// The other half: an item whose Arr row carries nothing is still described
/// when a source answers for it, so it is not counted as bare.
#[tokio::test]
async fn an_item_described_only_by_a_fetched_source_is_not_counted_as_bare() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET genres = '[]', original_language = NULL")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();
    assert_eq!(body["without_metadata"], 0, "TMDb still describes it: {body}");
    let genres: Vec<&str> =
        body["genres"].as_array().unwrap().iter().map(|f| f["value"].as_str().unwrap()).collect();
    assert!(genres.contains(&"Animation"), "got {genres:?}");
}
