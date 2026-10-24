//! Two writers meeting over one row: each test holds the first at its check
//! ([`crate::race::checked`]), lets the second finish, then lets the first write.
//! Whatever wins, no row names a missing category and no run stores a stale answer.

use super::{TestApp, TestResponse};
use crate::services::routing::{SimulationOptions, run_simulation};
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// A request held at a check: the test waits until it gets there, then lets
/// it go on.
pub(crate) struct Gate {
    reached: Notify,
    released: Notify,
}

impl Gate {
    /// Five seconds at most: a request refused before its gate fails its test instead of
    /// hanging it.
    async fn reached(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.reached.notified())
            .await
            .expect("the request never reached its gate");
    }

    pub(crate) fn release(&self) {
        self.released.notify_one();
    }
}

/// A check, what it is about, and the thread of the test that armed it.
type Checked = (&'static str, String, std::thread::ThreadId);

/// Keyed by the test's thread too, since each test runs its own runtime on its
/// own thread, so a test holds only its own requests whatever their subject.
static GATES: LazyLock<Mutex<HashMap<Checked, Arc<Gate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn checked_here(point: &'static str, subject: &str) -> Checked {
    (point, subject.to_string(), std::thread::current().id())
}

fn arm(point: &'static str, subject: &str) -> Arc<Gate> {
    let gate = Arc::new(Gate { reached: Notify::new(), released: Notify::new() });
    GATES.lock().expect("gates").insert(checked_here(point, subject), Arc::clone(&gate));
    gate
}

/// What [`crate::race::checked`] runs in a test: wait at an armed gate, once.
pub(crate) async fn hold(point: &'static str, subject: &str) {
    let gate = GATES.lock().expect("gates").remove(&checked_here(point, subject));
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.released.notified().await;
    }
}

/// Held, another test's request would wait on a release nobody sends.
#[tokio::test]
async fn a_gate_holds_only_the_test_that_armed_it() {
    let gate = arm("races::isolation", "");

    let elsewhere = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), hold("races::isolation", "")).await
        })
    });
    assert!(elsewhere.join().unwrap().is_ok(), "another test's request was held");

    let own = tokio::spawn(hold("races::isolation", ""));
    tokio::time::timeout(Duration::from_secs(5), gate.reached())
        .await
        .expect("the test's own request was not held");
    gate.release();
    own.await.unwrap();
}

/// Send `request` up to `point`, then drop it as a caller that leaves does: a
/// closed tab, a proxy's timeout. The work it started is still held there,
/// until the gate handed back is released.
pub(crate) async fn left_at(
    app: &TestApp,
    point: &'static str,
    request: axum::http::Request<axum::body::Body>,
) -> Arc<Gate> {
    use tower::ServiceExt;
    let gate = arm(point, "");
    let call = tokio::spawn(app.router.clone().oneshot(request));
    gate.reached().await;
    call.abort();
    let _ = call.await;
    gate
}

/// Run `first` up to its check about `subject` at `point`, then `second`,
/// then let `first` write, and hand back both answers.
async fn race(
    point: &'static str,
    subject: &str,
    first: impl Future<Output = TestResponse> + Send + 'static,
    second: impl Future<Output = TestResponse> + Send + 'static,
) -> (TestResponse, TestResponse) {
    let gate = arm(point, subject);
    let first = tokio::spawn(first);
    gate.reached().await;
    let mut second = tokio::spawn(second);
    // Long enough for an unheld second writer to finish between the first's check and write.
    let early = tokio::time::timeout(Duration::from_millis(300), &mut second).await;
    gate.release();
    let first = first.await.expect("first writer");
    let second = match early {
        Ok(done) => done.expect("second writer"),
        Err(_) => second.await.expect("second writer"),
    };
    (first, second)
}

/// A library with the category `name`, used by nothing yet.
async fn with_category(name: &str) -> Arc<TestApp> {
    let app = TestApp::new().await;
    app.seed_library().await;
    sqlx::query("INSERT INTO categories (id, name) VALUES (?, ?)")
        .bind(format!("cat-{name}"))
        .bind(name)
        .execute(&app.state.pool)
        .await
        .unwrap();
    Arc::new(app)
}

/// Rows naming a category no row of `categories` holds.
async fn dangling(app: &TestApp) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM rules
                  WHERE target_category NOT IN (SELECT name FROM categories))
              + (SELECT COUNT(*) FROM root_folders
                  WHERE category IS NOT NULL AND category NOT IN (SELECT name FROM categories))
              + (SELECT COUNT(*) FROM overrides
                  WHERE target_category NOT IN (SELECT name FROM categories))
              + (SELECT COUNT(*) FROM rule_tests
                  WHERE expected_category IS NOT NULL
                    AND expected_category NOT IN (SELECT name FROM categories))
              + (SELECT COUNT(*) FROM settings
                  WHERE key = 'default_category' AND value NOT IN (SELECT name FROM categories))",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
}

fn rule_to(category: &str) -> serde_json::Value {
    json!({
        "name": format!("To {category}"),
        "media_type": "both",
        "target_category": category,
        "conditions": [{ "type": "genre_contains", "value": ["Animation"] }]
    })
}

fn removing(app: &Arc<TestApp>, name: &str) -> impl Future<Output = TestResponse> + 'static {
    let (app, path) = (Arc::clone(app), format!("/api/v1/categories/cat-{name}"));
    async move { app.delete(&path).await }
}

fn posting(
    app: &Arc<TestApp>,
    path: &'static str,
    body: serde_json::Value,
) -> impl Future<Output = TestResponse> + 'static {
    let app = Arc::clone(app);
    async move { app.post(path, body).await }
}

fn putting(
    app: &Arc<TestApp>,
    path: String,
    body: serde_json::Value,
) -> impl Future<Output = TestResponse> + 'static {
    let app = Arc::clone(app);
    async move { app.put(&path, body).await }
}

#[tokio::test]
async fn a_category_removed_while_a_template_is_added_leaves_no_orphan() {
    let app = with_category("doomed-template").await;
    let adding = posting(
        &app,
        "/api/v1/rules/templates/documentaries/import",
        json!({ "category": "doomed-template" }),
    );

    let (added, removed) =
        race("templates::import", "documentaries", adding, removing(&app, "doomed-template")).await;

    assert!(added.status.is_success(), "{}", added.json);
    assert!(removed.status.is_success(), "{}", removed.json);
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_category_removed_while_a_rule_is_written_against_it_leaves_no_orphan() {
    let app = with_category("doomed-remove").await;
    race(
        "categories::remove",
        "doomed-remove",
        removing(&app, "doomed-remove"),
        posting(&app, "/api/v1/rules", rule_to("doomed-remove")),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_rule_written_while_its_category_is_removed_leaves_no_orphan() {
    let app = with_category("doomed-rule").await;
    race(
        "rules::write",
        "doomed-rule",
        posting(&app, "/api/v1/rules", rule_to("doomed-rule")),
        removing(&app, "doomed-rule"),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_pin_made_while_its_category_is_removed_leaves_no_orphan() {
    let app = with_category("doomed-pin").await;
    let pin = json!({ "media_id": "m-1", "target_category": "doomed-pin" });
    race(
        "exceptions::pin",
        "doomed-pin",
        posting(&app, "/api/v1/exceptions", pin),
        removing(&app, "doomed-pin"),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_mapping_made_while_its_category_is_removed_leaves_no_orphan() {
    let app = with_category("doomed-map").await;
    race(
        "root_folders::map",
        "doomed-map",
        putting(
            &app,
            "/api/v1/root-folders/rf-1/category".into(),
            json!({ "category": "doomed-map" }),
        ),
        removing(&app, "doomed-map"),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_default_set_while_its_category_is_removed_leaves_no_orphan() {
    let app = with_category("doomed-default").await;
    race(
        "settings::default_category",
        "doomed-default",
        putting(
            &app,
            "/api/v1/settings".into(),
            json!({ "settings": { "default_category": "doomed-default" } }),
        ),
        removing(&app, "doomed-default"),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_rule_test_pinned_while_its_category_is_removed_leaves_no_orphan() {
    let app = with_category("doomed-case").await;
    let case = json!({ "name": "A case", "media_id": "m-1", "expected_category": "doomed-case" });
    race(
        "rule_tests::write",
        "doomed-case",
        posting(&app, "/api/v1/rule-tests", case),
        removing(&app, "doomed-case"),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

#[tokio::test]
async fn a_rule_written_while_its_category_is_renamed_follows_the_rename() {
    let app = with_category("doomed-rename").await;
    race(
        "rules::write",
        "doomed-rename",
        posting(&app, "/api/v1/rules", rule_to("doomed-rename")),
        putting(&app, "/api/v1/categories/cat-doomed-rename".into(), json!({ "name": "reborn" })),
    )
    .await;
    assert_eq!(dangling(&app).await, 0);
}

/// The keep reads the category its folder leads to, then pins the title to it: a rename landing
/// between the two would leave the exception on a name that no longer exists.
#[tokio::test]
async fn a_keep_made_while_its_category_is_renamed_follows_the_rename() {
    let app = with_category("doomed-keep").await;
    let mapped = json!({ "category": "doomed-keep" });
    app.put("/api/v1/root-folders/rf-1/category", mapped).await.assert_ok();
    let (kept, renamed) = race(
        "exceptions::keep",
        "m-1",
        posting(&app, "/api/v1/library/m-1/keep", json!({})),
        putting(&app, "/api/v1/categories/cat-doomed-keep".into(), json!({ "name": "reborn" })),
    )
    .await;
    assert!(kept.status.is_success(), "{}", kept.json);
    assert!(renamed.status.is_success(), "{}", renamed.json);
    assert_eq!(
        app.count("SELECT COUNT(*) FROM overrides WHERE target_category = 'reborn'").await,
        1
    );
    assert_eq!(dangling(&app).await, 0);
}

/// A library-wide run of the anime rule over Totoro, which it moves, held
/// before it stores, and the app it runs on. `trigger` names the run, and
/// its gate, apart from those of the tests running beside it.
async fn held_sweep(
    trigger: &'static str,
) -> (Arc<TestApp>, Arc<Gate>, tokio::task::JoinHandle<()>) {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let app = Arc::new(app);
    let gate = arm("routing::store", trigger);
    let sweep = {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            run_simulation(
                &app.state.pool,
                &app.state.library_passes,
                SimulationOptions { trigger: trigger.into(), persist: true, ..Default::default() },
            )
            .await
            .expect("the sweep");
        })
    };
    gate.reached().await;
    (app, gate, sweep)
}

/// Totoro's standing proposals.
async fn proposals_for_totoro(app: &TestApp) -> i64 {
    app.count(
        "SELECT COUNT(*) FROM decisions
          WHERE media_id = 'm-1' AND status = 'pending' AND superseded = 0",
    )
    .await
}

/// Nothing would retire such a proposal, and the dashboard would count it.
#[tokio::test]
async fn a_run_proposes_nothing_for_a_title_deleted_after_it_read_the_library() {
    let (app, gate, sweep) = held_sweep("sweep-before-a-delete").await;
    app.execute(&["DELETE FROM media WHERE id = 'm-1'"]).await;
    gate.release();
    sweep.await.unwrap();

    assert_eq!(proposals_for_totoro(&app).await, 0);
}

/// A run that read the library later decided Totoro first, and its answer
/// stands: the earlier run's move rests on a rule switched off since.
#[tokio::test]
async fn a_run_leaves_alone_a_title_a_later_run_decided() {
    let (app, gate, sweep) = held_sweep("sweep-before-a-later-run").await;
    app.execute(&["UPDATE rules SET enabled = 0"]).await;
    run_simulation(
        &app.state.pool,
        &app.state.library_passes,
        SimulationOptions {
            media_ids: Some(vec!["m-1".into()]),
            persist: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    gate.release();
    sweep.await.unwrap();

    assert_eq!(proposals_for_totoro(&app).await, 0, "the earlier run's move was stored");
    let category: String =
        sqlx::query_scalar("SELECT category FROM media_routing WHERE media_id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(category, "standard");
}

/// A row a clock that ran ahead stored, or a restored archive brought, orders after a run
/// started now unless the run reads the floor.
#[tokio::test]
async fn a_run_after_the_clock_went_back_stores_what_it_decided() {
    let app = TestApp::new().await;
    app.seed_library().await;
    app.seed_anime_rule().await;
    let ahead = chrono::Utc::now().timestamp_micros() + 86_400_000_000;
    sqlx::query(
        "INSERT INTO media_routing (media_id, category, load_order, evaluated_at)
         VALUES ('m-1', 'standard', ?, datetime('now'))",
    )
    .bind(ahead)
    .execute(&app.state.pool)
    .await
    .unwrap();

    run_simulation(
        &app.state.pool,
        &app.state.library_passes,
        SimulationOptions { persist: true, ..Default::default() },
    )
    .await
    .unwrap();

    assert_eq!(proposals_for_totoro(&app).await, 1, "the run stored no move for Totoro");
    let (category, order): (String, i64) =
        sqlx::query_as("SELECT category, load_order FROM media_routing WHERE media_id = 'm-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(category, "anime");
    assert!(order > ahead, "the run took an order below the one stored");
}

/// A pass after a sync and a webhook's unattended pass each run the rule tests: the second to
/// read waits for the first's verdicts, so the suite turning to failing is told once.
#[tokio::test]
async fn two_rule_test_runs_announce_a_failure_once() {
    let receiver = crate::tests::notify::Receiver::start().await;
    let app = TestApp::new().await;
    app.seed_library().await;
    app.store_setting("notification_webhook_url", &receiver.url).await;
    let body = json!({ "name": "Totoro stays", "media_id": "m-1" });
    app.post("/api/v1/rule-tests", body).await.assert_ok();
    app.seed_anime_rule().await;
    let app = Arc::new(app);
    let check = || {
        let app = Arc::clone(&app);
        async move { crate::services::rule_tests::check(&app.state).await.unwrap().failed }
    };

    let gate = arm("rule_tests::check", "");
    let first = tokio::spawn(check());
    gate.reached().await;
    let mut second = tokio::spawn(check());
    let early = tokio::time::timeout(Duration::from_millis(300), &mut second).await;
    gate.release();
    assert_eq!(first.await.unwrap(), 1);
    let second = match early {
        Ok(done) => done.unwrap(),
        Err(_) => second.await.unwrap(),
    };
    assert_eq!(second, 1);

    receiver.awaiting(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let told: Vec<String> = receiver.deliveries().into_iter().map(|d| d.body).collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("rule_tests_failing"), "{told:?}");
}

/// The reorder checks its list under the lock it writes with.
#[tokio::test]
async fn a_rule_created_while_the_rules_are_reordered_waits_for_the_new_order() {
    let app = with_category("reordered").await;
    let first = app.post("/api/v1/rules", rule_to("reordered")).await.assert_ok()["id"].clone();
    let gate = arm("rules::reorder", "");
    let reorder =
        tokio::spawn(posting(&app, "/api/v1/rules/reorder", json!({ "rule_ids": [first] })));
    gate.reached().await;

    let mut creating = tokio::spawn(posting(&app, "/api/v1/rules", rule_to("reordered")));
    let early = tokio::time::timeout(Duration::from_millis(300), &mut creating).await;
    gate.release();

    assert!(early.is_err(), "a rule was written between the reorder's check and its write");
    reorder.await.unwrap().assert_ok();
    creating.await.unwrap().assert_ok();
}

/// A pass reads the rules, the mappings and the titles at one moment.
#[tokio::test]
async fn a_category_renamed_while_a_pass_reads_reaches_it_whole() {
    let (app, _dir) = crate::tests::backup::app_with_files("race-snapshot").await;
    app.seed_library().await;
    app.execute(&["INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
                                      target_category, match_mode)
                   VALUES ('rule-read-at-one-moment', 'Anime', 10, 1, 'both',
                           '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]',
                           'anime', 'all')"])
        .await;
    let app = Arc::new(app);
    let gate = arm("routing::load", "rule-read-at-one-moment");
    let pass = {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            run_simulation(&app.state.pool, &app.state.library_passes, SimulationOptions::default())
                .await
                .unwrap()
        })
    };
    gate.reached().await;

    putting(&app, "/api/v1/categories/cat-anime".into(), json!({ "name": "animation" }))
        .await
        .assert_ok();
    gate.release();

    let result = pass.await.unwrap();
    let actions: Vec<&str> = result.decisions.iter().map(|d| d.action.as_str()).collect();
    assert_eq!(actions, ["move"], "{:?}", result.decisions);
}

/// A folder the operator declared and a sync promoted to the Arr's own while
/// its delete was asked is the Arr's: the delete leaves it, and its category.
#[tokio::test]
async fn a_declared_folder_promoted_while_it_is_deleted_is_kept() {
    let app = Arc::new(TestApp::new().await);
    app.seed_library().await;
    app.execute(&["INSERT INTO root_folders (id, instance_id, path, origin, category)
                   VALUES ('rf-declared', 'inst-1', '/movies/kids', 'declared', 'anime')"])
        .await;
    let gate = arm("root_folders::delete", "rf-declared");
    let deleting = {
        let app = Arc::clone(&app);
        tokio::spawn(async move { app.delete("/api/v1/root-folders/rf-declared").await })
    };
    gate.reached().await;
    app.execute(&["UPDATE root_folders SET origin = 'arr', arr_id = 9 WHERE id = 'rf-declared'"])
        .await;
    gate.release();

    deleting.await.unwrap().assert_status(axum::http::StatusCode::CONFLICT);
    let kept: i64 = app.count("SELECT COUNT(*) FROM root_folders WHERE id = 'rf-declared'").await;
    assert_eq!(kept, 1, "the Arr's folder was deleted with its category");
}

/// An instance removed while a folder is declared on it, or a title removed
/// while it is pinned, answers that it is not found, not a server error.
#[tokio::test]
async fn what_goes_while_it_is_written_to_is_not_found() {
    for (point, subject, path, body, removal) in [
        (
            "root_folders::declare",
            "inst-1",
            "/api/v1/root-folders",
            json!({ "instance_id": "inst-1", "path": "/movies/new" }),
            "DELETE FROM instances WHERE id = 'inst-1'",
        ),
        (
            "exceptions::create",
            "m-1",
            "/api/v1/exceptions",
            json!({ "media_id": "m-1", "target_category": "anime" }),
            "DELETE FROM media WHERE id = 'm-1'",
        ),
    ] {
        let app = Arc::new(TestApp::new().await);
        app.seed_library().await;
        let gate = arm(point, subject);
        let writing = tokio::spawn(posting(&app, path, body));
        gate.reached().await;
        sqlx::query(removal).execute(&app.state.pool).await.unwrap();
        gate.release();

        writing.await.unwrap().assert_status(axum::http::StatusCode::NOT_FOUND);
    }
}
