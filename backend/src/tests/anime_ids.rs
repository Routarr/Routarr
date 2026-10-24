//! The anime identifier table. The fake AniList holds Totoro (523) and, for its probes, Cowboy
//! Bebop (1), a second work a table can name.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

use super::TestApp;
use super::fake_sources::FakeSources;
use crate::jobs::Attribution;
use crate::services::enrichment;

/// A table naming the test library's Totoro, TMDB film 8392, as the AniList id `anilist`.
fn table_for_totoro(anilist: i64) -> String {
    json!([
        { "type": "MOVIE", "anilist_id": anilist, "mal_id": 523, "imdb_id": ["tt0096283"],
          "themoviedb_id": { "movie": [8392] } },
        { "type": "TV", "anilist_id": 1, "mal_id": 1, "tvdb_id": 76885,
          "season": { "tvdb": 1, "tmdb": 1 } }
    ])
    .to_string()
}

async fn import(app: &TestApp, body: &str) -> super::TestResponse {
    app.send(
        Request::post("/api/v1/sources/anime-ids/import")
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap(),
    )
    .await
}

async fn pass(app: &TestApp) {
    enrichment::enrich_all_media(&app.state, &Attribution::manual(None)).await.unwrap();
}

/// How `source` holds the film: its id and how it was found.
async fn identified(app: &TestApp, source: &str) -> Option<(Option<String>, String)> {
    sqlx::query_as(
        "SELECT external_id, found_by FROM source_identifiers WHERE source = ? AND local_key = ?",
    )
    .bind(source)
    .bind("tmdb:8392")
    .fetch_optional(&app.state.pool)
    .await
    .unwrap()
}

fn asked(sources: &FakeSources) -> Vec<String> {
    let recorded = sources.recorded();
    recorded
        .details
        .iter()
        .filter(|(source, _)| *source == "anilist")
        .map(|(_, id)| id.clone())
        .collect()
}

fn searched(sources: &FakeSources) -> usize {
    sources.recorded().searches.iter().filter(|(source, _)| *source == "anilist").count()
}

#[tokio::test]
async fn a_title_the_table_names_is_read_by_its_ids_and_never_searched() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;

    let loaded = import(&app, &table_for_totoro(523)).await;
    assert_eq!(
        loaded.assert_ok(),
        &json!({ "entries": 2, "titles": 3 }),
        "one id per anime source"
    );
    pass(&app).await;

    assert_eq!(searched(&sources), 0, "a title the table names was searched");
    assert_eq!(asked(&sources), ["523"]);
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "mapping".into())));
    assert_eq!(identified(&app, "tenrai").await, Some((Some("523".into()), "mapping".into())));
}

/// Emptying `source_identifiers` stands for a title the load did not see.
#[tokio::test]
async fn a_title_added_after_the_load_is_read_by_the_table() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(523)).await.assert_ok();
    app.execute(&["DELETE FROM source_identifiers"]).await;

    pass(&app).await;

    assert_eq!(searched(&sources), 0, "a title the table names was searched");
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "mapping".into())));
}

#[tokio::test]
async fn the_table_wins_over_what_a_search_found() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    pass(&app).await;
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "search".into())));

    import(&app, &table_for_totoro(1)).await.assert_ok();
    assert_eq!(identified(&app, "anilist").await, Some((Some("1".into()), "mapping".into())));
    pass(&app).await;

    assert_eq!(asked(&sources).last().map(String::as_str), Some("1"));
}

/// An id the source does not hold is set aside as a miss is, and the title found by search again.
/// The refusal lasts a miss's lifetime, a refresh of the title or the source asks the table's id
/// again, and a 400, which speaks of the request rather than the work, sets nothing aside.
#[tokio::test]
async fn an_id_the_source_refuses_sends_the_title_back_to_a_search() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(999)).await.assert_ok();
    let asked_999 = || asked(&sources).iter().filter(|id| *id == "999").count();

    pass(&app).await;
    assert_eq!(identified(&app, "anilist").await, None, "the refused id is still held");
    let refused =
        app.count("SELECT COUNT(*) FROM anime_ids_refused WHERE external_id = '999'").await;
    assert_eq!(refused, 1);

    pass(&app).await;
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "search".into())));
    import(&app, &table_for_totoro(999)).await.assert_ok();
    assert_eq!(
        identified(&app, "anilist").await,
        Some((Some("523".into()), "search".into())),
        "a load brought a refused id back"
    );

    assert_eq!(asked_999(), 1);
    app.post("/api/v1/sources/refresh", json!({ "media_id": "m-1" })).await.assert_ok();
    assert_eq!(asked_999(), 2, "a refresh of the title kept the refusal");
    app.post("/api/v1/sources/refresh", json!({ "source": "anilist" })).await.assert_ok();
    assert_eq!(asked_999(), 3, "a refresh of the source kept its refusal");

    app.execute(&[
        "UPDATE anime_ids_refused SET refused_at = datetime('now', '-41 days')",
        "DELETE FROM source_identifiers WHERE source = 'anilist'",
    ])
    .await;
    pass(&app).await;
    assert_eq!(
        identified(&app, "anilist").await,
        Some((Some("999".into()), "mapping".into())),
        "a refusal outlived a miss's lifetime"
    );

    let refusing = FakeSources::failing(400).await;
    let app = TestApp::one_film_on(&refusing, "anilist").await;
    import(&app, &table_for_totoro(523)).await.assert_ok();
    pass(&app).await;
    assert_eq!(refusing.recorded().paths.len(), 1, "the control: the table's id was asked");
    assert_eq!(app.count("SELECT COUNT(*) FROM anime_ids_refused").await, 0, "a 400 set it aside");
}

#[tokio::test]
async fn an_unreadable_or_empty_file_is_refused_and_the_table_kept() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(523)).await.assert_ok();

    for (body, said) in [
        ("{}", "The file is not an anime identifier table"),
        ("[{\"type\":\"OVA\",\"kitsu_id\":5}]", "The file names no AniList or MyAnimeList id"),
    ] {
        let refused = import(&app, body).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{body}");
        let message = refused.json["message"].as_str().unwrap_or_default();
        assert!(message.starts_with(said), "{message}");
    }
    let state = app.get("/api/v1/sources/anime-ids").await.assert_ok().clone();
    assert_eq!((state["entries"].clone(), state["origin"].clone()), (json!(2), json!("file")));
}

/// A table served the way a person's mirror or GitHub serves it, counting what it answered.
async fn served_table(app: &TestApp, body: String) -> (TestApp, Arc<AtomicUsize>, super::Served) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let router = axum::Router::new().route(
        "/table.json",
        axum::routing::get(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            async move { ([("content-type", "application/json")], body) }
        }),
    );
    let server = super::serve(router).await;
    let mut config = (*app.state.config).clone();
    config.anime_ids_url = format!("{}/table.json", server.address);
    (TestApp::around(app.state.clone().with_config(config)), hits, server)
}

async fn tick(app: &TestApp) {
    let mut synced = Default::default();
    crate::jobs::scheduler::tick(&app.state, &mut synced, &mut None, &mut None).await.unwrap();
}

async fn settled(app: &TestApp) {
    for _ in 0..200 {
        let running = app.count("SELECT COUNT(*) FROM jobs WHERE status = 'running'").await;
        if running == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("a task never ended");
}

/// The weekly download, once on, asks once a week and, after a failed try, once a day.
#[tokio::test]
async fn the_table_is_downloaded_only_when_somebody_asks() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    app.store_setting("auto_sync_enabled", "false").await;
    app.store_setting("backup_enabled", "false").await;
    let (app, hits, _server) = served_table(&app, table_for_totoro(523)).await;

    tick(&app).await;
    settled(&app).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "downloaded with the weekly download off");

    app.post("/api/v1/sources/anime-ids/download", json!({})).await.assert_ok();
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the control: a download asked for");

    app.store_setting("anime_ids_weekly", "true").await;
    tick(&app).await;
    settled(&app).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1, "downloaded again within the week");

    app.execute(&[
        "UPDATE anime_ids_state SET loaded_at = datetime('now', '-8 days')",
        "UPDATE jobs SET started_at = datetime('now', '-2 days')",
    ])
    .await;
    tick(&app).await;
    settled(&app).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2, "not downloaded once the week passed");

    // A load tried today waits for tomorrow, or a failing one would ask at every tick.
    app.execute(&["UPDATE anime_ids_state SET loaded_at = datetime('now', '-8 days')"]).await;
    tick(&app).await;
    settled(&app).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2, "tried again within the day");
}

/// A table's address may carry credentials, a private mirror's or a token in the query: the
/// request sends them, and the log, the stored origin and the screen show the address without.
#[tokio::test]
async fn the_table_address_is_shown_without_its_credentials() {
    use tracing_subscriber::layer::SubscriberExt;
    let log = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(log.clone()));
    let _logging = tracing::subscriber::set_default(subscriber);
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    let (app, hits, server) = served_table(&app, table_for_totoro(523)).await;
    let mut config = (*app.state.config).clone();
    let host = server.address.trim_start_matches("http://");
    config.anime_ids_url = format!("http://user:secret@{host}/table.json?token=secret");
    let app = TestApp::around(app.state.clone().with_config(config));

    app.post("/api/v1/sources/anime-ids/download", json!({})).await.assert_ok();

    assert_eq!(hits.load(Ordering::SeqCst), 1, "the control: the table was downloaded");
    let shown = app.get("/api/v1/sources/anime-ids").await.assert_ok().clone();
    let contents = log.contents();
    let started = contents.lines().find(|line| line.contains("Downloading the anime identifiers"));
    let started = started.unwrap_or_else(|| panic!("no start in:\n{contents}")).to_string();
    for said in [shown["address"].to_string(), shown["origin"].to_string(), started] {
        assert!(said.contains("/table.json"), "the address is not shown: {said}");
        assert!(!said.contains("secret"), "a credential is shown: {said}");
    }
}

/// The explanation names how each source found the title, so a wrong match can be seen.
#[tokio::test]
async fn the_explanation_says_how_each_source_found_the_title() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    pass(&app).await;
    let said = |explained: &Value| explained["identified_by"]["anilist"].clone();
    assert_eq!(said(app.get("/api/v1/library/m-1/explain").await.assert_ok()), "search");

    import(&app, &table_for_totoro(523)).await.assert_ok();
    assert_eq!(said(app.get("/api/v1/library/m-1/explain").await.assert_ok()), "mapping");
}

/// Refreshing one title forgets what was searched for it and asks again: the table first.
#[tokio::test]
async fn a_title_asked_alone_is_read_by_the_table_before_a_search() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(523)).await.assert_ok();

    app.post("/api/v1/sources/refresh", json!({ "media_id": "m-1" })).await.assert_ok();

    assert_eq!(searched(&sources), 0, "a title the table names was searched");
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "mapping".into())));
}

/// A title asked about alone ends as a pass leaves it: the table's id the source refuses is set
/// aside, not kept as the title's, and the next pass searches the title.
#[tokio::test]
async fn a_title_asked_alone_whose_table_id_is_refused_is_left_to_a_search() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(999)).await.assert_ok();

    app.post("/api/v1/sources/refresh", json!({ "media_id": "m-1" })).await.assert_ok();

    assert_eq!(asked(&sources), ["999"], "the control: the table's id was asked");
    let refused =
        app.count("SELECT COUNT(*) FROM anime_ids_refused WHERE external_id = '999'").await;
    assert_eq!(refused, 1, "the refusal was not recorded");
    assert_ne!(identified(&app, "anilist").await, Some((Some("999".into()), "mapping".into())));
    pass(&app).await;
    assert_eq!(searched(&sources), 1, "the next pass did not search the title");
}

/// A load replaces what the table said, its mappings included: a title the new table does not
/// name loses the mapping and is searched at the next pass.
#[tokio::test]
async fn a_title_a_new_table_no_longer_names_is_searched_again() {
    let sources = FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "anilist").await;
    import(&app, &table_for_totoro(523)).await.assert_ok();
    assert_eq!(identified(&app, "anilist").await, Some((Some("523".into()), "mapping".into())));
    let series_alone = json!([{ "type": "TV", "anilist_id": 1, "mal_id": 1, "tvdb_id": 76885,
                                "season": { "tvdb": 1, "tmdb": 1 } }]);

    import(&app, &series_alone.to_string()).await.assert_ok();

    assert_eq!(identified(&app, "anilist").await, None, "a withdrawn mapping was kept");
    pass(&app).await;
    assert_eq!(searched(&sources), 1, "the title was not searched once its mapping went");
}
