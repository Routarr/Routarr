//! Manual housekeeping trigger.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use crate::api::jobs::{answer, prefers_async};
use crate::error::AppResult;
use crate::services::maintenance;
use crate::state::AppState;

/// Purge stale decisions, logs and jobs according to the retention settings.
pub async fn purge(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let (task_state, by) = (state.clone(), identity.attribution());
    let work = async move { maintenance::run(&task_state, &by).await };
    answer(&state, prefers_async(&headers), work).await
}
