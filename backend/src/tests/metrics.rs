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

/// The decision gauges count where each title stands now, its latest
/// standing decision, not the applied history an apply leaves standing.
#[tokio::test]
async fn the_decision_gauges_count_each_titles_latest_decision() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.execute(&["INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                          target_category, action, status, decided_at)
                   VALUES ('d-old', 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move',
                           'applied', '2026-09-01 10:00:00'),
                          ('d-new', 'm-1', 'Totoro', 'movie', 'inst-1', 'standard', 'move',
                           'pending', '2026-09-02 10:00:00')"])
        .await;

    let body = scrape(&app).await;

    assert!(body.contains("routarr_decisions_by_category{category=\"standard\"} 1"), "{body}");
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
/// in. Two instances may also share a name, and one series per name would
/// merge them.
#[tokio::test]
async fn two_instances_of_one_name_are_two_series() {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled, webhook_token)
         VALUES ('inst-2', 'Radarr', 'radarr', 'http://radarr-4k:7878', 'secret', 1, 'tok2')",
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

#[tokio::test]
async fn a_category_name_with_a_quote_does_not_break_the_scrape() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // Category names are free-form strings the user types. An unescaped quote
    // would produce a line Prometheus rejects, and it discards the entire
    // response, not just that line, so one bad name blinds every dashboard.
    sqlx::query(
        r#"INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
            target_category, action, status, decided_at)
            VALUES ('d-1', 'm-1', 'T', 'movie', 'inst-1', 'we"ird\path', 'move', 'pending',
                    datetime('now'))"#,
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
