//! Manual overrides — the human veto over the rule engine.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::state::AppState;

type OverrideRow = (String, String, String, Option<String>, bool, String, String, String, String);

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<OverrideWithMedia>>> {
    let rows: Vec<OverrideRow> = sqlx::query_as(
        "SELECT o.id, o.media_id, o.target_category, o.reason, o.locked, o.created_at,
         m.title, m.media_type, i.name
         FROM overrides o
         JOIN media m ON o.media_id = m.id
         JOIN instances i ON m.instance_id = i.id
         ORDER BY o.created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| OverrideWithMedia {
                override_entry: OverrideEntry {
                    id: r.0,
                    media_id: r.1,
                    target_category: r.2,
                    reason: r.3,
                    locked: r.4,
                    created_at: r.5,
                },
                media_title: r.6,
                media_type: r.7,
                instance_name: r.8,
            })
            .collect(),
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateOverrideRequest>,
) -> AppResult<Json<OverrideEntry>> {
    let category = req.target_category.trim().to_lowercase();

    let media_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media WHERE id = ?)")
        .bind(&req.media_id)
        .fetch_one(&state.pool)
        .await?;
    if !media_exists {
        return Err(AppError::NotFound(format!("Media {} not found", req.media_id)));
    }

    let category_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = ?)")
            .bind(&category)
            .fetch_one(&state.pool)
            .await?;
    if !category_exists {
        return Err(AppError::BadRequest(format!("Category '{category}' does not exist")));
    }

    sqlx::query(
        "INSERT INTO overrides (id, media_id, target_category, reason, locked)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(media_id) DO UPDATE SET
            target_category = excluded.target_category,
            reason = excluded.reason,
            locked = excluded.locked",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&req.media_id)
    .bind(&category)
    .bind(&req.reason)
    .bind(req.locked)
    .execute(&state.pool)
    .await?;

    // Any pending proposal predates this override and is now wrong.
    sqlx::query("UPDATE decisions SET superseded = 1 WHERE media_id = ? AND status = 'pending'")
        .bind(&req.media_id)
        .execute(&state.pool)
        .await?;

    // Read back so the response carries the row that actually exists — on an
    // upsert the stored id is the original one, not the one just generated.
    let row: OverrideRow = sqlx::query_as(
        "SELECT o.id, o.media_id, o.target_category, o.reason, o.locked, o.created_at,
         m.title, m.media_type, i.name
         FROM overrides o
         JOIN media m ON o.media_id = m.id
         JOIN instances i ON m.instance_id = i.id
         WHERE o.media_id = ?",
    )
    .bind(&req.media_id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(OverrideEntry {
        id: row.0,
        media_id: row.1,
        target_category: row.2,
        reason: row.3,
        locked: row.4,
        created_at: row.5,
    }))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let media_id: Option<String> =
        sqlx::query_scalar("SELECT media_id FROM overrides WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;

    let Some(media_id) = media_id else {
        return Err(AppError::NotFound(format!("Override {id} not found")));
    };

    sqlx::query("DELETE FROM overrides WHERE id = ?").bind(&id).execute(&state.pool).await?;

    sqlx::query("UPDATE decisions SET superseded = 1 WHERE media_id = ? AND status = 'pending'")
        .bind(&media_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(serde_json::json!({ "deleted": true })))
}
