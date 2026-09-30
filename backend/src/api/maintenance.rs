//! Manual housekeeping trigger.

use super::Json;
use axum::extract::State;

use crate::error::AppResult;
use crate::services::maintenance::{self, MaintenanceReport};
use crate::state::AppState;

/// Purge stale decisions, logs and jobs according to the retention settings.
pub async fn purge(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
) -> AppResult<Json<MaintenanceReport>> {
    Ok(Json(maintenance::run(&state, &identity.attribution()).await?))
}
