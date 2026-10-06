//! Pinned expectations: list, pin, delete, replay.
//!
//! CRUD queries from the handler, as every other resource here does. The
//! replay is in `services::rule_tests`, because evaluating rules would still be
//! logic without HTTP.

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

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<RuleTest>>> {
    Ok(Json(rule_tests::list(&state.pool).await?))
}

pub async fn run(State(state): State<AppState>) -> AppResult<Json<RuleTestRun>> {
    Ok(Json(rule_tests::run_all(&state.pool).await?))
}

/// Snapshot a library item and pin the category it must keep producing.
///
/// The metadata is resolved the same way `/media/{id}/explain` resolves it, so
/// what is stored is what the engine actually saw, not the raw row, which
/// carries none of the enrichment the conditions read.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<NewRuleTest>,
) -> AppResult<Json<RuleTest>> {
    rule_tests::validate(&body, &state.localizer().await)?;

    let media = crate::api::media::load_media(&state, &body.media_id).await?;
    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    // The instant is pinned with the fixture: `now` is an input (it is what
    // `added_within_days` compares against), so a case borrowing the wall clock
    // would answer a different question every day and eventually fail alone.
    let now = chrono::Utc::now();
    let evaluated_at = routing::format_timestamp(now);

    // Default to what the engine decides today, which is what makes pinning a
    // decision one click from the explanation panel.
    let localizer = state.localizer().await;
    let expected = match body.expected_category {
        // A category name as every writer of one stores it, and one that
        // exists: a case expecting `Anime ` or a category nobody has can only
        // ever fail.
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

    // Checked under the write lock the case is written with, so the category
    // it expects cannot be removed in between.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    crate::api::categories::ensure_exists(&mut tx, &expected, &localizer).await?;
    crate::race::checked("rule_tests::write", &expected).await;

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rule_tests (id, name, media_type, media_json, metadata_json, evaluated_at,
         expected_category, source_media_title)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(body.name.trim())
    .bind(media.media_type.to_string())
    .bind(serde_json::to_string(&media)?)
    .bind(metadata.as_ref().map(serde_json::to_string).transpose()?)
    .bind(&evaluated_at)
    .bind(&expected)
    .bind(&media.title)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let created = sqlx::query_as::<_, RuleTest>("SELECT * FROM rule_tests WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(created))
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
