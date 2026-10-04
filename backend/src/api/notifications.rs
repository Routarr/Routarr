//! The secret the notification webhook is signed with, and a test message to
//! it. Owner-only.

use super::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::error::AppResult;
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

pub async fn rotate_signing(State(state): State<AppState>) -> AppResult<Json<SigningSecret>> {
    Ok(Json(SigningSecret { secret: notify::rotate_signing_secret(&state).await? }))
}

pub async fn remove_signing(State(state): State<AppState>) -> AppResult<StatusCode> {
    notify::remove_signing_secrets(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Send a test notification to the saved address, in the saved format, and
/// answer once it has arrived or with why it has not.
pub async fn test(State(state): State<AppState>) -> AppResult<StatusCode> {
    let localizer = state.localizer().await;
    notify::send_test(&state, &localizer).await?;
    Ok(StatusCode::NO_CONTENT)
}
