//! Backup management.
//!
//! Every handler that takes a name from the URL runs it through
//! `is_valid_backup_name` first: these are the only endpoints where a caller
//! chooses a path on the server's filesystem, and an archive sits in the same
//! directory as the master key.

use super::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::services::backup::{self, BackupFile, BackupManifest};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct BackupListResponse {
    pub backups: Vec<BackupFile>,
    /// How many are kept before the oldest is pruned.
    pub retention_count: usize,
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<BackupListResponse>> {
    Ok(Json(BackupListResponse {
        backups: backup::list(&state),
        retention_count: state.setting("backup_retention_count", 7usize).await,
    }))
}

pub async fn create(State(state): State<AppState>) -> AppResult<Json<BackupFile>> {
    Ok(Json(backup::create(&state, crate::jobs::TRIGGER_MANUAL).await?))
}

/// Stream an archive to the caller.
///
/// It carries the master key, so this is the one endpoint that hands out the
/// means to decrypt every stored Arr credential. That is why it sits behind the
/// API key like everything else: whoever holds the key holds the credentials,
/// and that escalation is stated rather than left implicit.
pub async fn download(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    if !backup::is_valid_backup_name(&name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }

    // Streamed: an archive is the whole database, and reading it into memory
    // first doubles the process's footprint for the length of the download.
    let path = backup::backup_dir(&state).join(&name);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let length = file.metadata().await.map(|m| m.len()).ok();
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response();
    if let Some(length) = length {
        response.headers_mut().insert(header::CONTENT_LENGTH, length.into());
    }
    Ok(response)
}

pub async fn remove(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    backup::delete(&state, &name)?;
    Ok(Json(serde_json::json!({ "deleted": name })))
}

#[derive(Debug, Serialize)]
pub struct RestoreResponse {
    /// What was staged, so the interface can say what will be applied.
    pub manifest: BackupManifest,
    /// Always true: the swap happens at the next start, never under a live pool.
    pub restart_required: bool,
}

pub async fn restore(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<RestoreResponse>> {
    let manifest = backup::stage_restore(&state, &name).await?;
    Ok(Json(RestoreResponse { manifest, restart_required: true }))
}
