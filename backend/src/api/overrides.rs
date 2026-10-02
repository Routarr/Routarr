//! Manual overrides: the human veto over the rule engine.

use super::{Json, Query};
use axum::extract::State;

use super::Path;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::media::ExternalTitle;
use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::state::AppState;

type OverrideRow =
    (String, String, String, Option<String>, String, Option<String>, String, String, String);

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
) -> AppResult<Json<Vec<OverrideWithMedia>>> {
    let rows: Vec<OverrideRow> = sqlx::query_as(
        "SELECT o.id, o.media_id, o.target_category, o.reason, o.created_at, o.subject,
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
                    created_at: r.4,
                    subject: identity.shown_subject(r.5),
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
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Json(req): Json<CreateOverrideRequest>,
) -> AppResult<Json<OverrideEntry>> {
    let media_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media WHERE id = ?)")
        .bind(&req.media_id)
        .fetch_one(&state.pool)
        .await?;
    if !media_exists {
        return Err(AppError::NotFound(format!("Media {} not found", req.media_id)));
    }

    let pinned = pin(
        &state,
        std::slice::from_ref(&req.media_id),
        &req.target_category,
        req.reason.as_deref(),
        identity.actor(),
    )
    .await?;
    pinned
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Internal("a pin went unwritten".into()))
        .map(Json)
}

/// The category to pin a title to, and why.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PinRequest {
    /// The name of an existing category.
    pub target_category: String,
    pub reason: Option<String>,
}

/// Pin every copy of a title another service names, or the one on `instance`.
pub async fn pin_external(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Query(title): Query<ExternalTitle>,
    Json(req): Json<PinRequest>,
) -> AppResult<Json<Vec<OverrideEntry>>> {
    let copies = copies_of(&state, &title).await?;
    let pinned =
        pin(&state, &copies, &req.target_category, req.reason.as_deref(), identity.actor()).await?;
    Ok(Json(pinned))
}

/// Remove the pins on every copy of a title another service names, or on the
/// one on `instance`. A title with none answers `deleted: false`.
pub async fn unpin_external(
    State(state): State<AppState>,
    Query(title): Query<ExternalTitle>,
) -> AppResult<Json<Deleted>> {
    let copies = copies_of(&state, &title).await?;
    let mut tx = state.pool.begin().await?;
    // Only a copy whose pin went loses its proposals: a copy that had none
    // keeps the ones somebody may be reviewing.
    let mut unpinned = Vec::new();
    for media_id in &copies {
        let removed = sqlx::query("DELETE FROM overrides WHERE media_id = ?")
            .bind(media_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if removed > 0 {
            unpinned.push(media_id.as_str());
        }
    }
    crate::services::routing::supersede_pending(&mut tx, &unpinned).await?;
    tx.commit().await?;
    Ok(Json(Deleted { deleted: !unpinned.is_empty() }))
}

/// The library rows of the title `title` names.
async fn copies_of(state: &AppState, title: &ExternalTitle) -> AppResult<Vec<String>> {
    let (media_type, id) = title.named()?;
    let instance = title.instance.as_deref();
    let copies =
        crate::services::routing::media_by_external_id(&state.pool, media_type, &id, instance)
            .await?;
    if copies.is_empty() {
        return Err(AppError::NotFound(format!(
            "No {media_type} in the library has {} {}.",
            id.column(),
            id.value()
        )));
    }
    Ok(copies.into_iter().map(|media| media.id).collect())
}

/// Pin each title to `category`, replacing the pin it has, and withdraw the
/// pending proposals of each whose category changed, which the pin now
/// decides. A pin repeating a title's category keeps what it produced.
async fn pin(
    state: &AppState,
    media_ids: &[String],
    category: &str,
    reason: Option<&str>,
    subject: Option<&str>,
) -> AppResult<Vec<OverrideEntry>> {
    let category = category.trim().to_lowercase();
    let category_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = ?)")
            .bind(&category)
            .fetch_one(&state.pool)
            .await?;
    if !category_exists {
        return Err(AppError::BadRequest(format!("Category '{category}' does not exist")));
    }

    let mut tx = state.pool.begin().await?;
    let mut changed = Vec::new();
    for media_id in media_ids {
        let held: Option<String> =
            sqlx::query_scalar("SELECT target_category FROM overrides WHERE media_id = ?")
                .bind(media_id)
                .fetch_optional(&mut *tx)
                .await?;
        if held.as_deref() != Some(category.as_str()) {
            changed.push(media_id.as_str());
        }
        sqlx::query(
            "INSERT INTO overrides (id, media_id, target_category, reason, subject)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(media_id) DO UPDATE SET
                target_category = excluded.target_category,
                reason = excluded.reason,
                subject = excluded.subject",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(media_id)
        .bind(&category)
        .bind(reason)
        .bind(subject)
        .execute(&mut *tx)
        .await?;
    }
    crate::services::routing::supersede_pending(&mut tx, &changed).await?;

    // Read back so the answer carries the rows that exist: on an upsert the
    // stored id is the original one, not the one just generated. Inside the
    // transaction, so a pin made meanwhile is not read as this one.
    let mut pinned = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let row: (String, String, String, Option<String>, String, Option<String>) = sqlx::query_as(
            "SELECT id, media_id, target_category, reason, created_at, subject
                 FROM overrides WHERE media_id = ?",
        )
        .bind(media_id)
        .fetch_one(&mut *tx)
        .await?;
        pinned.push(OverrideEntry {
            id: row.0,
            media_id: row.1,
            target_category: row.2,
            reason: row.3,
            created_at: row.4,
            subject: row.5,
        });
    }
    tx.commit().await?;
    Ok(pinned)
}

/// What a removal answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Deleted {
    pub deleted: bool,
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Deleted>> {
    let media_id: Option<String> =
        sqlx::query_scalar("SELECT media_id FROM overrides WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;

    let Some(media_id) = media_id else {
        return Err(AppError::NotFound(format!("Override {id} not found")));
    };

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM overrides WHERE id = ?").bind(&id).execute(&mut *tx).await?;
    crate::services::routing::supersede_pending(&mut tx, &[&media_id]).await?;
    tx.commit().await?;

    Ok(Json(Deleted { deleted: true }))
}
