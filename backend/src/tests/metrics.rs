//! Prometheus exposition.
//!
//! The format is unforgiving: one malformed line and the scraper discards the
//! whole response. Since category names come from the user, most of what is
//! worth testing is about surviving what they might type.

use super::TestApp;

async fn scrape(app: &TestApp) -> String {
    let response = app.get("/api/v1/metrics").await;
    assert!(response.status.is_success());
    // The body is text, not JSON, so the harness leaves `json` null. A second
    // call keeps the raw bytes.
    app.text("/api/v1/metrics").await
}

/// Every sample line must be `name{labels} value` or `name value`.
fn assert_well_formed(body: &str) {
    for line in body.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let (metric, value) = line.rsplit_once(' ').unwrap_or_else(|| panic!("no value: {line}"));
        assert!(value.parse::<f64>().is_ok(), "value is not a number in: {line}");
        assert!(!metric.is_empty(), "empty metric name in: {line}");
        // Braces balance, or the scraper rejects the whole payload.
        assert_eq!(
            metric.matches('{').count(),
            metric.matches('}').count(),
            "unbalanced braces in: {line}"
        );
    }
}

#[tokio::test]
async fn every_family_declares_its_help_and_type() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = scrape(&app).await;

    // A family without HELP/TYPE still scrapes, but nothing downstream can say
    // what it means.
    let names: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("# HELP "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(!names.is_empty());
    for name in &names {
        assert!(body.contains(&format!("# TYPE {name} gauge")), "{name} has no TYPE");
        assert!(name.starts_with("routarr_"), "{name} is not namespaced");
    }
}

/// The category gauge counts each title where the last run sends it, a title
/// already there included, and the status gauge each title's latest standing
/// decision, not the applied history an apply leaves standing.
#[tokio::test]
async fn the_decision_gauges_count_where_each_title_stands_now() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                target_category, action, status, decided_at)
         VALUES ('d-old', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move',
                 'applied', '2026-09-01 10:00:00'),
                ('d-new', 'm-1', 'Totoro', 'movie', 'inst-1', 'standard', 'move',
                 'pending', '2026-09-02 10:00:00')",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, current_root_folder,
                            monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Heat', '/movies/standard', 1, 1)",
        "INSERT INTO media_routing (media_id, category, load_order, evaluated_at)
         VALUES ('m-1', 'standard', 1, '2026-09-02 10:00:00'),
                ('m-2', 'standard', 1, '2026-09-02 10:00:00')",
    ])
    .await;

    let body = scrape(&app).await;

    assert!(body.contains("routarr_decisions_by_category{category=\"standard\"} 2"), "{body}");
    assert!(!body.contains("routarr_decisions_by_category{category=\"anime\"}"), "{body}");
    assert!(body.contains("routarr_decisions_by_status{status=\"pending\"} 1"), "{body}");
    assert!(!body.contains("routarr_decisions_by_status{status=\"applied\"}"), "{body}");
}

#[tokio::test]
async fn the_library_is_reported_per_instance_and_type() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = scrape(&app).await;

    assert!(
        body.contains(
            r#"routarr_media_total{arr_instance="Radarr",arr_instance_id="inst-1",media_type="movie"} 1"#
        ),
        "got:\n{body}"
    );
    assert_well_formed(&body);
}

#[tokio::test]
async fn instance_health_is_a_gauge_worth_alerting_on() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // Never synced: not up.
    let body = scrape(&app).await;
    assert!(
        body.contains(r#"routarr_instance_up{arr_instance="Radarr",arr_instance_id="inst-1"} 0"#),
        "got:\n{body}"
    );

    sqlx::query("UPDATE instances SET last_sync_status = 'success'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let body = scrape(&app).await;
    assert!(
        body.contains(r#"routarr_instance_up{arr_instance="Radarr",arr_instance_id="inst-1"} 1"#),
        "got:\n{body}"
    );
}

/// An instance switched off is told apart from one that is down, and each
/// job outcome is a series of its own, so a run of failed applies can alert.
#[tokio::test]
async fn the_enabled_switch_and_each_job_outcome_are_series() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-2', 'Sonarr', 'sonarr', 'http://127.0.0.1:1', 'secret', 0, 'tok2')",
        "INSERT INTO jobs (id, kind, status) VALUES ('j-1', 'sync', 'success'),
                ('j-2', 'sync', 'success'), ('j-3', 'apply', 'failed'), ('j-4', 'apply', 'success')",
    ])
    .await;

    let body = scrape(&app).await;

    for line in [
        r#"routarr_instance_enabled{arr_instance="Radarr",arr_instance_id="inst-1"} 1"#,
        r#"routarr_instance_enabled{arr_instance="Sonarr",arr_instance_id="inst-2"} 0"#,
        r#"routarr_jobs_total{kind="sync",status="success"} 2"#,
        r#"routarr_jobs_total{kind="apply",status="failed"} 1"#,
        r#"routarr_jobs_total{kind="apply",status="success"} 1"#,
    ] {
        assert!(body.lines().any(|l| l == line), "{line} missing from:\n{body}");
    }
}

/// Prometheus sets `instance` on every scraped series to the target it
/// scraped, so a label of that name is renamed `exported_instance` on the way
/// in. Each instance is its own series, by its id.
#[tokio::test]
async fn each_instance_is_a_series_of_its_own() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-2', 'Radarr 4K', 'radarr', 'http://radarr-4k:7878', 'secret', 1, 'tok2')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let body = scrape(&app).await;

    let up: Vec<&str> =
        body.lines().filter(|line| line.starts_with("routarr_instance_up{")).collect();
    assert_eq!(up.len(), 2, "got:\n{body}");
    assert_ne!(up[0], up[1], "got:\n{body}");
    assert!(!body.contains("{instance="), "a label Prometheus renames:\n{body}");
    assert_well_formed(&body);
}

#[tokio::test]
async fn the_guardrail_switches_are_exposed() {
    let app = TestApp::new().await;

    // "Why has nothing moved for a week" is answered by these two, and a graph
    // annotation beats reading a settings page after the fact.
    let body = scrape(&app).await;
    assert!(body.contains("routarr_dry_run 1"), "dry-run defaults to on:\n{body}");
    assert!(body.contains("routarr_auto_apply_enabled 0"), "got:\n{body}");

    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    assert!(scrape(&app).await.contains("routarr_dry_run 0"));
}

/// A source down, a quota spent and titles nothing describes are watched as
/// an instance down is: a source never probed has no health sample, an
/// instance's probe is not a source's, and the quota of a source without
/// one is not reported.
#[tokio::test]
async fn each_metadata_source_reports_its_health_cache_and_quota() {
    let sources = super::fake_sources::FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,omdb,anilist,jikan").await;
    app.execute(&[
        "INSERT INTO probe_results (subject, reachable, detail, checked_at)
         VALUES ('source:anilist', 1, NULL, datetime('now')),
                ('source:omdb', 0, 'quota spent', datetime('now')),
                ('instance:inst-1', 0, 'HTTP 503', datetime('now'))",
        "INSERT INTO metadata_cache (source, external_id, media_type, genres)
         VALUES ('omdb', 'tt0096283', 'movie', '[\"Animation\"]'),
                ('anilist', '523', 'movie', '[]'),
                ('anilist', '524', 'movie', '[]')",
        "INSERT INTO source_requests (source, day, spent) VALUES ('omdb', date('now'), 1000)",
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
         VALUES ('m-2', 'inst-1', 11, 'movie', 'Heat', 1, 1)",
    ])
    .await;

    let body = scrape(&app).await;

    let source = |name: &str, id: &str| format!("{name}{{source=\"{id}\"}}");
    for (series, value) in [
        (source("routarr_metadata_source_up", "anilist"), 1),
        (source("routarr_metadata_source_up", "omdb"), 0),
        (source("routarr_metadata_cache_entries", "anilist"), 2),
        (source("routarr_metadata_cache_entries", "omdb"), 1),
        (source("routarr_metadata_quota_spent", "omdb"), 1000),
        (source("routarr_metadata_quota_limit", "omdb"), 1000),
    ] {
        assert!(body.contains(&format!("{series} {value}\n")), "no {series} {value} in:\n{body}");
    }
    for absent in [
        source("routarr_metadata_source_up", "jikan"),
        source("routarr_metadata_source_up", "inst-1"),
        source("routarr_metadata_quota_spent", "anilist"),
    ] {
        assert!(!body.contains(&absent), "{absent} in:\n{body}");
    }
    // Totoro is described by OMDb, Heat by nothing.
    assert!(
        body.contains(
            "routarr_media_without_metadata{arr_instance=\"Arr\",arr_instance_id=\"inst-1\",\
             media_type=\"movie\"} 1\n"
        ),
        "{body}"
    );
    assert_well_formed(&body);
}

#[tokio::test]
async fn a_category_name_with_a_quote_does_not_break_the_scrape() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // Category names are free-form strings the user types. An unescaped quote
    // would produce a line Prometheus rejects, and it discards the entire
    // response, not just that line, so one bad name blinds every dashboard.
    sqlx::query(
        r#"INSERT INTO media_routing (media_id, category, load_order, evaluated_at)
            VALUES ('m-1', 'we"ird\path', 1, datetime('now'))"#,
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let body = scrape(&app).await;

    assert!(body.contains(r#"category="we\"ird\\path""#), "not escaped:\n{body}");
    assert_well_formed(&body);
}

#[tokio::test]
async fn the_content_type_is_the_one_prometheus_expects() {
    let app = TestApp::new().await;

    let response = app.raw("/api/v1/metrics").await;

    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/plain"), "got {content_type}");
    assert!(content_type.contains("version=0.0.4"), "got {content_type}");
}

/// The date of the newest archive is a series an alert reads against the
/// interval, and the free space of the data disk another, neither of which a
/// count of failed jobs, trimmed by the retention, can stand for.
#[tokio::test]
async fn the_newest_archive_and_the_free_space_are_series() {
    let (app, _dir) = super::backup::app_with_files("metrics-backup").await;
    let before = chrono::Utc::now().timestamp();
    crate::services::backup::create(&app.state, &crate::jobs::Attribution::manual(None))
        .await
        .unwrap();

    let body = scrape(&app).await;

    let value = |name: &str| {
        let line = body
            .lines()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no {name} sample in:\n{body}"));
        line.rsplit_once(' ').unwrap().1.parse::<f64>().unwrap()
    };
    let taken = value("routarr_backup_last_success_timestamp_seconds") as i64;
    assert!((before - 1..=before + 5).contains(&taken), "{taken} against {before}");
    assert!(value("routarr_data_free_bytes") > 0.0);
    assert_well_formed(&body);
}
