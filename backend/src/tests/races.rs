//! Two writers meeting over one row.
//!
//! A category is a name other rows hold, with no foreign key: a rule, a
//! folder mapping, a pin, a rule test and the default setting. Each writer
//! checks the name before it writes, and another writer may change the name
//! in between. Each test holds the first writer at its check
//! ([`crate::race::checked`]), sends the second, gives it the moment it needs
//! to finish if nothing stops it, then lets the first write. Whatever wins,
//! no row may name a category that does not exist.
//!
//! A run stores what it decided about a library it read earlier, and a sync
//! or another run may change a title in between.

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
    async fn reached(&self) {
        self.reached.notified().await;
    }

    fn release(&self) {
        self.released.notify_one();
    }
}

/// A check and the category it is about.
type Checked = (&'static str, String);

/// Keyed by the check and the category, so a test holds its own request and
/// none of those the other tests send meanwhile.
static GATES: LazyLock<Mutex<HashMap<Checked, Arc<Gate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn arm(point: &'static str, subject: &str) -> Arc<Gate> {
    let gate = Arc::new(Gate { reached: Notify::new(), released: Notify::new() });
    GATES.lock().expect("gates").insert((point, subject.to_string()), Arc::clone(&gate));
    gate
}

/// What [`crate::race::checked`] runs in a test: wait at an armed gate, once.
pub(crate) async fn hold(point: &'static str, subject: &str) {
    let gate = GATES.lock().expect("gates").remove(&(point, subject.to_string()));
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.released.notified().await;
    }
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
    // Long enough for the second writer to finish when nothing holds it: it
    // then writes between the first one's check and its write.
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
        "overrides::pin",
        "doomed-pin",
        posting(&app, "/api/v1/overrides", pin),
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

/// A title the Arr deleted after the run read the library gains no proposal:
/// nothing would retire it, and the dashboard would count it.
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

/// A reorder checks it was given every rule under the write lock it writes
/// with: a rule created meanwhile waits for the new order, rather than land
/// between the list the reorder checked and the priorities it writes.
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

/// A pass reads the rules, the mappings and the titles at one moment: a
/// category renamed while it reads, rules and mappings together, would
/// otherwise reach it half renamed, and every title bound for that category
/// would read as having no folder.
#[tokio::test]
async fn a_category_renamed_while_a_pass_reads_reaches_it_whole() {
    let (app, _dir) = crate::tests::backup::app_with_files("snapshot").await;
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
            run_simulation(&app.state.pool, SimulationOptions::default()).await.unwrap()
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
