//! Integration client behaviour, exercised against a real socket.

use crate::error::AppError;
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::radarr::RadarrClient;
use crate::integrations::sonarr::SonarrClient;

use super::fake_arr::FakeArr;
use super::http_client as client;

// ------------------------------------------------------------------ Radarr

/// An answer has a size beyond which it is not read: a hostile or intercepted
/// address streaming without end stops at the cap, at once, rather than
/// filling memory until the timeout, and an error body the same.
#[tokio::test]
async fn an_answer_without_end_stops_at_the_cap() {
    use axum::routing::get;

    let endless = || async {
        let chunk = axum::body::Bytes::from(vec![b' '; 64 * 1024]);
        let stream = futures::stream::repeat_with(move || Ok::<_, std::io::Error>(chunk.clone()));
        axum::body::Body::from_stream(stream)
    };
    let app = axum::Router::new().route("/api/v3/system/status", get(endless)).route(
        "/api/v3/tag",
        get(move || async move { (axum::http::StatusCode::BAD_REQUEST, endless().await) }),
    );
    let address = super::serve(app).await;
    // Five seconds to answer, so a read that went on until the timeout shows.
    let config = crate::config::Config {
        http_timeout: std::time::Duration::from_secs(5),
        ..crate::config::Config::for_tests()
    };
    let radarr = RadarrClient::new(crate::http::build_client(&config).unwrap(), &address, "k");

    let started = std::time::Instant::now();
    let answered = radarr.test_connection().await;
    let Err(AppError::ExternalApi { message, .. }) = &answered else {
        panic!("an endless answer was read: {answered:?}");
    };
    assert!(message.contains("larger than"), "{message}");
    let refused = radarr.get_tags().await;
    assert!(matches!(refused, Err(AppError::ExternalApi { status: 400, .. })), "{refused:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

/// A write redirected with a 301 or a 302 reaches its new address as a GET,
/// without its body, and the GET's 200 would read as the write done. Refused,
/// the move is reported as failed rather than recorded as made.
#[tokio::test]
async fn a_write_redirected_to_a_read_is_a_failure() {
    use axum::routing::get;

    let film = || async { axum::Json(serde_json::json!({ "id": 10, "path": "/movies/Totoro" })) };
    let app = axum::Router::new()
        .route(
            "/api/v3/movie/10",
            get(film).put(|| async { axum::response::Redirect::to("/api/v3/movie/10/read") }),
        )
        .route("/api/v3/movie/10/read", get(film));
    let address = super::serve(app).await;
    let radarr = RadarrClient::new(client(), &address, "k");

    let moved = radarr.update_movie_path(10, "/movies/anime", false).await;

    assert!(moved.is_err(), "a redirected move read as done: {moved:?}");
}

#[tokio::test]
async fn radarr_sends_the_api_key_and_parses_the_status() {
    let arr = FakeArr::start().await;
    let radarr = RadarrClient::new(client(), &arr.base_url, "arr-key");

    let status = radarr.test_connection().await.unwrap();
    assert_eq!(status.version, "5.2.6.8376");
    assert_eq!(arr.recorded().api_keys, vec!["arr-key"]);
}

#[tokio::test]
async fn radarr_maps_movies_onto_the_shared_shape() {
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));

    let media = adapter.get_media().await.unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].media_type, "movie");
    assert_eq!(media[0].tmdb_id, Some(8392));
    assert!(media[0].has_files);
    assert_eq!(media[0].root_folder_path.as_deref(), Some("/movies/standard"));
}

#[tokio::test]
async fn root_folder_accessibility_is_preserved() {
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));

    let folders = adapter.get_root_folders().await.unwrap();
    assert_eq!(folders.len(), 3);
    assert!(folders[0].accessible);
    assert!(!folders[1].accessible, "an inaccessible folder must not look usable");
    assert!(folders[2].accessible);
}

/// The refusal names its service and status, and keeps the start of what the
/// Arr said, cut: the fake's refusal is over a thousand characters long, and
/// the message goes into the log and back to the screen.
#[tokio::test]
async fn upstream_errors_carry_the_status_and_are_truncated() {
    let arr = FakeArr::failing(422).await;
    let radarr = RadarrClient::new(client(), &arr.base_url, "k");

    let error = radarr.update_movie_path(10, "/movies/anime", false).await.unwrap_err();

    match error {
        AppError::ExternalApi { service, status, message, .. } => {
            assert_eq!(service, "Radarr");
            assert_eq!(status, 422);
            assert!(message.starts_with("upstream rejected the edit"), "{message}");
            assert!(message.ends_with("(truncated)"), "{message}");
            assert!(message.chars().count() < 600, "{} characters", message.chars().count());
        }
        other => panic!("expected an ExternalApi error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_refused_connection_is_named_unreachable_with_no_status() {
    // Port 1 on the loopback: nothing listens, so the connection is refused at
    // once. A black-hole address (TEST-NET-1) waits the full timeout instead,
    // on every test.
    let radarr = RadarrClient::new(client(), "http://127.0.0.1:1", "k");

    match radarr.test_connection().await.unwrap_err() {
        AppError::ExternalApi { status, message, .. } => {
            assert_eq!(status, 0, "a transport failure has no HTTP status");
            assert!(message.contains("unreachable"), "unhelpful message: {message}");
        }
        other => panic!("expected an ExternalApi error, got {other:?}"),
    }
}

/// Radarr filters its list by `tmdbId`, and a proxy or a fork that ignores the
/// filter answers the whole library: the film looked up is the one asked.
#[tokio::test]
async fn a_radarr_lookup_is_held_to_the_film_asked() {
    use axum::routing::get;

    let app = axum::Router::new()
        .route(
            "/api/v3/movie/lookup/tmdb",
            get(|| async { axum::Json(serde_json::json!({ "title": "Totoro", "tmdbId": 8392 })) }),
        )
        .route(
            "/api/v3/movie",
            get(|| async {
                axum::Json(serde_json::json!([
                    { "id": 1, "title": "Akira", "tmdbId": 149 },
                    { "id": 10, "title": "Totoro", "tmdbId": 8392 },
                ]))
            }),
        );
    let address = super::serve(app).await;
    let radarr = RadarrClient::new(client(), &address, "k");

    let found = radarr.lookup_movie(&crate::models::ExternalId::Tmdb(8392)).await.unwrap();

    assert_eq!(found.map(|movie| movie.id), Some(10), "another film of the library was taken");
}

// ------------------------------------------------------------------ Sonarr

#[tokio::test]
async fn sonarr_derives_has_files_from_the_statistics_block() {
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Sonarr(SonarrClient::new(client(), &arr.base_url, "k"));

    let media = adapter.get_media().await.unwrap();
    assert_eq!(media[0].media_type, "series");
    assert!(media[0].has_files);
    assert_eq!(media[0].tvdb_id, Some(76885));
}

/// Both Arrs are sent the title's whole record with its path in full: the
/// folder name kept, deriving it from the slug would rename it on disk, and
/// every other field kept, since a PUT drops what it leaves out.
#[tokio::test]
async fn a_move_keeps_the_folder_name_and_every_other_field() {
    let arr = FakeArr::with_series_path("/tv/standard/Cowboy Bebop (1998)").await;
    let radarr = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));
    let sonarr = ArrAdapter::Sonarr(SonarrClient::new(client(), &arr.base_url, "k"));

    let film = radarr.move_item(10, "/movies/anime", true).await.unwrap();
    let series = sonarr.move_item(20, "/tv/anime/", false).await.unwrap();

    assert_eq!(film, "/movies/anime/My Neighbor Totoro (1988)");
    assert_eq!(series, "/tv/anime/Cowboy Bebop (1998)");
    let recorded = arr.recorded();
    assert_eq!(recorded.writes[0]["path"], film);
    assert_eq!(recorded.writes[0]["certification"], "G", "a field was dropped");
    assert_eq!(recorded.writes[1]["rootFolderPath"], "/tv/anime/");
    assert_eq!(recorded.writes[1]["path"], series);
    assert_eq!(recorded.writes[1]["qualityProfileId"], 3, "a field was dropped");
    assert_eq!(recorded.query_strings, ["moveFiles=true", "moveFiles=false"]);
}

#[tokio::test]
async fn a_series_that_was_never_scanned_falls_back_to_the_slug() {
    let arr = FakeArr::with_series_path("").await;
    let sonarr = SonarrClient::new(client(), &arr.base_url, "k");

    sonarr.update_series_path(20, "/tv/anime", false).await.unwrap();

    assert_eq!(arr.recorded().writes[0]["path"], "/tv/anime/cowboy-bebop");
}

#[tokio::test]
async fn the_adapter_refuses_an_unknown_instance_type() {
    let instance = crate::models::Instance {
        id: "i".into(),
        name: "Lidarr".into(),
        instance_type: "lidarr".into(),
        base_url: "http://localhost:8686".into(),
        api_key: "k".into(),
        enabled: true,
        sync_interval_minutes: 15,
        last_sync_at: None,
        last_sync_attempt_at: None,
        last_sync_status: None,
        created_at: String::new(),
        updated_at: String::new(),
        webhook_token: None,
    };

    assert!(ArrAdapter::for_instance(client(), &instance, "k").is_err());
}

/// A 2xx body that is not a series object is reported, not patched.
///
/// A move reads the series back, patches two fields and re-sends it.
/// Indexing a `serde_json::Value` that
/// is not an object *panics* (`[]`, a string and a number all do), so a reverse
/// proxy answering 200 with a cached empty array, or a base URL pointing at
/// some other service on the same host, would abort the apply with a 500 that
/// names nothing. The `CatchPanicLayer` makes a panic that 500 rather than a
/// dropped connection, which is no reason to panic.
#[tokio::test]
async fn a_series_payload_that_is_not_an_object_is_reported_rather_than_patched() {
    for body in [serde_json::json!([]), serde_json::json!("error"), serde_json::json!(12)] {
        let arr = FakeArr::answering_series_with(body.clone()).await;
        let sonarr = SonarrClient::new(client(), &arr.base_url, "k");

        let outcome = sonarr.update_series_path(20, "/tv/anime", true).await;

        let error = outcome.expect_err(&format!("{body} must not be accepted as a series"));
        let described = error.to_string();
        assert!(described.to_lowercase().contains("sonarr"), "must name the source: {described}");
        assert!(
            arr.recorded().writes.is_empty(),
            "nothing may be sent back when the payload was not understood: {described}"
        );
    }
}

/// A series Sonarr knows and does not hold is found by its TheTVDB id and by
/// its IMDb id, each held to the id asked, and comes back as the lookup
/// describes it: no Arr id, no folder, the type and monitoring it would be
/// added with by default.
#[tokio::test]
async fn sonarr_finds_a_series_it_does_not_hold_by_either_id() {
    use crate::models::ExternalId;
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Sonarr(SonarrClient::new(client(), &arr.base_url, "k"));

    for id in [ExternalId::Tvdb(81178), ExternalId::Imdb("tt0807832".into())] {
        let found = adapter.lookup(&id).await.unwrap().unwrap_or_else(|| panic!("{id:?} missed"));
        assert_eq!(
            (found.arr_id, found.title.as_str(), found.tvdb_id, found.imdb_id.as_deref()),
            (0, "Mushishi", Some(81178), Some("tt0807832")),
            "{id:?}"
        );
        assert_eq!(found.series_type.as_deref(), Some("standard"));
        assert!(!found.monitored && found.path.is_none(), "{id:?}");
    }
    let unknown = adapter.lookup(&ExternalId::Tvdb(1)).await.unwrap();
    assert!(unknown.is_none(), "an id nobody knows was found");
}
