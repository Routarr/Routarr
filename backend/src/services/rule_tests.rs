//! Pinned expectations for the rule engine, and the runner that replays them. A pinned case
//! guards a routing nobody is watching, since a new rule rebalances every rule below it. The
//! engine is pure, so replaying a hundred cases costs a few queries and no request.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};
use crate::localization::Localizer;
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
    /// The pinned `EvalContext`: the serialised `Media` and the merged metadata, read when a
    /// case runs and never sent.
    #[serde(skip_serializing)]
    pub media_json: String,
    #[serde(skip_serializing)]
    pub metadata_json: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub evaluated_at: String,
    pub expected_category: String,
    pub source_media_title: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
    /// Whether the last run found the case passing. Null until a run reads it.
    pub last_passed: Option<bool>,
    /// When the case last ran.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_checked_at: Option<String>,
    /// Who pinned it, as `Decision.subject` reads.
    pub subject: Option<String>,
    #[serde(skip)]
    pub subject_key: Option<String>,
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
    /// Set when the stored case itself is the problem rather than the rules,
    /// in the interface's language.
    pub error: Option<String>,
    /// What `error` says, for a program: `RuleTestSnapshotUnreadable` or
    /// `RuleTestInstantUnreadable`. The list may grow.
    pub error_key: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleTestRun {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<RuleTestResult>,
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<RuleTest>> {
    Ok(sqlx::query_as::<_, RuleTest>("SELECT * FROM rule_tests ORDER BY name, created_at, id")
        .fetch_all(pool)
        .await?)
}

/// Replay every case against the rules as they stand now. A case that matches nothing gets
/// the default category, where the engine would put it.
pub async fn run_all(pool: &SqlitePool, localizer: &Localizer) -> AppResult<RuleTestRun> {
    let cases = list(pool).await?;
    let rules = routing::load_rules(pool).await?;
    let default_category = crate::state::AppState::default_category(pool).await?;

    let rules = rule_engine::Prepared::new(&rules);
    let results: Vec<RuleTestResult> =
        cases.iter().map(|case| run_one(case, &rules, &default_category, localizer)).collect();

    let passed = results.iter().filter(|r| r.passed).count();
    Ok(RuleTestRun { total: results.len(), passed, failed: results.len() - passed, results })
}

/// Run every case, keep each verdict, and say once when the suite starts failing. Every run
/// goes through here: Run, a pin, the pass after a sync and the auto-apply check.
pub async fn check(state: &crate::state::AppState) -> AppResult<RuleTestRun> {
    let run = run_all(&state.pool, &state.localizer().await).await?;
    let checked_at = routing::format_timestamp(chrono::Utc::now());
    // Read under the lock the verdicts are written with, or two runs both find the suite passing.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let (was_failing, _) = failing(&mut *tx).await?;
    crate::race::checked("rule_tests::check", "").await;
    for result in &run.results {
        sqlx::query("UPDATE rule_tests SET last_passed = ?, last_checked_at = ? WHERE id = ?")
            .bind(result.passed)
            .bind(&checked_at)
            .bind(&result.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if was_failing == 0 && run.failed > 0 {
        crate::services::notify::send_later(
            state,
            crate::services::notify::Event::RuleTestsFailing {
                failed: run.failed,
                total: run.total,
            },
        );
    }
    Ok(run)
}

/// The cases the last runs found failing, and every case.
pub async fn failing<'e, E>(executor: E) -> AppResult<(i64, i64)>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_as("SELECT COALESCE(SUM(last_passed = 0), 0), COUNT(*) FROM rule_tests")
        .fetch_one(executor)
        .await?)
}

/// The category the rules alone give one item, and the rule that won, if any. No override, or
/// a pinned exception would mask a broken rule.
pub fn decided_by_rules(
    ctx: EvalContext,
    rules: &rule_engine::Prepared<'_>,
    default_category: &str,
) -> (String, Option<String>) {
    match rule_engine::evaluate_prepared(ctx, rules, None).winner {
        Some(winner) => (winner.category, Some(winner.rule_name)),
        None => (default_category.to_string(), None),
    }
}

fn run_one(
    case: &RuleTest,
    rules: &rule_engine::Prepared<'_>,
    default_category: &str,
    localizer: &Localizer,
) -> RuleTestResult {
    // The detail goes to the log: a parser's message means nothing on the screen.
    let fail = |key: &str, detail: &dyn std::fmt::Display| {
        tracing::warn!(case = %case.id, "The rule test '{}' cannot be replayed: {detail}", case.name);
        RuleTestResult {
            id: case.id.clone(),
            name: case.name.clone(),
            expected_category: case.expected_category.clone(),
            actual_category: None,
            passed: false,
            matched_rule: None,
            error: Some(localizer.translate(key, &[])),
            error_key: Some(key.to_string()),
        }
    };

    let media: Media = match serde_json::from_str(&case.media_json) {
        Ok(media) => media,
        // A failure, never a skip: a case that stops running stops protecting anything.
        Err(e) => return fail("RuleTestSnapshotUnreadable", &e),
    };
    let metadata: Option<MediaMetadata> = match case.metadata_json.as_deref() {
        None => None,
        Some(raw) => match serde_json::from_str(raw) {
            Ok(metadata) => Some(metadata),
            Err(e) => return fail("RuleTestSnapshotUnreadable", &e),
        },
    };
    let Some(now) = routing::parse_timestamp(&case.evaluated_at) else {
        return fail("RuleTestInstantUnreadable", &case.evaluated_at);
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
        error_key: None,
    }
}

/// What a caller supplies to pin a case.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewRuleTest {
    pub name: String,
    /// The library item to snapshot. The case keeps its own copy, so it outlives the item.
    pub media_id: String,
    /// Defaults to what the engine decides for that item today.
    pub expected_category: Option<String>,
}

/// Whether a case can be stored. Only the name is typed, so only its refusals are translated.
pub fn validate(new: &NewRuleTest, localizer: &crate::localization::Localizer) -> AppResult<()> {
    if new.name.trim().is_empty() {
        return Err(AppError::BadRequest(localizer.translate("RuleTestNameRequired", &[])));
    }
    let max = crate::services::rule_engine::MAX_NAME_LENGTH;
    if new.name.chars().count() > max {
        return Err(AppError::BadRequest(
            localizer.translate("RuleTestNameTooLong", &[("max", &max.to_string())]),
        ));
    }
    if new.media_id.trim().is_empty() {
        return Err(AppError::BadRequest("a test needs a media item to snapshot".into()));
    }
    Ok(())
}
