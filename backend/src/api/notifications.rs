//! The secret the notification webhook is signed with, and a test message to
//! it. Owner-only.

use super::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::api::auth::allowed;
use crate::error::AppResult;
use crate::services::audit::Kind;
use crate::services::notify::{self, SigningStatus};
use crate::state::AppState;

pub async fn signing(State(state): State<AppState>) -> AppResult<Json<SigningStatus>> {
    Ok(Json(notify::signing_status(&state).await?))
}

/// A new secret, returned this once. The one it replaces keeps signing beside
/// it for a day.
#[derive(Debug, serde::Serialize)]
pub struct SigningSecret {
    pub secret: String,
}

pub async fn rotate_signing(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
) -> AppResult<Json<SigningSecret>> {
    let secret = notify::rotate_signing_secret(&state).await?;
    let event = allowed(Kind::SigningSecret, "AuditSigningSecretMade");
    crate::api::auth::audited(&state, &identity, client, event);
    Ok(Json(SigningSecret { secret }))
}

pub async fn remove_signing(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
) -> AppResult<Json<super::Deleted>> {
    notify::remove_signing_secrets(&state).await?;
    let event = allowed(Kind::SigningSecret, "AuditSigningSecretsRemoved");
    crate::api::auth::audited(&state, &identity, client, event);
    Ok(Json(super::Deleted { deleted: true }))
}

/// Send a test notification to the saved address, in the saved format, and
/// answer once it has arrived or with why it has not.
pub async fn test(State(state): State<AppState>) -> AppResult<StatusCode> {
    let localizer = state.localizer().await;
    notify::send_test(&state, &localizer).await?;
    Ok(StatusCode::NO_CONTENT)
}
