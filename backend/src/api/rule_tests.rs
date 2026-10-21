//! Pinned expectations: list, pin, delete, replay. The CRUD queries from the handler, and the
//! replay lives in `services::rule_tests`, since evaluating rules is logic without HTTP.

use super::Json;
use axum::extract::State;

use super::Path;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::services::enrichment;
use crate::services::routing;
use crate::services::rule_engine::EvalContext;
use crate::services::rule_tests::{self, NewRuleTest, RuleTest, RuleTestRun};
use crate::state::AppState;

/// A page of the cases, by name. The replay reads every case, the list a page.
pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    super::Query(query): super::Query<super::PageQuery>,
) -> AppResult<Json<super::Page<RuleTest>>> {
    let (page, per_page, offset) = super::paginate(query.page, query.per_page);
    let cases = sqlx::query_as::<_, RuleTest>(
        "SELECT * FROM rule_tests ORDER BY name, created_at, id LIMIT ? OFFSET ?",
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rule_tests").fetch_one(&state.pool).await?;
    let cases = cases
        .into_iter()
        .map(|case| RuleTest {
            subject: identity.shown_subject(case.subject.clone(), case.subject_key.as_deref()),
            ..case
        })
        .collect();
    Ok(Json(super::Page::new(cases, page, per_page, total)))
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

    let media = crate::api::media::load_media(&state, &body.media_id).await?;
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
            rule_tests::decided_by_rules(ctx, &rules, &default_category).0
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

    let created = sqlx::query_as::<_, RuleTest>("SELECT * FROM rule_tests WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
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
