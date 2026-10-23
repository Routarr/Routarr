//! Backup management.
//!
//! Every handler taking a name from the URL runs it through `is_valid_backup_name` first:
//! the name picks a file in the folder that also holds the master key.

use super::Json;
use axum::extract::State;

use super::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::auth::allowed;
use crate::error::{AppError, AppResult};
use crate::services::audit::Kind;
use crate::services::backup::{
    self, BackupCopy, BackupFile, BackupManifest, PendingRestore, ReplacedDatabase,
};
use crate::state::AppState;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackupListResponse {
    pub backups: Vec<BackupFile>,
    /// How many are kept before the oldest is pruned.
    pub retention_count: usize,
    /// The copy into `ROUTARR_BACKUP_COPY_DIR`, null when the variable is unset.
    pub copy: Option<BackupCopy>,
    /// The restore the next start applies, whenever it comes. Null when none is staged.
    pub pending_restore: Option<PendingRestore>,
    /// The database the last restore replaced, kept until deleted. Null when there is none.
    pub replaced_database: Option<ReplacedDatabase>,
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<BackupListResponse>> {
    let localizer = state.localizer().await;
    let mut backups = backup::list(&state);
    let copy = match backup::copy_status(&state, &localizer).await? {
        Some((copy, present)) => {
            for file in &mut backups {
                file.copied = present.contains(&file.name);
            }
            Some(copy)
        }
        None => None,
    };
    Ok(Json(BackupListResponse {
        backups,
        retention_count: state.setting::<usize>("backup_retention_count").await,
        copy,
        pending_restore: backup::pending_restore(&state.config),
        replaced_database: backup::replaced_database(&state.config),
    }))
}

/// Delete the database the last restore replaced, the way back from a restore made by mistake.
pub async fn delete_replaced_database(
    State(state): State<AppState>,
) -> AppResult<Json<super::Deleted>> {
    let deleted = backup::delete_replaced_database(&state.config)?;
    Ok(Json(super::Deleted { deleted }))
}

/// Take the copy folder as it is now, after a change made on purpose left it without the
/// marker of its first copy, and copy into it.
pub async fn adopt_copy_folder(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
) -> AppResult<StatusCode> {
    backup::adopt_copy_folder(&state, identity.attribution()).await?;
    let event = allowed(Kind::Backup, "AuditBackupCopyAdopted");
    crate::api::auth::audited(&state, &identity, client, event);
    Ok(StatusCode::ACCEPTED)
}

pub async fn create(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let (task_state, by) = (state.clone(), identity.attribution());
    let work = async move { backup::create(&task_state, &by).await };
    crate::api::jobs::answer(&state, crate::api::jobs::prefers_async(&headers), work).await
}

/// Stream an archive to a caller that sent the key, or to anyone in a mode without one.
/// It carries the master key and the API key, nothing a key holder lacks. A session proves
/// itself again first, through [`download_proven`].
pub async fn download(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(name): Path<String>,
) -> AppResult<Response> {
    if !backup::is_valid_backup_name(&name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    if identity.signed_in_at.is_some() {
        let localizer = state.localizer().await;
        let message = localizer.translate("ErrorBackupDownloadProof", &[("name", &name)]);
        return Err(AppError::ProofRequired(message));
    }
    stream_archive(&state, &identity, client, &name).await
}

/// Stream an archive once a session proved itself again: the password in `forms`, the API key
/// in `apikey`, a recent sign-in at the provider in `oidc`.
pub async fn download_proven(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> AppResult<Response> {
    if !backup::is_valid_backup_name(&name) {
        return Err(AppError::NotFound("Unknown backup".into()));
    }
    let proof = crate::api::account::proof_in(&body)?;
    crate::api::account::prove(&state, &identity, &proof, client).await?;
    stream_archive(&state, &identity, client, &name).await
}

async fn stream_archive(
    state: &AppState,
    identity: &crate::api::auth::Identity,
    client: Option<std::net::IpAddr>,
    name: &str,
) -> AppResult<Response> {
    let event = allowed(Kind::Backup, "AuditBackupDownloaded").with("name", name);
    crate::api::auth::audited(state, identity, client, event);

    // Streamed: an archive is the whole database, and reading it into memory
    // first doubles the process's footprint for the length of the download.
    let path = backup::backup_dir(state).join(name);
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
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(name): Path<String>,
) -> AppResult<Json<super::Deleted>> {
    backup::delete(&state, &name)?;
    let event = allowed(Kind::Backup, "AuditBackupDeleted").with("name", &name);
    crate::api::auth::audited(&state, &identity, client, event);
    Ok(Json(super::Deleted { deleted: true }))
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

/// Withdraw the restore staged for the next start, which would otherwise apply it whenever it
/// comes.
pub async fn withdraw_restore(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
) -> AppResult<Json<super::Deleted>> {
    let withdrawn = backup::withdraw_pending_restore(&state).await?;
    if let Some(withdrawn) = &withdrawn {
        let event = allowed(Kind::Restore, "AuditRestoreWithdrawn").with("name", &withdrawn.name);
        crate::api::auth::audited(&state, &identity, client, event);
    }
    Ok(Json(super::Deleted { deleted: withdrawn.is_some() }))
}

/// The backup passphrase to set, an empty one removing it, with the proof a
/// session gives. No `Debug`: it would print the passphrase.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
