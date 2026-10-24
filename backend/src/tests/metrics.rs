//! Prometheus exposition: one malformed line and the scraper discards the whole response, and
//! category names are whatever the user typed.

use super::TestApp;

async fn scrape(app: &TestApp) -> String {
    let response = app.get("/api/v1/metrics").await;
    assert!(response.status.is_success());
    // The harness leaves `json` null for a text body, so a second call reads the raw text.
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

    // A family without HELP and TYPE scrapes, but nothing downstream can say what it means.
    let names: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("# HELP "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(!names.is_empty());
    for name in &names {
        // `_total` names a counter: a gauge under it misleads a rate the first time it goes down.
        let kind = if name.ends_with("_total") { "counter" } else { "gauge" };
        assert!(body.contains(&format!("# TYPE {name} {kind}")), "{name} is not a {kind}");
        // The `process_` families carry the names every Prometheus client gives them.
        assert!(name.starts_with("routarr_") || name.starts_with("process_"), "{name}");
    }
    assert!(names.contains(&"routarr_deprecated_api_requests_total"), "no counter is emitted");
}

/// A title already in its category counts there, and only a title's latest decision counts,
/// not the applied one before it.
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

    // Each under its name and its deprecated alias, whose help says so.
    for (by_category, by_status) in [
        ("routarr_titles_routed", "routarr_decisions"),
        ("routarr_decisions_by_category", "routarr_decisions_by_status"),
    ] {
        assert!(body.contains(&format!("{by_category}{{category=\"standard\"}} 2")), "{body}");
        assert!(!body.contains(&format!("{by_category}{{category=\"anime\"}}")), "{body}");
        assert!(body.contains(&format!("{by_status}{{status=\"pending\"}} 1")), "{body}");
        assert!(!body.contains(&format!("{by_status}{{status=\"applied\"}}")), "{body}");
    }
    for old in ["routarr_decisions_by_category", "routarr_decisions_by_status"] {
        assert!(body.contains(&format!("# HELP {old} Deprecated, read routarr_")), "{old}");
    }
}

/// A restart loop or a memory leak shows in the two families every Prometheus client exposes.
#[tokio::test]
async fn the_process_says_when_it_started_and_what_memory_it_holds() {
    let app = TestApp::new().await;
    let body = scrape(&app).await;
    let value = |name: &str| {
        body.lines()
            .find_map(|line| line.strip_prefix(&format!("{name} ")))
            .map(|value| value.parse::<f64>().unwrap())
    };
    let started = value("process_start_time_seconds").expect("a start time");
    let now = chrono::Utc::now().timestamp() as f64;
    assert!(started > now - 3600.0 && started <= now, "{started}");
    if cfg!(target_os = "linux") {
        assert!(value("process_resident_memory_bytes").expect("a resident size") > 1e6);
    }
}

/// A failure a later run found settled is no move waiting for a person: the metric counts what
/// the top bar counts.
#[tokio::test]
async fn a_failure_a_later_run_settled_is_not_counted_failed() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                target_category, action, status, decided_at)
         VALUES ('d-1', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move',
                 'failed', '2026-09-01 10:00:00')",
        "INSERT INTO media_routing (media_id, category, load_order, evaluated_at)
         VALUES ('m-1', 'anime', 1, '2026-09-01 11:00:00')",
    ])
    .await;
    let counted = |body: &str| {
        let sample = |name| {
            body.lines()
                .find_map(|line| line.strip_prefix(&format!("{name}{{status=\"failed\"}} ")))
                .map_or(0, |count| count.parse::<i64>().unwrap())
        };
        let counted = sample("routarr_decisions_by_status");
        assert_eq!(sample("routarr_decisions"), counted, "the two names disagree");
        counted
    };
    let shown =
        || async { app.get("/api/v1/status").await.assert_ok()["failed_decisions"].clone() };

    assert_eq!(shown().await, 0);
    assert_eq!(counted(&scrape(&app).await), 0, "a settled failure is counted");

    app.execute(&["DELETE FROM media_routing"]).await;
    assert_eq!(shown().await, 1);
    assert_eq!(counted(&scrape(&app).await), 1, "an open failure is not counted");
}

#[tokio::test]
async fn the_library_is_reported_per_instance_and_type() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = scrape(&app).await;

    assert!(
        body.contains(
            r#"routarr_media_items{arr_instance="Radarr",arr_instance_id="inst-1",media_type="movie"} 1"#
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

    sqlx::query(
        "UPDATE instances SET last_sync_status = 'success',
                last_sync_at = '2026-10-05 21:26:14', last_sync_attempt_at = '2026-10-05 21:40:00'",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let body = scrape(&app).await;
    assert!(
        body.contains(r#"routarr_instance_up{arr_instance="Radarr",arr_instance_id="inst-1"} 1"#),
        "got:\n{body}"
    );
    // How old `up` is: a sync that stopped running leaves it at 1.
    let labels = r#"{arr_instance="Radarr",arr_instance_id="inst-1"}"#;
    assert!(
        body.contains(&format!("routarr_instance_last_sync_timestamp_seconds{labels} 1791235574")),
        "got:\n{body}"
    );
    assert!(
        body.contains(&format!(
            "routarr_instance_last_sync_attempt_timestamp_seconds{labels} 1791236400"
        )),
        "got:\n{body}"
    );
}

/// Switched off is told apart from down, and kept to proposals from armed. Each job outcome is
/// its own series, so a run of failed applies can alert.
#[tokio::test]
async fn the_instance_switches_and_each_job_outcome_are_series() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&[
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token,
                                auto_apply)
         VALUES ('inst-2', 'Sonarr', 'sonarr', 'http://127.0.0.1:1', 'secret', 0, 'tok2', 0)",
        "INSERT INTO jobs (id, kind, status) VALUES ('j-1', 'sync', 'success'),
                ('j-2', 'sync', 'success'), ('j-3', 'apply', 'failed'), ('j-4', 'apply', 'success')",
    ])
    .await;

    let body = scrape(&app).await;

    for line in [
        r#"routarr_instance_enabled{arr_instance="Radarr",arr_instance_id="inst-1"} 1"#,
        r#"routarr_instance_enabled{arr_instance="Sonarr",arr_instance_id="inst-2"} 0"#,
        r#"routarr_instance_auto_apply{arr_instance="Radarr",arr_instance_id="inst-1"} 1"#,
        r#"routarr_instance_auto_apply{arr_instance="Sonarr",arr_instance_id="inst-2"} 0"#,
        r#"routarr_jobs{kind="sync",status="success"} 2"#,
        r#"routarr_jobs{kind="apply",status="failed"} 1"#,
        r#"routarr_jobs{kind="apply",status="success"} 1"#,
    ] {
        assert!(body.lines().any(|l| l == line), "{line} missing from:\n{body}");
    }
}

/// Prometheus sets `instance` on every series to the target it scraped, so a label of that
/// name is renamed `exported_instance` on the way in.
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

    // These two answer why nothing has moved for a week, on a graph rather than a settings page.
    let body = scrape(&app).await;
    assert!(body.contains("routarr_dry_run 1"), "dry-run defaults to on:\n{body}");
    assert!(body.contains("routarr_auto_apply_enabled 0"), "got:\n{body}");

    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'global_dry_run'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    assert!(scrape(&app).await.contains("routarr_dry_run 0"));
}

/// A source down, a quota spent and titles nothing describes are watched as an instance down
/// is. A source not probed within a day has no health sample, though its probe time is said,
/// an instance's probe is not a source's, and a source without a quota reports none.
#[tokio::test]
async fn each_metadata_source_reports_its_health_cache_and_quota() {
    let sources = super::fake_sources::FakeSources::start().await;
    let app = TestApp::one_film_on(&sources, "arr,omdb,anilist,tenrai").await;
    app.execute(&[
        "INSERT INTO probe_results (subject, reachable, detail, checked_at)
         VALUES ('source:anilist', 1, NULL, datetime('now')),
                ('source:omdb', 0, 'quota spent', datetime('now')),
                ('source:tenrai', 0, NULL, datetime('now', '-2 days')),
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
    let checked = source("routarr_metadata_source_checked_timestamp_seconds", "tenrai");
    let two_days = (chrono::Utc::now() - chrono::Duration::days(2)).timestamp();
    let said: f64 = body
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{checked} ")))
        .unwrap_or_else(|| panic!("no {checked} in:\n{body}"))
        .parse()
        .unwrap();
    assert!((said - two_days as f64).abs() < 60.0, "{checked} {said}, not two days ago");
    for absent in [
        source("routarr_metadata_source_up", "tenrai"),
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

    // Unescaped, one quote in a name makes Prometheus discard the whole response.
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

/// An alert reads these against the interval and the disk, which a count of failed jobs
/// trimmed by the retention cannot stand for.
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

/// An alert reads each warning with the threshold the application applies, and a stale-sync
/// rule reads each instance's interval.
#[tokio::test]
async fn the_warnings_the_version_the_database_and_the_sync_intervals_are_series() {
    let (app, _dir) = super::backup::app_with_files("metrics-warnings").await;
    app.execute(&[crate::tests::AN_INSTANCE, "UPDATE instances SET sync_interval_minutes = 30"])
        .await;

    let body = scrape(&app).await;

    let version = env!("CARGO_PKG_VERSION");
    for line in [
        format!(r#"routarr_build_info{{version="{version}"}} 1"#),
        r#"routarr_warnings{code="instance_without_mapping",severity="warning"} 1"#.to_string(),
        concat!(
            r#"routarr_instance_sync_interval_seconds"#,
            r#"{arr_instance="Radarr",arr_instance_id="inst-1"} 1800"#
        )
        .to_string(),
    ] {
        assert!(body.lines().any(|l| l == line), "{line} missing from:\n{body}");
    }
    let database = body
        .lines()
        .find_map(|line| line.strip_prefix("routarr_database_bytes "))
        .unwrap_or_else(|| panic!("no database size in:\n{body}"));
    assert!(database.parse::<f64>().unwrap() > 0.0);
    assert_well_formed(&body);
}

/// The dashboard and the alert rules in `contrib/monitoring/` read series Routarr emits: a family
/// renamed or removed would leave a panel empty and an alert that never fires, unseen.
#[tokio::test]
async fn the_shipped_dashboard_and_alerts_read_series_routarr_emits() {
    let app = TestApp::new().await;
    let body = scrape(&app).await;
    let dashboard = include_str!("../../../contrib/monitoring/grafana-dashboard.json");
    let rules = include_str!("../../../contrib/monitoring/prometheus-alerts.yml");
    serde_json::from_str::<serde_json::Value>(dashboard).expect("the dashboard is JSON");

    let mut named = std::collections::BTreeSet::new();
    for text in [dashboard, rules] {
        for (at, _) in text.match_indices("routarr_") {
            let name: String =
                text[at..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
            named.insert(name);
        }
    }
    assert!(named.len() > 10, "{named:?}");
    for name in named {
        assert!(body.contains(&format!("# HELP {name} ")), "{name} is not emitted");
        assert!(!body.contains(&format!("# HELP {name} Deprecated")), "{name} is deprecated");
    }

    // A rule on a warning code reads a code Routarr writes, or it never fires.
    let mut codes = std::collections::BTreeSet::new();
    for text in [dashboard, rules] {
        for (at, _) in text.match_indices("code") {
            let rest = text[at + 4..].trim_start_matches(['=', '!', '~', '\\']);
            let Some(quoted) = rest.strip_prefix('"') else { continue };
            let values = quoted.split(['"', '\\']).next().unwrap_or_default();
            codes.extend(values.split('|').map(str::to_owned));
        }
    }
    assert!(codes.len() > 5, "{codes:?}");
    for code in &codes {
        let known = crate::api::health::CODES.iter().any(|(listed, _)| listed == code);
        assert!(known, "a shipped rule selects {code}, a code Routarr never writes");
    }
    assert!(rules.contains("absent(up{job=\"routarr\"})"), "no rule fires when Routarr is down");
}
