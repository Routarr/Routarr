//! Arr instance management.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::services::sync;
use crate::state::AppState;
use serde::Serialize;

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<InstanceResponse>>> {
    let instances = state.instances(false).await?;
    Ok(Json(
        instances
            .into_iter()
            .map(|i| InstanceResponse::from_instance(i, &state.config.base_path))
            .collect(),
    ))
}

/// Fetch one instance, with its API key masked like the list endpoint.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<Json<InstanceResponse>> {
    let base_url = validate(&req)?;

    let id = Uuid::new_v4().to_string();
    let webhook_token = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
         sync_interval_minutes, webhook_token)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(req.instance_type.to_lowercase())
    .bind(&base_url)
    // Stored encrypted; the plaintext never touches the database.
    .bind(state.secrets.seal(req.api_key.trim())?)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&webhook_token)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<Json<InstanceResponse>> {
    let base_url = validate(&req)?;
    let existing = state.instance(&id).await?;

    // An empty api_key means "keep the current one" — the UI only ever shows a
    // masked value, so re-submitting the form must not wipe the secret.
    let api_key = if req.api_key.trim().is_empty() {
        existing.api_key.clone()
    } else {
        state.secrets.seal(req.api_key.trim())?
    };

    sqlx::query(
        "UPDATE instances SET name = ?, instance_type = ?, base_url = ?, api_key = ?,
         enabled = ?, sync_interval_minutes = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(req.name.trim())
    .bind(req.instance_type.to_lowercase())
    .bind(&base_url)
    .bind(api_key)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&id)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let result =
        sqlx::query("DELETE FROM instances WHERE id = ?").bind(&id).execute(&state.pool).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Instance {id} not found")));
    }

    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// What a connection probe reports. A named struct rather than a `json!` so
/// `check-api-types` can hold it against its TypeScript mirror.
#[derive(Debug, Serialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub version: String,
    pub app_name: Option<String>,
    pub root_folders: usize,
    pub inaccessible_root_folders: usize,
}

pub async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<TestConnectionResponse>> {
    let instance = state.instance(&id).await?;
    let adapter = state.adapter(&instance)?;
    let status = adapter.test_connection().await?;
    let root_folders = adapter.get_root_folders().await?;

    Ok(Json(TestConnectionResponse {
        success: true,
        version: status.version,
        app_name: status.app_name,
        root_folders: root_folders.len(),
        inaccessible_root_folders: root_folders.iter().filter(|rf| !rf.accessible).count(),
    }))
}

/// Sync every enabled instance.
pub async fn sync_all(State(state): State<AppState>) -> AppResult<Json<Vec<sync::SyncReport>>> {
    Ok(Json(sync::sync_all_instances(&state, "manual").await?))
}

pub async fn sync_now(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<sync::SyncReport>> {
    Ok(Json(sync::sync_instance(&state, &id, "manual").await?))
}

/// Issue a fresh webhook token, invalidating the previous URL.
pub async fn rotate_webhook_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    state.instance(&id).await?;

    sqlx::query(
        "UPDATE instances SET webhook_token = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&id)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

/// Shared validation for create and update.
fn validate(req: &CreateInstanceRequest) -> AppResult<String> {
    req.instance_type.parse::<InstanceType>().map_err(AppError::BadRequest)?;

    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("Instance name cannot be empty".into()));
    }

    let base_url = req.base_url.trim().trim_end_matches('/').to_string();
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(AppError::BadRequest("base_url must start with http:// or https://".into()));
    }

    Ok(base_url)
}
