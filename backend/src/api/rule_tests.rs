//! Pinned expectations: list, pin, delete, replay.
//!
//! CRUD queries from the handler, as every other resource here does; the replay
//! is in `services::rule_tests`, because evaluating rules would still be logic
//! without HTTP.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::services::enrichment;
use crate::services::routing;
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
/// what is stored is what the engine actually saw — not the raw row, which
/// carries none of the enrichment the conditions read.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<NewRuleTest>,
) -> AppResult<Json<RuleTest>> {
    rule_tests::validate(&body)?;

    let media = crate::api::media::load_media(&state, &body.media_id).await?;
    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    // The instant is pinned with the fixture: `now` is an input — it is what
    // `added_within_days` compares against — so a case borrowing the wall clock
    // would answer a different question every day and eventually fail alone.
    let evaluated_at = routing::format_timestamp(chrono::Utc::now());

    // Default to what the engine decides today, which is what makes pinning a
    // decision one click from the explanation panel.
    let expected = match body.expected_category {
        Some(category) if !category.trim().is_empty() => category,
        _ => {
            let rules = routing::load_rules(&state.pool).await?;
            let ctx = crate::services::rule_engine::EvalContext {
                media: &media,
                metadata: metadata.as_ref(),
                now: chrono::Utc::now(),
            };
            crate::services::rule_engine::evaluate_rules(ctx, &rules, None)
                .winner
                .map(|w| w.category)
                .unwrap_or(AppState::default_category(&state.pool).await)
        }
    };

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
    .execute(&state.pool)
    .await?;

    let created = sqlx::query_as::<_, RuleTest>("SELECT * FROM rule_tests WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(created))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let affected =
        sqlx::query("DELETE FROM rule_tests WHERE id = ?").bind(&id).execute(&state.pool).await?;
    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule test {id} not found")));
    }
    Ok(Json(serde_json::json!({ "deleted": id })))
}
