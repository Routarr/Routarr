//! Pinned expectations for the rule engine, and the runner that replays them.
//!
//! The rule preview answers "what would this change". A pinned case answers
//! "what must this *not* change". With first-match-by-priority, inserting one
//! rule rebalances every rule below it, and the routing that quietly moves is
//! always the one nobody was looking at.
//!
//! This is cheap because `rule_engine` is pure: no I/O, no clock of its own,
//! `now` handed in. A case is the two things `EvalContext` reads plus the
//! instant, so replaying a hundred of them costs one query and no requests.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};
use crate::models::media::Media;
use crate::models::metadata::MediaMetadata;
use crate::services::routing;
use crate::services::rule_engine::{self, EvalContext};

/// A stored case: inputs, and the category they are required to produce.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RuleTest {
    pub id: String,
    pub name: String,
    pub media_type: String,
    /// The pinned `EvalContext`: the serialised `Media` and the merged
    /// metadata. Read when a case is *run*, never sent to a client: the list
    /// endpoint returns every case, and the interface shows a name and a
    /// category. Skipped rather than trimmed at the handler so the struct stays
    /// the one thing that decides what a case is.
    #[serde(skip_serializing)]
    pub media_json: String,
    #[serde(skip_serializing)]
    pub metadata_json: Option<String>,
    pub evaluated_at: String,
    pub expected_category: String,
    pub source_media_title: Option<String>,
    pub created_at: String,
}

/// What one case did on this run.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleTestResult {
    pub id: String,
    pub name: String,
    pub expected_category: String,
    /// What the engine says today. `null` when the stored fixture fails to parse.
    pub actual_category: Option<String>,
    pub passed: bool,
    /// The rule that won, so a failure names what took the decision.
    pub matched_rule: Option<String>,
    /// Set when the fixture itself is the problem rather than the rules.
    pub error: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleTestRun {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<RuleTestResult>,
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<RuleTest>> {
    Ok(sqlx::query_as::<_, RuleTest>("SELECT * FROM rule_tests ORDER BY name, created_at")
        .fetch_all(pool)
        .await?)
}

/// Replay every case against the rules as they stand right now.
///
/// One load of the rules for the whole suite, and the default category read
/// once: a case that matches nothing lands where the engine would actually put
/// it, not in a `None` the caller has to interpret.
pub async fn run_all(pool: &SqlitePool) -> AppResult<RuleTestRun> {
    let cases = list(pool).await?;
    let rules = routing::load_rules(pool).await?;
    let default_category = crate::state::AppState::default_category(pool).await;

    let results: Vec<RuleTestResult> =
        cases.iter().map(|case| run_one(case, &rules, &default_category)).collect();

    let passed = results.iter().filter(|r| r.passed).count();
    Ok(RuleTestRun { total: results.len(), passed, failed: results.len() - passed, results })
}

/// The category the rules alone give one item, and the rule that won, if any.
///
/// No override: a case pins what the *rules* decide. An override is a human
/// decision about one item, and it short-circuits the engine entirely, so
/// folding it in here would let a pinned exception mask a broken rule.
pub fn decided_by_rules(
    ctx: EvalContext,
    rules: &[crate::models::rule::Rule],
    default_category: &str,
) -> (String, Option<String>) {
    match rule_engine::evaluate_rules(ctx, rules, None).winner {
        Some(winner) => (winner.category, Some(winner.rule_name)),
        None => (default_category.to_string(), None),
    }
}

fn run_one(
    case: &RuleTest,
    rules: &[crate::models::rule::Rule],
    default_category: &str,
) -> RuleTestResult {
    let fail = |error: String| RuleTestResult {
        id: case.id.clone(),
        name: case.name.clone(),
        expected_category: case.expected_category.clone(),
        actual_category: None,
        passed: false,
        matched_rule: None,
        error: Some(error),
    };

    let media: Media = match serde_json::from_str(&case.media_json) {
        Ok(media) => media,
        // A fixture that fails to parse is a failure, never a skip: a case
        // that stops running is a case that stops protecting anything.
        Err(e) => return fail(format!("the stored media snapshot no longer parses: {e}")),
    };
    let metadata: Option<MediaMetadata> = match case.metadata_json.as_deref() {
        None => None,
        Some(raw) => match serde_json::from_str(raw) {
            Ok(metadata) => Some(metadata),
            Err(e) => return fail(format!("the stored metadata snapshot no longer parses: {e}")),
        },
    };
    let Some(now) = routing::parse_timestamp(&case.evaluated_at) else {
        return fail(format!("unreadable evaluation instant: {}", case.evaluated_at));
    };

    let ctx = EvalContext { media: &media, metadata: metadata.as_ref(), now };
    let (actual, matched_rule) = decided_by_rules(ctx, rules, default_category);

    RuleTestResult {
        id: case.id.clone(),
        name: case.name.clone(),
        passed: actual == case.expected_category,
        expected_category: case.expected_category.clone(),
        actual_category: Some(actual),
        matched_rule,
        error: None,
    }
}

/// What a caller supplies to pin a case.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct NewRuleTest {
    pub name: String,
    /// The library item to snapshot. Resolved once, here, and never referenced
    /// again: a case keeps its own copy of the item, so it outlives it.
    pub media_id: String,
    /// Defaults to what the engine decides for that item today.
    pub expected_category: Option<String>,
}

/// Whether a case can be stored. The name is typed on the Tests screen, so its
/// refusal speaks the interface's language. The item is the one the screen
/// pinned, never typed, so its refusal stays English.
pub fn validate(new: &NewRuleTest, localizer: &crate::localization::Localizer) -> AppResult<()> {
    if new.name.trim().is_empty() {
        return Err(AppError::BadRequest(localizer.translate("RuleTestNameRequired", &[])));
    }
    if new.media_id.trim().is_empty() {
        return Err(AppError::BadRequest("a test needs a media item to snapshot".into()));
    }
    Ok(())
}
