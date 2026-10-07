//! `GET /route`: where a title another service names would go, whether the
//! library holds it or only an Arr knows it, with nothing stored.

use axum::http::StatusCode;
use serde_json::Value;

use super::TestApp;
use super::fake_arr::FakeArr;
use super::fake_sources::FakeSources;
use super::fake_tmdb::FakeTmdb;
use crate::jobs::Attribution;

/// A Radarr synced into the library, Totoro in `/movies/standard`, the anime
/// rule, and `anime` and `kids` mapped onto the fake's folders.
async fn radarr_library(app: &TestApp, arr: &FakeArr) {
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;
    crate::services::sync::sync_instance(&app.state, "inst-1", &Attribution::manual(None))
        .await
        .unwrap();
    app.seed_anime_rule().await;
    for statement in [
        "INSERT INTO categories (id, name) VALUES ('cat-anime', 'anime'), ('cat-kids', 'kids')",
        "UPDATE root_folders SET category = 'anime' WHERE path = '/movies/anime'",
        "UPDATE root_folders SET category = 'kids' WHERE path = '/movies/kids'",
        "UPDATE root_folders SET category = 'standard' WHERE path = '/movies/standard'",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }
}

async fn count(app: &TestApp, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

fn only(answer: &Value) -> &Value {
    let answers = answer["answers"].as_array().expect("answers");
    assert_eq!(answers.len(), 1, "{answer}");
    &answers[0]
}

#[tokio::test]
async fn a_title_the_library_holds_goes_where_the_simulation_sends_it() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    // Tried after the winner, so what it reads cannot change the answer.
    app.seed_rule_on(serde_json::json!({ "type": "origin_country", "value": ["JP"] })).await;
    sqlx::query("UPDATE rules SET priority = 50 WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let placed = app.get("/api/v1/route?type=movie&tmdb=8392").await;
    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "library");
    assert_eq!(answer["category"], "anime");
    assert_eq!(answer["root_folder"]["path"], "/movies/anime");
    assert_eq!(answer["root_folder"]["origin"], "arr");
    assert_eq!(answer["current_root_folder"], "/movies/standard");
    assert_eq!(answer["action"], "move");
    assert_eq!(answer["rule"], "Anime");
    assert_eq!(answer["unanswered_fields"], serde_json::json!([]));

    // The explanation and the simulation give the same answer for the title.
    let media_id = answer["media_id"].as_str().unwrap();
    let explained = app.get(&format!("/api/v1/media/{media_id}/explain")).await;
    assert_eq!(explained.assert_ok()["target_category"], answer["category"]);
}

/// Each answer a request bot acts on, over the one fixture: a title already
/// where it belongs is left alone, one whose category has no folder is
/// skipped, a declared folder says so, a pin settles the title whatever the
/// rules read, and a field only an exclusion reads is still named unanswered,
/// since answered, it could send the title elsewhere.
#[tokio::test]
async fn each_answer_a_caller_acts_on_comes_back_as_such() {
    struct Case {
        name: &'static str,
        setup: &'static [&'static str],
        action: &'static str,
        category: &'static str,
        folder: Value,
        pinned: bool,
        unanswered: Value,
    }
    let arr_folder = serde_json::json!({ "path": "/movies/anime", "origin": "arr" });
    let cases = [
        Case {
            name: "already there",
            setup: &["UPDATE media SET current_root_folder = '/movies/anime'"],
            action: "none",
            category: "anime",
            folder: arr_folder.clone(),
            pinned: false,
            unanswered: serde_json::json!([]),
        },
        Case {
            name: "no folder for its category",
            setup: &["UPDATE root_folders SET category = NULL WHERE path = '/movies/anime'"],
            action: "skip",
            category: "anime",
            folder: Value::Null,
            pinned: false,
            unanswered: serde_json::json!([]),
        },
        Case {
            name: "a declared folder",
            setup: &[
                "UPDATE root_folders SET category = NULL WHERE path = '/movies/anime'",
                "INSERT INTO root_folders (id, instance_id, path, accessible, origin, category)
                 VALUES ('rf-d', 'inst-1', '/movies/anime/films', 1, 'declared', 'anime')",
            ],
            action: "move",
            category: "anime",
            folder: serde_json::json!({ "path": "/movies/anime/films", "origin": "declared" }),
            pinned: false,
            unanswered: serde_json::json!([]),
        },
        Case {
            name: "a pin",
            setup: &[
                "INSERT INTO overrides (id, media_id, target_category)
                 SELECT 'o-1', id, 'kids' FROM media",
                "UPDATE rules SET exclusions = '[{\"type\":\"keyword_contains\",\"value\":[\"live action\"]}]'",
            ],
            action: "move",
            category: "kids",
            folder: serde_json::json!({ "path": "/movies/kids", "origin": "arr" }),
            pinned: true,
            unanswered: serde_json::json!([]),
        },
        Case {
            name: "an exclusion on a field no source answers",
            setup: &[
                "UPDATE rules SET exclusions = '[{\"type\":\"keyword_contains\",\"value\":[\"live action\"]}]'",
            ],
            action: "move",
            category: "anime",
            folder: arr_folder.clone(),
            pinned: false,
            unanswered: serde_json::json!(["keywords"]),
        },
    ];

    for case in cases {
        let arr = FakeArr::start().await;
        let app = TestApp::new().await;
        radarr_library(&app, &arr).await;
        app.execute(case.setup).await;

        let placed = app.get("/api/v1/route?type=movie&tmdb=8392").await;
        let answer = only(placed.assert_ok());

        assert_eq!(answer["action"], case.action, "{}: {answer}", case.name);
        assert_eq!(answer["category"], case.category, "{}", case.name);
        assert_eq!(answer["root_folder"], case.folder, "{}", case.name);
        assert_eq!(answer["is_override"], case.pinned, "{}", case.name);
        assert_eq!(answer["unanswered_fields"], case.unanswered, "{}", case.name);
    }
}

#[tokio::test]
async fn a_title_only_the_arr_knows_is_looked_up_and_placed_without_being_stored() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    let (media, decisions) = (count(&app, "media").await, count(&app, "decisions").await);

    for query in ["tmdb=129", "imdb=tt0245429"] {
        let placed = app.get(&format!("/api/v1/route?type=movie&{query}")).await;
        let answer = only(placed.assert_ok());
        assert_eq!(answer["source"], "lookup", "{query}");
        assert_eq!(answer["media_id"], Value::Null);
        assert_eq!(answer["title"], "Spirited Away");
        assert_eq!(answer["category"], "anime");
        assert_eq!(answer["root_folder"]["path"], "/movies/anime");
        assert_eq!(answer["action"], "add", "a title the Arr does not hold is added, not moved");
    }
    assert_eq!(count(&app, "media").await, media, "a looked-up title was stored");
    assert_eq!(count(&app, "decisions").await, decisions, "a placement was stored");
    assert!(arr.recorded().reads.iter().any(|read| read == "/api/v3/movie/lookup/129"));
}

/// A title nobody knows is not found, and one no instance of its kind can
/// hold is not found either.
#[tokio::test]
async fn a_title_no_instance_knows_is_not_found() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;

    let missing = app.get("/api/v1/route?type=movie&tmdb=999999").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let no_sonarr = app.get("/api/v1/route?type=series&tvdb=76885").await;
    assert_eq!(no_sonarr.status, StatusCode::NOT_FOUND);
    let refused = app.get("/api/v1/route?type=movie").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

/// One Arr that cannot be asked is said, and the other still answers.
#[tokio::test]
async fn an_instance_that_fails_is_reported_and_stops_none_of_the_others() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-down', 'Radarr 4K', 'radarr', 'http://127.0.0.1:1', ?, 1, 'tok-down')",
    )
    .bind(app.state.secrets.seal("arr-key").unwrap())
    .execute(&app.state.pool)
    .await
    .unwrap();

    let placed = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let body = placed.assert_ok();
    assert_eq!(only(body)["instance_id"], "inst-1");
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{body}");
    assert_eq!(errors[0]["instance_id"], "inst-down");

    let narrowed = app.get("/api/v1/route?type=movie&tmdb=129&instance=inst-1").await;
    assert!(narrowed.assert_ok()["errors"].as_array().unwrap().is_empty());
}

/// The tags a requester would add the title with reach a rule reading them.
#[tokio::test]
async fn the_tags_a_new_title_would_carry_reach_the_rules() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "tag_in", "value": ["4k"] })).await;
    sqlx::query("UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let plain = app.get("/api/v1/route?type=movie&tmdb=129").await;
    assert_eq!(only(plain.assert_ok())["category"], "anime");
    let tagged = app.get("/api/v1/route?type=movie&tmdb=129&tags=hdr,%204k").await;
    assert_eq!(only(tagged.assert_ok())["category"], "kids");
}

/// A rule tried before the winner reads a field nobody answered, so the
/// placement could change once a source does. `enrich` asks the source now,
/// and stores nothing it hears.
#[tokio::test]
async fn enrich_asks_a_source_for_what_the_rules_read_and_stores_nothing() {
    let arr = FakeArr::start().await;
    let tmdb = FakeTmdb::start().await;
    let app = TestApp::new().await;
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    let app = TestApp::around(app.state.clone().with_config(config));
    crate::services::maintenance::converge_metadata_sources(&app.state).await.unwrap();
    radarr_library(&app, &arr).await;
    // Tried before the anime rule, and only TMDB answers keywords.
    app.seed_rule_on(serde_json::json!({ "type": "keyword_contains", "value": ["anime"] })).await;
    sqlx::query("UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let cached = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let answer = only(cached.assert_ok());
    assert_eq!(answer["category"], "anime");
    assert_eq!(answer["unanswered_fields"], serde_json::json!(["keywords"]));

    let enriched = app.get("/api/v1/route?type=movie&tmdb=129&enrich=true").await;
    let answer = only(enriched.assert_ok());
    assert_eq!(answer["category"], "kids", "{answer}");
    assert_eq!(answer["unanswered_fields"], serde_json::json!([]));
    assert_eq!(count(&app, "metadata_cache").await, 0, "an enriched placement stored an answer");
}

/// A title two instances know is one title to a source: `enrich` asks each
/// source once, at its pace and on its quota, however many instances answer.
#[tokio::test]
async fn enrich_asks_each_source_once_however_many_instances_know_the_title() {
    let arr = FakeArr::start().await;
    let tmdb = FakeTmdb::start().await;
    let app = TestApp::new().await;
    let mut config = crate::config::Config::for_tests();
    config.tmdb_api_key = Some("tmdb-key".into());
    config.tmdb_base_url = format!("{}/3", tmdb.base_url);
    let app = TestApp::around(app.state.clone().with_config(config));
    crate::services::maintenance::converge_metadata_sources(&app.state).await.unwrap();
    radarr_library(&app, &arr).await;
    app.seed_instance_at("inst-2", "radarr", &arr.base_url).await;

    let placed = app.get("/api/v1/route?type=movie&tmdb=129&enrich=true").await;

    assert_eq!(placed.assert_ok()["answers"].as_array().unwrap().len(), 2, "{}", placed.json);
    let asked = tmdb.recorded().paths.iter().filter(|path| path.starts_with("/movie/129")).count();
    assert_eq!(asked, 1, "{:?}", tmdb.recorded().paths);
}

/// The Radarr library, AniList as its one source, and a rule sending what
/// AniList calls Japanese to `kids` before the anime rule.
async fn library_on_anilist(arr: &FakeArr, sources: &FakeSources) -> TestApp {
    let app = TestApp::new().await;
    let mut config = crate::config::Config::for_tests();
    config.anilist_base_url = sources.anilist_url();
    let app = TestApp::around(app.state.clone().with_config(config));
    radarr_library(&app, arr).await;
    app.store_setting("metadata_providers", "anilist").await;
    crate::services::maintenance::converge_metadata_sources(&app.state).await.unwrap();
    app.seed_rule_on(serde_json::json!({ "type": "origin_country", "value": ["JP"] })).await;
    app.execute(&["UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'"])
        .await;
    app
}

/// A source found by title, as AniList is, answers under the id its search
/// resolved. `enrich` reads that answer under that id, and stores neither.
#[tokio::test]
async fn enrich_reads_what_a_source_found_by_title_answers_and_stores_nothing() {
    let (arr, sources) = (FakeArr::start().await, FakeSources::start().await);
    let app = library_on_anilist(&arr, &sources).await;

    let enriched = app.get("/api/v1/route?type=movie&tmdb=8392&enrich=true").await;

    assert_eq!(only(enriched.assert_ok())["category"], "kids", "AniList's answer went unread");
    assert_eq!(count(&app, "metadata_cache").await, 0, "an enriched placement stored an answer");
    assert_eq!(count(&app, "source_identifiers").await, 0, "it stored what the search found");
}

/// A search on record that found nothing is not run again for a placement:
/// the enrichment pass searches again once the miss is old enough.
#[tokio::test]
async fn enrich_does_not_search_again_for_a_title_a_search_missed() {
    let (arr, sources) = (FakeArr::start().await, FakeSources::start().await);
    let app = library_on_anilist(&arr, &sources).await;
    app.execute(&["INSERT INTO source_identifiers (source, media_type, local_key, external_id)
                   VALUES ('anilist', 'movie', 'tmdb:8392', NULL)"])
        .await;

    app.get("/api/v1/route?type=movie&tmdb=8392&enrich=true").await.assert_ok();

    assert!(sources.recorded().searches.is_empty(), "{:?}", sources.recorded().searches);
}

#[tokio::test]
async fn a_series_is_placed_through_sonarr() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("sonarr", &arr).await;

    let placed = app.get("/api/v1/route?type=series&tvdb=76885").await;
    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "library");
    assert_eq!(answer["title"], "Cowboy Bebop");
    let unknown = app.get("/api/v1/route?type=series&tvdb=1").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    // Named by TMDB, a series is found in the library, and one the library
    // does not hold through Sonarr's lookup of a `tmdb:` term.
    let by_tmdb = app.get("/api/v1/route?type=series&tmdb=30991").await;
    let found = by_tmdb.assert_ok();
    assert_eq!(found["answers"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["errors"], serde_json::json!([]), "{found}");
    let not_held = app.get("/api/v1/route?type=series&tmdb=26209").await;
    let looked_up = only(not_held.assert_ok());
    assert_eq!(
        (looked_up["source"].as_str(), looked_up["title"].as_str()),
        (Some("lookup"), Some("Mushishi"))
    );
    let unknown = app.get("/api/v1/route?type=series&tmdb=1").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.json);
}

/// TheTVDB knows series alone: a movie named by a TheTVDB id is refused. And
/// an Arr writes a title it has no id for as 0, so an id of 0 names every such
/// title at once: refused before anything is asked.
#[tokio::test]
async fn a_movie_named_by_a_tvdb_id_or_a_title_by_id_zero_is_refused() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;

    for path in [
        "/api/v1/route?type=movie&tvdb=76885",
        "/api/v1/route?type=movie&tmdb=0",
        "/api/v1/route?type=series&tvdb=-1",
    ] {
        let refused = app.get(path).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{path}: {:?}", refused.json);
    }
    assert!(arr.recorded().reads.iter().all(|read| !read.contains("lookup")), "Radarr was asked");
}

/// A film Radarr does not hold has no file, though its lookup does not say so.
#[tokio::test]
async fn a_film_radarr_does_not_hold_is_judged_as_having_no_file() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "has_files", "value": false })).await;
    app.execute(&["UPDATE rules SET priority = 1, target_category = 'kids' WHERE id = 'r-1'"])
        .await;

    let placed = app.get("/api/v1/route?type=movie&tmdb=129").await;

    assert_eq!(only(placed.assert_ok())["category"], "kids", "read as a film with a file");
}

/// The Sonarr library, with `condition` sending a series to `anime`.
async fn sonarr_library_routing_on(arr: &FakeArr, condition: Value) -> TestApp {
    let app = TestApp::synced_from("sonarr", arr).await;
    app.seed_rule_on(condition).await;
    app.execute(&["UPDATE root_folders SET category = 'anime' WHERE path = '/movies/anime'"]).await;
    app
}

/// A series Sonarr does not hold yet is judged on what adding it would set,
/// not on the defaults its lookup answers: the type the caller names.
#[tokio::test]
async fn a_series_sonarr_does_not_hold_is_judged_on_the_type_it_would_be_added_with() {
    let arr = FakeArr::start().await;
    let app = sonarr_library_routing_on(
        &arr,
        serde_json::json!({ "type": "series_type_is", "value": ["anime"] }),
    )
    .await;

    let as_looked_up = app.get("/api/v1/route?type=series&tvdb=81178").await;
    assert_ne!(only(as_looked_up.assert_ok())["category"], "anime");
    let as_added = app.get("/api/v1/route?type=series&tvdb=81178&series_type=anime").await;
    assert_eq!(
        only(as_added.assert_ok())["category"],
        "anime",
        "the type it is added with went unread"
    );
    let unknown = app.get("/api/v1/route?type=series&tvdb=81178&series_type=cartoon").await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{:?}", unknown.json);
}

/// And added today, which the lookup's date for a title nobody added is not.
#[tokio::test]
async fn a_title_the_arr_does_not_hold_is_judged_as_added_today() {
    let arr = FakeArr::start().await;
    let app = sonarr_library_routing_on(
        &arr,
        serde_json::json!({ "type": "added_within_days", "value": 7 }),
    )
    .await;

    let placed = app.get("/api/v1/route?type=series&tvdb=81178").await;
    assert_eq!(only(placed.assert_ok())["category"], "anime");
}

/// A title the Arr holds and the last sync did not read keeps its own tags,
/// which the tag catalogue names: the tags a caller sends are for a title
/// that has none yet.
#[tokio::test]
async fn a_title_the_arr_holds_keeps_its_own_tags_before_it_is_synced() {
    let arr = FakeArr::start().await;
    let app = sonarr_library_routing_on(
        &arr,
        serde_json::json!({ "type": "tag_in", "value": ["anime"] }),
    )
    .await;
    app.execute(&["DELETE FROM media"]).await;

    let placed = app.get("/api/v1/route?type=series&tvdb=76885&tags=kids").await;

    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "lookup");
    assert_eq!(answer["category"], "anime", "{answer}");
}

/// The same through Radarr, whose lookup builds the film afresh with no id:
/// that Radarr holds it is read from its library, and the film keeps its own
/// tags and folder.
#[tokio::test]
async fn a_film_radarr_holds_keeps_its_own_tags_before_it_is_synced() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.seed_rule_on(serde_json::json!({ "type": "tag_in", "value": ["anime"] })).await;
    app.execute(&[
        "UPDATE root_folders SET category = 'anime' WHERE path = '/movies/anime'",
        "DELETE FROM media",
    ])
    .await;

    let placed = app.get("/api/v1/route?type=movie&tmdb=8392&tags=kids").await;

    let answer = only(placed.assert_ok());
    assert_eq!(answer["source"], "lookup");
    assert_eq!(answer["current_root_folder"], "/movies/standard", "{answer}");
    assert_eq!(answer["category"], "anime", "{answer}");
}

/// An instance whose key cannot be opened fails for a reason that belongs to
/// the operator's log, not to whoever asked: the decryption error names the
/// variable that holds the master key. `/route` and the task list say that
/// something failed inside, and the log keeps the detail.
#[tokio::test]
async fn an_internal_failure_is_told_to_a_caller_in_one_generic_sentence() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    let foreign = crate::crypto::SecretBox::load(
        Some("a-master-key-this-installation-never-had"),
        None,
        std::path::Path::new("/nonexistent"),
        None,
    )
    .unwrap()
    .seal("arr-key")
    .unwrap();
    sqlx::query("UPDATE instances SET api_key = ?")
        .bind(&foreign)
        .execute(&app.state.pool)
        .await
        .unwrap();

    let placed = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let told = placed.json["errors"][0]["error"].as_str().unwrap_or_default().to_string();
    assert!(!told.is_empty(), "{:?}", placed.json);
    assert!(!told.to_lowercase().contains("decrypt"), "the caller read {told}");

    app.post("/api/v1/instances/inst-1/sync", serde_json::json!({})).await;
    let jobs = app.get("/api/v1/jobs?status=failed").await;
    let failed = &jobs.assert_ok()["data"][0];
    assert_eq!(failed["kind"], "sync", "{:?}", jobs.json);
    let told = failed["error_message"].as_str().unwrap_or_default();
    assert!(
        !told.is_empty() && !told.to_lowercase().contains("decrypt"),
        "the task list read {told}"
    );

    // A probe records what it found, and `/status`, which any key reads,
    // repeats it.
    let probed = app.get("/api/v1/health").await;
    let status =
        probed.assert_ok()["instances"][0]["status"].as_str().unwrap_or_default().to_string();
    assert!(status.starts_with("error"), "{:?}", probed.json);
    assert!(!status.to_lowercase().contains("decrypt"), "the probe told {status}");
    let warnings = app.get("/api/v1/status").await.assert_ok()["warnings"].to_string();
    assert!(!warnings.to_lowercase().contains("decrypt"), "/status told {warnings}");
}

/// A key looping on ids no library holds would send each to the Arr, which
/// sends it to its own metadata service, on the owner's quota. A title the Arr
/// did not know is believed unknown for ten minutes.
#[tokio::test]
async fn a_title_no_arr_knows_is_asked_about_once_within_ten_minutes() {
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    radarr_library(&app, &arr).await;
    let asked = || arr.recorded().reads.iter().filter(|p| p.contains("lookup")).count();

    for _ in 0..3 {
        let missing = app.get("/api/v1/route?type=movie&tmdb=999999").await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(asked(), 1, "the Arr was asked again about a title it does not know");

    app.get("/api/v1/route?type=movie&tmdb=999998").await;
    assert_eq!(asked(), 2, "another title went unasked");
}
