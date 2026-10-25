//! Pinned expectations: list, pin, delete, replay. The replay is `services::rule_tests`.

use super::Json;
use axum::extract::State;

use super::Path;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::services::enrichment;
use crate::services::routing;
use crate::services::rule_engine::EvalContext;
use crate::services::rule_tests::{self, NewRuleTest, RuleTest, RuleTestExpectation, RuleTestRun};
use crate::state::AppState;
use sqlx::AssertSqlSafe;

/// Which page of the cases, and whether only those the last run found failing.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct RuleTestQuery {
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
    /// `true` lists only the tests the last run found failing.
    pub failing: Option<bool>,
}

/// A page of the cases, by name. The replay reads every case, the list a page.
pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    super::Query(query): super::Query<RuleTestQuery>,
) -> AppResult<Json<super::Page<RuleTest>>> {
    let (page, per_page, offset) = super::paginate(query.page, query.per_page);
    let narrowed = if query.failing == Some(true) { " WHERE t.last_passed = 0" } else { "" };
    let cases = sqlx::query_as::<_, RuleTest>(AssertSqlSafe(format!(
        "{}{narrowed} ORDER BY name, created_at, id LIMIT ? OFFSET ?",
        rule_tests::SELECT
    )))
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let total: i64 =
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT COUNT(*) FROM rule_tests t{narrowed}")))
            .fetch_one(&state.pool)
            .await?;
    let cases = cases
        .into_iter()
        .map(|case| RuleTest {
            subject: identity.shown_subject(case.subject.clone(), case.subject_key.as_deref()),
            ..case
        })
        .collect();
    Ok(Json(super::Page::new(cases, page, per_page, total)))
}

pub async fn get_one(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<RuleTest>> {
    Ok(Json(read_one(&state, &identity, &id).await?))
}

async fn read_one(
    state: &AppState,
    identity: &crate::api::auth::Identity,
    id: &str,
) -> AppResult<RuleTest> {
    let case = sqlx::query_as::<_, RuleTest>(AssertSqlSafe(format!(
        "{} WHERE t.id = ?",
        rule_tests::SELECT
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("Rule test {id} not found")))?;
    let subject = identity.shown_subject(case.subject.clone(), case.subject_key.as_deref());
    Ok(RuleTest { subject, ..case })
}

/// Expect another category of a case. The cases run again, so the answer carries the verdict.
pub async fn set_expected_category(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Path(id): Path<String>,
    Json(body): Json<RuleTestExpectation>,
) -> AppResult<Json<RuleTest>> {
    let localizer = state.localizer().await;
    let expected = crate::api::categories::normalise(&body.expected_category, &localizer)?;

    // Checked under the case's write lock, so its category cannot be removed in between.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let changed = sqlx::query("UPDATE rule_tests SET expected_category = ? WHERE id = ?")
        .bind(&expected)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule test {id} not found")));
    }
    crate::api::categories::ensure_exists(&mut tx, &expected, &localizer).await?;
    crate::race::checked("rule_tests::write", &expected).await;
    tx.commit().await?;

    if let Err(e) = rule_tests::check(&state).await {
        tracing::warn!("The rule tests did not run after an expectation changed: {e}");
    }
    Ok(Json(read_one(&state, &identity, &id).await?))
}

pub async fn run(State(state): State<AppState>) -> AppResult<Json<RuleTestRun>> {
    Ok(Json(rule_tests::check(&state).await?))
}

/// Snapshot a library item and pin the category it must keep producing. The metadata is
/// resolved as `/library/{id}/explain` resolves it, so the case holds the enrichment rules read.
pub async fn create(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Json(body): Json<NewRuleTest>,
) -> AppResult<super::Created<RuleTest>> {
    rule_tests::validate(&body, &state.localizer().await)?;

    let media = crate::api::library::load_media(&state, &body.media_id).await?;
    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    // Pinned with the case, since `added_within_days` reads `now` and the wall clock moves on.
    let now = chrono::Utc::now();
    let evaluated_at = routing::format_timestamp(now);

    // With none given, pin today's decision: the explanation panel's one-click pin relies on it.
    let localizer = state.localizer().await;
    let expected = match body.expected_category {
        // Normalised and existing: a case expecting `Anime ` or an unknown category can only fail.
        Some(category) if !category.trim().is_empty() => {
            crate::api::categories::normalise(&category, &localizer)?
        }
        _ => {
            let rules = routing::load_rules(&state.pool).await?;
            let ctx = EvalContext { media: &media, metadata: metadata.as_ref(), now };
            let default_category = AppState::default_category(&state.pool).await?;
            rule_tests::decided_by_rules(
                ctx,
                &crate::services::rule_engine::Prepared::new(&rules),
                &default_category,
            )
            .0
        }
    };

    // Checked under the case's write lock, so its category cannot be removed in between.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    crate::api::categories::ensure_exists(&mut tx, &expected, &localizer).await?;
    crate::race::checked("rule_tests::write", &expected).await;

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rule_tests (id, name, media_type, media_json, metadata_json, evaluated_at,
         expected_category, source_media_title, subject, subject_key)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(body.name.trim())
    .bind(media.media_type.to_string())
    .bind(serde_json::to_string(&media)?)
    .bind(metadata.as_ref().map(serde_json::to_string).transpose()?)
    .bind(&evaluated_at)
    .bind(&expected)
    .bind(&media.title)
    .bind(&identity.attribution().subject)
    .bind(&identity.attribution().key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // Its verdict at once: one pinned failing on purpose holds auto-apply from now.
    if let Err(e) = rule_tests::check(&state).await {
        tracing::warn!("The rule tests did not run after a pin: {e}");
    }

    let created = read_one(&state, &identity, &id).await?;
    Ok(super::Created::at(&state, format!("/rule-tests/{id}"), created))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<super::Deleted>> {
    let affected =
        sqlx::query("DELETE FROM rule_tests WHERE id = ?").bind(&id).execute(&state.pool).await?;
    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule test {id} not found")));
    }
    Ok(Json(super::Deleted { deleted: true }))
}
