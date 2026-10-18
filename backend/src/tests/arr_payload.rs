//! What the Arrs send about a title beyond its folder: its quality profile, runtime, studio,
//! network and collection, which rules read straight off the media row.

use axum::http::StatusCode;
use serde_json::json;

use super::TestApp;
use super::fake_arr::FakeArr;
use crate::jobs::Attribution;
use crate::services::sync;

/// One media row's new fields, as the sync wrote them.
async fn fields(
    app: &TestApp,
    id: &str,
) -> (Option<String>, Option<i64>, Option<String>, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT quality_profile, runtime, studio, network, collection FROM media WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_sync_names_the_quality_profile_and_reads_runtime_studio_network_and_collection() {
    let arr = FakeArr::start().await;
    let radarr = TestApp::synced_from("radarr", &arr).await;
    let film = fields(&radarr, "m-inst-1-10").await;
    assert_eq!(film.0.as_deref(), Some("Ultra-HD"), "the profile is stored by its name");
    assert_eq!((film.1, film.2.as_deref()), (Some(86), Some("Studio Ghibli")));
    assert_eq!((film.3, film.4.as_deref()), (None, Some("Studio Ghibli Collection")));
    let catalogue = radarr.count("SELECT COUNT(*) FROM arr_quality_profiles").await;
    assert_eq!(catalogue, 3);

    let sonarr = TestApp::synced_from("sonarr", &arr).await;
    let series = fields(&sonarr, "m-inst-1-20").await;
    assert_eq!(series.0.as_deref(), Some("HD-720p"));
    assert_eq!((series.1, series.2, series.3.as_deref()), (Some(25), None, Some("TV Tokyo")));
}

/// A failed read of the profiles keeps the names the last good one stored, as the tags do,
/// or every `quality_profile_in` rule stops matching until the next pass.
#[tokio::test]
async fn a_profile_list_the_arr_fails_to_give_keeps_the_names_last_read() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    arr.break_profile_endpoint();

    sync::sync_instance(&app.state, "inst-1", &Attribution::manual(None)).await.unwrap();

    assert_eq!(fields(&app, "m-inst-1-10").await.0.as_deref(), Some("Ultra-HD"));
    assert_eq!(app.count("SELECT COUNT(*) FROM arr_quality_profiles").await, 3);
}

/// The webhook path names a profile from the stored list, and reads the list again only for an
/// id it does not hold, as a 4K profile created after the last sync.
#[tokio::test]
async fn a_title_added_with_a_profile_the_stored_list_lacks_reads_the_list_again() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.execute(&["DELETE FROM arr_quality_profiles WHERE arr_id = 4"]).await;
    let instance = app.state.instance("inst-1").await.unwrap();
    let asked =
        || arr.recorded().reads.iter().filter(|read| *read == "/api/v3/qualityprofile").count();
    let before = asked();

    sync::sync_single_media(&app.state, &instance, 10).await.unwrap();

    assert_eq!(asked(), before + 1);
    assert_eq!(fields(&app, "m-inst-1-10").await.0.as_deref(), Some("Ultra-HD"));
    assert_eq!(app.count("SELECT COUNT(*) FROM arr_quality_profiles").await, 3);

    sync::sync_single_media(&app.state, &instance, 10).await.unwrap();
    assert_eq!(asked(), before + 1, "a profile the stored list names was asked for again");
}

#[tokio::test]
async fn the_facets_offer_every_defined_profile_and_the_names_the_library_holds() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    let facets = app.get("/api/v1/media/facets").await;
    let facets = facets.assert_ok();

    let profiles: Vec<(String, i64)> = facets["quality_profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|facet| {
            (facet["value"].as_str().unwrap().to_string(), facet["count"].as_i64().unwrap())
        })
        .collect();
    assert_eq!(profiles[0], ("Ultra-HD".to_string(), 1));
    assert!(
        profiles.contains(&("HD-720p".to_string(), 0)),
        "an unused profile is offered: {profiles:?}"
    );
    assert_eq!(facets["studios"][0]["value"], "Studio Ghibli");
    assert_eq!(facets["collections"][0]["value"], "Studio Ghibli Collection");
    assert_eq!(facets["networks"], json!([]));
}

/// A rule on each new field matches the title holding it, through the real router and the
/// engine's folding, and a title whose value is unknown matches none.
#[tokio::test]
async fn a_rule_on_each_new_field_routes_the_title_that_holds_it() {
    let arr = FakeArr::start().await;
    for condition in [
        json!({ "type": "quality_profile_in", "value": ["ultra-hd"] }),
        json!({ "type": "runtime_range", "value": { "min": 80, "max": 90 } }),
        json!({ "type": "studio_in", "value": ["studio ghibli"] }),
        json!({ "type": "collection_in", "value": ["Studio Ghibli Collection"] }),
    ] {
        let app = TestApp::synced_from("radarr", &arr).await;
        app.seed_rule_on(condition.clone()).await;
        let explained = app.get("/api/v1/media/m-inst-1-10/explain").await;
        assert_eq!(explained.assert_ok()["rules_category"], "anime", "{condition}");

        app.execute(&[
            "UPDATE media SET quality_profile = NULL, runtime = NULL, studio = NULL, collection = NULL",
        ])
        .await;
        let explained = app.get("/api/v1/media/m-inst-1-10/explain").await;
        assert_ne!(explained.assert_ok()["rules_category"], "anime", "unknown matched {condition}");
    }
}

/// A title the Arr does not hold yet is judged on the profile it would be added with, which a
/// request bot names: a 4K request is routed before Radarr holds it.
#[tokio::test]
async fn a_title_the_arr_does_not_hold_is_judged_on_the_profile_it_would_be_added_with() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;
    app.seed_rule_on(json!({ "type": "quality_profile_in", "value": ["Ultra-HD"] })).await;
    app.execute(&["UPDATE root_folders SET category = 'anime' WHERE path = '/movies/anime'"]).await;

    let plain = app.get("/api/v1/route?type=movie&tmdb=129").await;
    let plain = &plain.assert_ok()["answers"][0];
    assert_ne!(plain["category"], "anime", "{plain}");
    let in_4k = app.get("/api/v1/route?type=movie&tmdb=129&quality_profile=Ultra-HD").await;
    let in_4k = &in_4k.assert_ok()["answers"][0];
    assert_eq!(
        (&in_4k["source"], &in_4k["category"]),
        (&json!("lookup"), &json!("anime")),
        "{in_4k}"
    );
}

/// The explanation carries what the Arr reported, which the panel shows beside the decision.
#[tokio::test]
async fn the_explanation_carries_what_the_arr_reports() {
    let arr = FakeArr::start().await;
    let app = TestApp::synced_from("radarr", &arr).await;

    let explained = app.get("/api/v1/media/m-inst-1-10/explain").await;
    let media = &explained.assert_ok()["media"];

    assert_eq!(media["quality_profile"], "Ultra-HD");
    assert_eq!(media["runtime"], 86);
    assert_eq!(media["collection_tmdb_id"], 1000);
    assert_eq!(explained.status, StatusCode::OK);
}
