//! Two writers meeting over one category.
//!
//! A category is a name other rows hold, with no foreign key: a rule, a
//! folder mapping, a pin, a rule test and the default setting. Each writer
//! checks the name before it writes, and another writer may change the name
//! in between. Each test holds the first writer at its check
//! ([`crate::race::checked`]), sends the second, gives it the moment it needs
//! to finish if nothing stops it, then lets the first write. Whatever wins,
//! no row may name a category that does not exist.

use super::{TestApp, TestResponse};
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
