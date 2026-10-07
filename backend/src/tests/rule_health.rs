//! Which rules are unreachable, the collisions between rules and what the
//! library holds per axis a condition reads.
//!
//! Each answers a question nothing else does: validation looks inside one
//! rule, and the guardrails weigh a move without ever weighing the plan.

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
/// the preview reports that as "0 changes", the same thing it reports for a
/// rule that correctly changes nothing.
#[tokio::test]
async fn a_rule_that_never_wins_names_the_rule_taking_its_items() {
    let app = TestApp::new().await;
    app.seed_library().await;
    // Both match the same film, and the lower priority number wins.
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
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, current_path,
         current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Totoro Returns', 2026,
                 '/movies/standard/Totoro Returns (2026)', '/movies/standard', 1, 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    add_rule(&app, "r-1", "Totoro", 10, "totoro", "anime").await;
    add_rule(&app, "r-2", "Sequels", 1, "returns", "standard").await;

    let response = app.get("/api/v1/rules/health").await;
    let totoro = rule(response.assert_ok(), "Totoro");
    assert_eq!(totoro["won"], 1, "{totoro}");
    assert_eq!(totoro["shadowed"], 1, "it has to lose one for the claim to mean anything");
    assert!(totoro["shadowed_by"].is_null(), "{totoro}");
}

/// Matching nothing is a different fault from being shadowed (a condition too
/// narrow rather than a priority too low), and the two want different fixes.
/// A switched-off rule is read by no run, which says nothing of its
/// conditions.
#[tokio::test]
async fn a_rule_matching_nothing_is_reported_apart_from_a_shadowed_one() {
    let app = TestApp::new().await;
    app.seed_library().await;
    add_rule(&app, "r-none", "Nothing at all", 10, "zzzzz-no-such-title", "anime").await;
    add_rule(&app, "r-off", "Switched off", 20, "zzzzz-no-such-title", "anime").await;
    app.execute(&["UPDATE rules SET enabled = 0 WHERE id = 'r-off'"]).await;

    let response = app.get("/api/v1/rules/health").await;
    let orphan = rule(response.assert_ok(), "Nothing at all");
    assert_eq!(orphan["matched_nothing"], true);
    assert_eq!(orphan["shadowed"], 0);
    assert!(orphan["shadowed_by"].is_null());
    assert_eq!(rule(response.assert_ok(), "Switched off")["matched_nothing"], false);
}

// ----------------------------------------------------------------- collisions

/// Two rules with one name and one priority leave the winner to their ids,
/// which are random: the engine breaks ties on priority, then name, then id,
/// and that last step exists precisely because this happens.
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
/// different things, and two alike in every respect but the order of their
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

/// Rules alike but for their exclusions, their match mode or their media type
/// decide different things, and are not called duplicates. A name shared at
/// two priorities is no ambiguity: the priority settles the order.
#[tokio::test]
async fn rules_differing_in_any_one_respect_are_not_duplicates() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let condition = r#"[{"type":"title_contains","value":["totoro"]}]"#;
    let veto = r#"[{"type":"genre_contains","value":["Horror"]}]"#;
    for (id, name, priority, exclusions, mode, media_type) in [
        ("r-1", "Plain", 10, "[]", "all", "both"),
        ("r-2", "Vetoing", 20, veto, "all", "both"),
        ("r-3", "Any", 30, "[]", "any", "both"),
        ("r-4", "Films", 40, "[]", "all", "movie"),
        ("r-5", "Plain", 50, veto, "any", "series"),
    ] {
        sqlx::query(
            "INSERT INTO rules (id, name, priority, enabled, media_type, conditions, exclusions,
             target_category, match_mode)
             VALUES (?, ?, ?, 1, ?, ?, ?, 'anime', ?)",
        )
        .bind(id)
        .bind(name)
        .bind(priority)
        .bind(media_type)
        .bind(condition)
        .bind(exclusions)
        .bind(mode)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    let body = app.get("/api/v1/rules/health").await;
    let entries = body.assert_ok()["rules"].as_array().unwrap().clone();
    for entry in &entries {
        assert!(entry["duplicate_of"].is_null(), "called a duplicate: {entry}");
        assert!(entry["ambiguous_with"].is_null(), "called ambiguous: {entry}");
    }
}

/// A rule its exclusion sets aside counts its vetoes, and is not reported as
/// one that matches nothing.
#[tokio::test]
async fn a_rule_its_exclusion_sets_aside_counts_the_vetoes() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[r#"INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
                     exclusions, target_category, match_mode)
                     VALUES ('r-1', 'Vetoed', 10, 1, 'both',
                             '[{"type":"title_contains","value":["totoro"]}]',
                             '[{"type":"title_contains","value":["neighbor"]}]', 'anime', 'all')"#])
        .await;

    let body = app.get("/api/v1/rules/health").await;
    let vetoed = rule(body.assert_ok(), "Vetoed");
    assert_eq!(vetoed["vetoed"], 1, "{vetoed}");
    assert_eq!(vetoed["matched_nothing"], false, "{vetoed}");
}

// -------------------------------------------------------------------- facets// -------------------------------------------------------------------- facets

/// `U` and `TV-PG` say nothing to most readers, and this panel exists to show
/// what the library holds.
///
/// The code stays the value, since it is what a rule matches on, and the
/// meaning is added beside it. Only where the systems agree: `12` is
/// twelve-and-over for the BBFC, the FSK and the CNC alike, while `M` is
/// fifteen-and-over in Australia and something else in the United States, so
/// `M` is left bare. A wrong name on a right value is worse than no name.
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

/// Codes of several countries mean the same thing, and a rule wants every one
/// of them: the list gathers the codes by what they mean, the youngest audience
/// first, and leaves the codes nobody can name at the end, ungrouped.
#[tokio::test]
async fn certifications_are_grouped_by_meaning_youngest_first() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("UPDATE media SET certification = 'U' WHERE id = 'm-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    // The ages out of order, so only their rank can put them in it.
    for (id, certification) in [
        ("m-2", "12"),
        ("m-3", "TP"),
        ("m-4", "TP"),
        ("m-5", "M"),
        ("m-6", "18"),
        ("m-7", "PG"),
        ("m-8", "NR"),
        ("m-9", "16"),
    ] {
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
             monitored, has_files, certification)
             VALUES (?, 'inst-1', ?, 'movie', ?, '/movies/standard', 1, 1, ?)",
        )
        .bind(id)
        .bind(id.trim_start_matches("m-").parse::<i64>().unwrap() + 100)
        .bind(format!("Title {id}"))
        .bind(certification)
        .execute(&app.state.pool)
        .await
        .unwrap();
    }

    let response = app.get("/api/v1/media/facets").await;
    let listed: Vec<(String, Option<String>)> = response.assert_ok()["certifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["value"].as_str().unwrap().to_string(), f["group"].as_str().map(String::from)))
        .collect();

    let localizer = app.state.localizer().await;
    let named = |key: &str| Some(localizer.translate(key, &[]));
    let from = |age: &str| Some(localizer.translate("CertFromAge", &[("age", age)]));
    let all_ages = named("CertAllAges");
    let groups: Vec<Option<String>> = listed.iter().map(|(_, group)| group.clone()).collect();
    // `U` from the Arr, `G` from TMDB for the same film, and `TP` twice. Then a
    // parent's guidance, each age upwards, the unrated, and what nobody can name.
    assert_eq!(
        groups,
        vec![
            all_ages.clone(),
            all_ages.clone(),
            all_ages,
            named("CertGuidance"),
            from("12"),
            from("16"),
            from("18"),
            named("CertNotRated"),
            None,
        ],
        "{listed:?}"
    );
    assert_eq!(listed[0].0, "TP", "the most frequent code leads its group: {listed:?}");
    assert_eq!(listed.last().unwrap().0, "M");
}

/// What the library holds, per axis a condition reads. Without it, a rule is
/// written by guessing what the library carries.
#[tokio::test]
async fn the_facets_count_what_the_library_actually_carries() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "UPDATE media SET genres = '[\"Animation\",\"Family\"]', original_language = 'ja',
         certification = 'PG', status = 'inCinemas'",
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
    // `G` the cached TMDB one. A list holding only the first would not contain
    // every value the engine can match.
    let certifications = values("certifications");
    assert!(
        certifications.iter().any(|c| c == "PG") && certifications.iter().any(|c| c == "G"),
        "got {certifications:?}"
    );
    assert_eq!(body["without_metadata"], 0);
    // The Arr's own status, as it writes it, named in words.
    assert_eq!(body["statuses"][0]["value"], "inCinemas");
    assert_eq!(body["statuses"][0]["label"], "In cinemas");
}

/// A language rule is written against an ISO code, and the library holds five
/// of them at most. Offering only those would hide the rest of the table, and
/// leave the code, which nobody guesses from a name, to be typed blind.
#[tokio::test]
async fn a_coded_axis_offers_its_whole_vocabulary_not_the_synced_part() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let response = app.get("/api/v1/media/facets").await;
    let body = response.assert_ok();

    let languages = body["vocabularies"]["original_languages"].as_array().unwrap();
    assert!(languages.len() > 40, "got {} entries", languages.len());

    // The value is the code the engine compares, and the label is the only part
    // a reader recognises. Neither is derivable from the other.
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
    assert_eq!(certifications, vec!["PG"], "TMDB is off, so its `G` must not be offered");
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
    // The cached TMDB answer has to go too: an item a source still describes is
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
    assert_eq!(body["without_metadata"], 0, "TMDB still describes it: {body}");
    let genres: Vec<&str> =
        body["genres"].as_array().unwrap().iter().map(|f| f["value"].as_str().unwrap()).collect();
    assert!(genres.contains(&"Animation"), "got {genres:?}");
}
