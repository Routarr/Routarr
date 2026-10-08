//! Backup management.
//!
//! Every handler that takes a name from the URL runs it through
//! `is_valid_backup_name` first: these are the only endpoints where a caller
//! chooses a path on the server's filesystem, and an archive sits in the same
//! directory as the master key.

use super::Json;
use axum::extract::State;

use super::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::auth::allowed;
use crate::error::{AppError, AppResult};
use crate::services::audit::Kind;
use crate::services::backup::{self, BackupFile, BackupManifest};
use crate::state::AppState;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackupListResponse {
    pub backups: Vec<BackupFile>,
    /// How many are kept before the oldest is pruned.
    pub retention_count: usize,
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<BackupListResponse>> {
    Ok(Json(BackupListResponse {
        backups: backup::list(&state),
        retention_count: state.setting::<usize>("backup_retention_count").await,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
) -> AppResult<Json<BackupFile>> {
    Ok(Json(backup::create(&state, &identity.attribution()).await?))
}

/// Stream an archive to the caller.
///
/// It carries the master key, so this is the one endpoint that hands out the
/// means to decrypt every stored Arr credential. That is why it sits behind the
/// API key like everything else: whoever holds the key holds the credentials,
/// and that escalation is stated rather than left implicit.
pub async fn download(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(name): Path<String>,
) -> AppResult<Response> {
    if !backup::is_valid_backup_name(&name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let event = allowed(Kind::Backup, "AuditBackupDownloaded").with("name", &name);
    crate::api::auth::audited(&state, &identity, client, event);

    // Streamed: an archive is the whole database, and reading it into memory
    // first doubles the process's footprint for the length of the download.
    let path = backup::backup_dir(&state).join(&name);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound("Unknown backup".into()))?;
    let length = file.metadata().await.map(|m| m.len()).ok();
    let content_type =
        if backup::is_sealed_file(&path) { "application/octet-stream" } else { "application/zip" };
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
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

/// What a restore may carry: the passphrase of a sealed archive, once the
/// server asked for it with `passphrase_required`. No `Debug`: it would print
/// the passphrase.
#[derive(Default, serde::Deserialize)]
pub struct RestoreRequest {
    #[serde(default)]
    pub passphrase: Option<String>,
}

pub async fn restore(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> AppResult<Json<RestoreResponse>> {
    let request: RestoreRequest = if body.iter().all(u8::is_ascii_whitespace) {
        RestoreRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("The body is not a restore request: {e}")))?
    };
    let given = request.passphrase.map(backup::passphrase_of).filter(|given| !given.is_empty());
    let manifest = backup::stage_restore(&state, &name, given).await?;
    let event = allowed(Kind::Restore, "AuditRestoreStaged").with("name", &name);
    crate::api::auth::audited(&state, &identity, client, event);
    Ok(Json(RestoreResponse { manifest, restart_required: true }))
}

/// The backup passphrase to set, an empty one removing it, with the proof a
/// session gives. No `Debug`: it would print the passphrase.
#[derive(serde::Deserialize)]
pub struct PassphraseChange {
    pub passphrase: String,
    #[serde(flatten)]
    pub proof: crate::api::account::Proof,
}

/// Set, change or remove the backup passphrase, and convert the archives on
/// disk to it. The proof first, as for a key: removing the passphrase opens
/// every archive, and one set by a session left open would seal them for
/// somebody else.
pub async fn set_passphrase(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Json(change): Json<PassphraseChange>,
) -> AppResult<StatusCode> {
    crate::api::account::prove(&state, &identity, &change.proof, client).await?;
    let new = backup::passphrase_of(change.passphrase);
    if !new.is_empty() && new.chars().count() < backup::MIN_PASSPHRASE_LENGTH {
        let min = backup::MIN_PASSPHRASE_LENGTH.to_string();
        let refusal = state.localizer().await.translate("ErrorPassphraseShort", &[("min", &min)]);
        return Err(AppError::BadRequest(refusal));
    }
    let message =
        if new.is_empty() { "AuditBackupPassphraseRemoved" } else { "AuditBackupPassphraseSet" };
    backup::set_passphrase(&state, new, identity.attribution()).await?;
    crate::api::auth::audited(&state, &identity, client, allowed(Kind::Backup, message));
    Ok(StatusCode::NO_CONTENT)
}
