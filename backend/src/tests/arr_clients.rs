//! Integration client behaviour, exercised against a real socket.

use crate::config::Config;
use crate::error::AppError;
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::radarr::RadarrClient;
use crate::integrations::sonarr::SonarrClient;

use super::fake_arr::FakeArr;

fn client() -> reqwest::Client {
    crate::http::build_client(&Config::for_tests()).expect("test http client")
}

// ------------------------------------------------------------------ Radarr

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

#[tokio::test]
async fn radarr_moves_the_whole_batch_in_one_call() {
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));

    let results = adapter.move_to_root_folder(&[1, 2, 3], "/movies/anime", true).await;

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|(_, outcome)| outcome.is_ok()));

    let recorded = arr.recorded();
    assert_eq!(recorded.writes.len(), 1, "one bulk call, not one per movie");
    assert_eq!(recorded.writes[0]["movieIds"], serde_json::json!([1, 2, 3]));
    assert_eq!(recorded.writes[0]["rootFolderPath"], "/movies/anime");
    assert_eq!(recorded.writes[0]["moveFiles"], true);
}

#[tokio::test]
async fn an_empty_batch_makes_no_request() {
    let arr = FakeArr::start().await;
    let radarr = RadarrClient::new(client(), &arr.base_url, "k");

    radarr.update_movies_root_folder(&[], "/movies/anime", false).await.unwrap();
    radarr.refresh_movies(&[]).await.unwrap();

    assert!(arr.recorded().writes.is_empty());
}

#[tokio::test]
async fn a_bulk_failure_is_reported_for_every_item() {
    let arr = FakeArr::failing(500).await;
    let adapter = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));

    let results = adapter.move_to_root_folder(&[1, 2], "/movies/anime", false).await;

    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|(_, outcome)| outcome.is_err()),
        "a failed bulk call must not report partial success"
    );
}

#[tokio::test]
async fn upstream_errors_carry_the_status_and_are_truncated() {
    let arr = FakeArr::failing(422).await;
    let radarr = RadarrClient::new(client(), &arr.base_url, "k");

    let error = radarr.update_movies_root_folder(&[1], "/movies/anime", false).await.unwrap_err();

    match error {
        AppError::ExternalApi { service, status, message, .. } => {
            assert_eq!(service, "Radarr");
            assert_eq!(status, 422);
            assert!(message.contains("rejected"));
            assert!(message.len() < 600);
        }
        other => panic!("expected an ExternalApi error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unreachable_host_is_a_transport_error_not_a_panic() {
    // Port 1 on the loopback: nothing listens, so the connection is refused at
    // once. A black-hole address (TEST-NET-1) waits the full timeout instead —
    // one to five seconds per test, depending on the host's routes.
    let radarr = RadarrClient::new(client(), "http://127.0.0.1:1", "k");

    match radarr.test_connection().await.unwrap_err() {
        AppError::ExternalApi { status, message, .. } => {
            assert_eq!(status, 0, "a transport failure has no HTTP status");
            assert!(
                message.contains("timed out") || message.contains("unreachable"),
                "unhelpful message: {message}"
            );
        }
        other => panic!("expected an ExternalApi error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_refresh_command_names_the_moved_items() {
    let arr = FakeArr::start().await;
    let adapter = ArrAdapter::Radarr(RadarrClient::new(client(), &arr.base_url, "k"));

    adapter.refresh(&[10, 11]).await.unwrap();

    let recorded = arr.recorded();
    assert_eq!(recorded.writes[0]["name"], "RefreshMovie");
    assert_eq!(recorded.writes[0]["movieIds"], serde_json::json!([10, 11]));
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

#[tokio::test]
async fn moving_a_series_keeps_its_existing_folder_name() {
    let arr = FakeArr::with_series_path("/tv/standard/Cowboy Bebop (1998)").await;
    let sonarr = SonarrClient::new(client(), &arr.base_url, "k");

    sonarr.update_series_path(20, "/tv/anime", true).await.unwrap();

    let recorded = arr.recorded();
    let sent = &recorded.writes[0];
    assert_eq!(sent["rootFolderPath"], "/tv/anime");
    assert_eq!(
        sent["path"], "/tv/anime/Cowboy Bebop (1998)",
        "deriving the folder from titleSlug would silently rename it on disk"
    );
    assert_eq!(sent["qualityProfileId"], 3, "unrelated fields must survive the round trip");
    assert_eq!(recorded.query_strings[0], "moveFiles=true");
}

#[tokio::test]
async fn a_series_that_was_never_scanned_falls_back_to_the_slug() {
    let arr = FakeArr::with_series_path("").await;
    let sonarr = SonarrClient::new(client(), &arr.base_url, "k");

    sonarr.update_series_path(20, "/tv/anime", false).await.unwrap();

    assert_eq!(arr.recorded().writes[0]["path"], "/tv/anime/cowboy-bebop");
}

#[tokio::test]
async fn sonarr_reports_failures_per_item() {
    let arr = FakeArr::failing(409).await;
    let adapter = ArrAdapter::Sonarr(SonarrClient::new(client(), &arr.base_url, "k"));

    let results = adapter.move_to_root_folder(&[20, 21], "/tv/anime", false).await;

    assert_eq!(results.len(), 2, "one failure must not abort the rest of the batch");
    assert!(results.iter().all(|(_, outcome)| outcome.is_err()));
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
/// `update_series_path` reads the series back, patches two fields and re-sends
/// it, because Sonarr has no bulk editor. Indexing a `serde_json::Value` that
/// is not an object *panics* — `[]`, a string and a number all do — so a
/// reverse proxy answering 200 with a cached empty array, or a base URL
/// pointing at some other service on the same host, aborted the apply with a
/// 500 that named nothing. There is a `CatchPanicLayer`, which is why it was a
/// 500 and not a dropped connection; it is not a reason to panic.
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
