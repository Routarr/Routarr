//! What an automation relies on: finding a title by the id another service
//! gives it, pinning it that way, following long work without waiting for
//! it, and warnings a script can tell apart.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::fake_arr::FakeArr;
use super::{TestApp, TestResponse, finished, preferring_async};
use crate::api::jobs::prefers_async;

/// Totoro twice: on `inst-1` as the library seeds it, and on a second Radarr.
async fn two_copies() -> TestApp {
    let app = TestApp::new().await;
    app.seed_library().await;
    for statement in [
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-2', 'Radarr 4K', 'radarr', 'http://127.0.0.1:1', 'secret', 1, 'tok-2')",
        "UPDATE media SET imdb_id = 'tt0096283' WHERE id = 'm-1'",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id, imdb_id,
         current_path, current_root_folder, monitored, has_files)
         VALUES ('m-2', 'inst-2', 11, 'movie', 'My Neighbor Totoro', 1988, 8392, 'tt0096283',
                 '/movies-4k/My Neighbor Totoro (1988)', '/movies-4k', 1, 1)",
    ] {
        sqlx::query(statement).execute(&app.state.pool).await.unwrap();
    }
    app
}

async fn pins(app: &TestApp) -> Vec<(String, String)> {
    sqlx::query_as("SELECT media_id, target_category FROM overrides ORDER BY media_id")
        .fetch_all(&app.state.pool)
        .await
        .unwrap()
}

async fn put(app: &TestApp, path: &str, body: Value) -> TestResponse {
    app.put(path, body).await
}

#[tokio::test]
async fn the_library_is_found_by_the_id_another_service_gives_a_title() {
    let app = two_copies().await;

    let found = app.get("/api/v1/media?tmdb_id=8392").await;
    assert_eq!(found.assert_ok()["pagination"]["total"], 2);
    assert_eq!(found.json["data"][0]["imdb_id"], "tt0096283");
    let found = app.get("/api/v1/media?imdb_id=tt0096283&instance_id=inst-2").await;
    assert_eq!(found.assert_ok()["pagination"]["total"], 1);
    assert_eq!(found.json["data"][0]["id"], "m-2");

    for missing in ["tmdb_id=1", "tvdb_id=8392", "imdb_id=tt0000001"] {
        let found = app.get(&format!("/api/v1/media?{missing}")).await;
        assert_eq!(found.assert_ok()["pagination"]["total"], 0, "{missing}");
    }
}

#[tokio::test]
async fn a_pin_by_external_id_reaches_every_copy_and_an_instance_narrows_it() {
    let app = two_copies().await;
    let pin = json!({ "target_category": "anime", "reason": "a request bot" });

    let set = put(&app, "/api/v1/overrides/external?type=movie&tmdb=8392", pin.clone()).await;
    assert_eq!(set.assert_ok().as_array().unwrap().len(), 2, "{:?}", set.json);
    let both = vec![("m-1".into(), "anime".into()), ("m-2".into(), "anime".into())];
    assert_eq!(pins(&app).await, both);

    let path = "/api/v1/overrides/external?type=movie&imdb=tt0096283&instance=inst-2";
    let removed = app.delete(path).await;
    assert_eq!(removed.assert_ok()["deleted"], true);
    assert_eq!(pins(&app).await, vec![("m-1".into(), "anime".into())]);
    let again = app.delete(path).await;
    assert_eq!(again.assert_ok()["deleted"], false, "nothing left to remove is not a failure");

    let narrowed = json!({ "target_category": "standard" });
    let path = "/api/v1/overrides/external?type=movie&tmdb=8392&instance=inst-1";
    assert_eq!(put(&app, path, narrowed).await.assert_ok().as_array().unwrap().len(), 1);
    assert_eq!(pins(&app).await, vec![("m-1".into(), "standard".into())]);
}

#[tokio::test]
async fn a_title_named_twice_or_not_at_all_is_refused_and_one_nobody_holds_is_not_found() {
    let app = two_copies().await;
    let pin = json!({ "target_category": "anime" });

    for query in ["type=movie", "type=movie&tmdb=8392&imdb=tt0096283", "type=film&tmdb=8392"] {
        let refused = put(&app, &format!("/api/v1/overrides/external?{query}"), pin.clone()).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{query}: {:?}", refused.json);
    }
    let missing = put(&app, "/api/v1/overrides/external?type=movie&tmdb=1", pin.clone()).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let missing = put(&app, "/api/v1/overrides/external?type=series&tmdb=8392", pin).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "a movie's id names no series");
    assert!(pins(&app).await.is_empty());
}

/// The pin decides now, so what the rules proposed before it is withdrawn.
#[tokio::test]
async fn a_pin_by_external_id_withdraws_the_pending_proposals() {
    let app = two_copies().await;
    app.seed_anime_rule().await;
    app.simulate().await;
    let pending = "SELECT COUNT(*) FROM decisions WHERE media_id = 'm-1' AND superseded = 0";
    let before: i64 = sqlx::query_scalar(pending).fetch_one(&app.state.pool).await.unwrap();
    assert!(before > 0, "the simulation proposed nothing for the film");

    let pin = json!({ "target_category": "anime" });
    put(&app, "/api/v1/overrides/external?type=movie&tmdb=8392", pin).await.assert_ok();

    let after: i64 = sqlx::query_scalar(pending).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(after, 0);
}

/// Only what a pin or an unpin changes loses its proposals: pinning again to
/// the category a copy already has, or unpinning a copy that had no pin,
/// leaves the proposals somebody may be reviewing.
#[tokio::test]
async fn a_pin_or_unpin_that_changes_nothing_keeps_the_proposals() {
    let app = two_copies().await;
    app.seed_anime_rule().await;
    let first = "/api/v1/overrides/external?type=movie&tmdb=8392&instance=inst-1";
    let pin = json!({ "target_category": "anime" });
    put(&app, first, pin.clone()).await.assert_ok();
    app.simulate().await;
    let standing = |media: &'static str| {
        let app = &app;
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM decisions WHERE media_id = ? AND superseded = 0",
            )
            .bind(media)
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
        }
    };
    let (pinned, other) = (standing("m-1").await, standing("m-2").await);
    assert!(pinned > 0 && other > 0, "the simulation proposed nothing to keep");

    put(&app, first, pin).await.assert_ok();
    assert_eq!(standing("m-1").await, pinned, "repeating a pin withdrew what it produced");
    let second = "/api/v1/overrides/external?type=movie&tmdb=8392&instance=inst-2";
    assert_eq!(app.delete(second).await.assert_ok()["deleted"], false);
    assert_eq!(standing("m-2").await, other, "an unpin that removed nothing withdrew proposals");
}

/// A preview stores no decision, so its task keeps them: asked to answer at
/// once, a preview's caller finds the proposals in the task's result.
#[tokio::test]
async fn an_asynchronous_preview_keeps_its_decisions_in_the_task() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let body = json!({ "persist": false });
    let started = app.send(preferring_async("/api/v1/simulate", body)).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.json);
    let task = finished(&app, started.json["job_id"].as_str().unwrap()).await;

    let decisions = task["result"]["decisions"].as_array().expect("decisions in the result");
    assert!(!decisions.is_empty(), "the preview's proposals were dropped: {task}");
    assert_eq!(task["result"]["returned"], decisions.len());
}

/// A stored run and a preview are told apart on the Tasks screen, and a
/// screen looking for the run whose proposals it lists finds only stored ones.
#[tokio::test]
async fn a_preview_is_a_task_of_its_own_kind() {
    let app = TestApp::new().await;
    app.seed_library().await;

    app.post("/api/v1/simulate", json!({ "persist": false })).await.assert_ok();
    app.post("/api/v1/simulate", json!({})).await.assert_ok();

    let jobs = app.get("/api/v1/jobs").await;
    let mut kinds: Vec<&str> = jobs.assert_ok()["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|job| job["kind"].as_str())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["preview", "simulate"]);
}

/// Followed rather than waited for, a run says how far it has gone, and has
/// gone through every title once it has finished.
#[tokio::test]
async fn a_followed_simulation_counts_the_titles_it_evaluated() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;

    let started = app.send(preferring_async("/api/v1/simulate", json!({}))).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.json);
    let task = finished(&app, started.json["job_id"].as_str().unwrap()).await;

    let total = task["result"]["total_media"].as_u64().unwrap();
    assert!(total > 0, "{task}");
    assert_eq!(task["progress_total"], total, "{task}");
    assert_eq!(task["progress_current"], total, "{task}");
}

/// A request no library can satisfy is refused before any task starts, so a
/// caller asking for an answer at once gets the refusal, not a task that fails.
#[tokio::test]
async fn a_simulation_asked_for_what_cannot_be_is_refused_before_its_task() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let too_many: Vec<String> =
        (0..=crate::services::routing::BIND_CHUNK).map(|i| format!("i-{i}")).collect();

    for body in [json!({ "instance_ids": too_many }), json!({ "media_type": "film" })] {
        let refused = app.send(preferring_async("/api/v1/simulate", body.clone())).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{:?}", refused.json);
    }
    assert_eq!(app.count("SELECT COUNT(*) FROM jobs").await, 0, "a refused request left a task");
}

/// Every call that starts long work answers 202 at once when asked to, and
/// its finished task holds the report the waited call gives: an apply, an
/// apply-all, a revert and the sync of one instance.
#[tokio::test]
async fn every_long_call_answered_at_once_leaves_its_report_on_the_task() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 1).await;
    let simulation = app.simulate().await;
    let decision: String = sqlx::query_scalar("SELECT id FROM decisions WHERE simulation_id = ?")
        .bind(&simulation)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let every = ["batch", "threshold", "capacity", "unreachable"];
    let at_once = |path: &'static str, body: Value, counted: &'static str| {
        let app = &app;
        async move {
            let started = app.send(preferring_async(path, body)).await;
            assert_eq!(started.status, StatusCode::ACCEPTED, "{path}: {:?}", started.json);
            let task = finished(app, started.json["job_id"].as_str().unwrap()).await;
            assert_eq!(task["status"], "success", "{path}: {task}");
            assert_eq!(task["result"][counted], 1, "{path} left no report: {task}");
        }
    };

    let ids = json!({ "decision_ids": [decision], "confirm": every });
    at_once("/api/v1/decisions/apply", ids.clone(), "applied").await;
    at_once("/api/v1/decisions/revert", ids, "applied").await;
    let whole = json!({ "simulation_id": app.simulate().await, "confirm": every });
    at_once("/api/v1/decisions/apply-all", whole, "applied").await;
    at_once("/api/v1/instances/inst-1/sync", json!({}), "media").await;
}

#[tokio::test]
async fn respond_async_answers_the_started_task_whose_result_holds_the_report() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;

    let request = preferring_async("/api/v1/simulate", json!({ "persist": true }));
    let response = app.send_raw(request).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let headers = response.headers().clone();
    assert_eq!(headers["preference-applied"], "respond-async");
    let location = headers[axum::http::header::LOCATION].to_str().unwrap().to_string();
    let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    let accepted: Value = serde_json::from_slice(&bytes).unwrap();
    let job_id = accepted["job_id"].as_str().unwrap();
    assert_eq!(location, format!("/api/v1/jobs/{job_id}"));

    let task = finished(&app, job_id).await;
    assert_eq!(task["status"], "success");
    assert_eq!(task["result"]["moves_required"], 2, "{task}");
    assert_eq!(task["result"]["decisions"], json!([]), "the proposals are listed elsewhere");
    let simulation = task["result"]["simulation_id"].as_str().unwrap();
    let listed = app.get(&format!("/api/v1/decisions?simulation_id={simulation}")).await;
    assert_eq!(listed.assert_ok()["pagination"]["total"], 2);

    let sync = app.send_raw(preferring_async("/api/v1/instances/inst-1/sync", json!({}))).await;
    assert_eq!(sync.status(), StatusCode::ACCEPTED);
}

/// A question is asked before any work starts, so it answers at once
/// whatever the caller prefers, and nothing runs.
#[tokio::test]
async fn a_guardrail_still_answers_at_once_when_the_caller_prefers_not_to_wait() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let simulation = app.simulate().await;

    let body = json!({ "simulation_id": simulation });
    let asked = app.send(preferring_async("/api/v1/decisions/apply-all", body)).await;
    assert_eq!(asked.status, StatusCode::CONFLICT);
    assert_eq!(asked.json["confirm"], "batch");
    let applies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'apply'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(applies, 0);
    assert!(arr.recorded().writes.is_empty());
}

/// Waited for, the call answers its report, and its task keeps the same one.
#[tokio::test]
async fn a_task_keeps_the_report_its_call_answered() {
    let arr = FakeArr::start().await;
    let app = TestApp::films_to_move(&arr, 2).await;
    let simulation = app.simulate().await;

    let body = json!({ "simulation_id": simulation, "confirm": ["batch"] });
    let answered = app.post("/api/v1/decisions/apply-all", body).await;
    assert_eq!(answered.assert_ok()["applied"], 2);

    let stored: String = sqlx::query_scalar("SELECT result FROM jobs WHERE kind = 'apply'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), answered.json);
}

#[test]
fn respond_async_is_read_among_other_preferences_in_any_case() {
    let prefer = |value: &str| {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("prefer", value.parse().unwrap());
        prefers_async(&headers)
    };
    assert!(prefer("respond-async"));
    assert!(prefer("wait=10, Respond-Async"));
    assert!(!prefer("return=minimal"));
    assert!(!prefer("respond-asynchronously"));
    assert!(!prefers_async(&axum::http::HeaderMap::new()));
}

/// A script tells one warning from another by its code, whatever the language
/// the message is in.
#[tokio::test]
async fn each_warning_carries_a_stable_code_beside_its_message() {
    let app = TestApp::new().await;
    app.store_setting("ui_language", "fr").await;

    let status = app.get("/api/v1/status").await;
    let codes: Vec<&str> = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"api_unauthenticated"), "{codes:?}");
    assert!(codes.contains(&"no_enabled_instance"), "{codes:?}");
}

/// An asynchronous preview answers 202 before it holds a library pass, so a
/// caller that does not wait could otherwise queue them without end, and the
/// applies and automatic routing behind them would wait for hours. Past a few
/// waiting, a preview is refused before any task is made.
#[tokio::test]
async fn asynchronous_previews_queue_only_so_far() {
    let app = TestApp::new().await;
    app.seed_library().await;
    let held = (
        crate::services::routing::library_pass().await,
        crate::services::routing::library_pass().await,
    );

    let mut statuses = Vec::new();
    for _ in 0..6 {
        let preview = preferring_async("/api/v1/simulate", json!({ "persist": false }));
        statuses.push(app.send(preview).await.status);
    }
    let accepted = statuses.iter().filter(|status| **status == StatusCode::ACCEPTED).count();
    let refused = statuses.iter().filter(|status| **status == StatusCode::CONFLICT).count();
    assert_eq!((accepted, refused), (4, 2), "{statuses:?}");
    let started: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'preview'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(started, 4, "a refused preview left a task behind");
    drop(held);
}
