//! Dry-run simulation.

use super::Json;
use axum::extract::State;

use crate::error::AppResult;
use crate::jobs::JobKind;
use crate::models::*;
use crate::services::routing::{self, SimulationOptions};
use crate::state::AppState;

/// Run a simulation over the (optionally filtered) library.
///
/// `persist: false` writes no decision; the run is still recorded as a job.
pub async fn run(
    State(state): State<AppState>,
    // Whoever asked, so the decisions this writes name them. The middleware
    // puts one there for every protected route, so the extractor cannot fail.
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Json(req): Json<SimulationRequest>,
) -> AppResult<Json<SimulationResult>> {
    // One *persisting* pass at a time — see `jobs::FULL_SIMULATION`. What the
    // lock protects is the writing: two passes each supersede the other's
    // pending decisions and the later commit wins. A run that persists
    // nothing supersedes nothing; what bounds it is the permit every pass
    // takes in `routing::run_simulation`, which waits rather than refuses.
    let _pass = if req.persist {
        match state.jobs.try_lock(crate::jobs::FULL_SIMULATION) {
            Some(lock) => Some(lock),
            None => {
                return Err(crate::error::AppError::Conflict(
                    state.localizer().await.translate("ErrorSimulationInProgress", &[]),
                ));
            }
        }
    } else {
        None
    };

    let job = state
        .jobs
        .start(JobKind::Simulate, crate::jobs::TRIGGER_MANUAL, None, "Running simulation")
        .await?;

    let outcome = routing::run_simulation(
        &state.pool,
        SimulationOptions {
            trigger: crate::jobs::TRIGGER_MANUAL.to_string(),
            subject: identity.actor().map(str::to_string),
            instance_ids: req.instance_ids.unwrap_or_default(),
            media_ids: None,
            media_type: req.media_type,
            persist: req.persist,
            persist_unchanged: req.persist_unchanged,
            rules_override: None,
            // Returning 20 000 decisions in one response is a memory spike on
            // both ends; the counters still describe the whole library.
            max_returned: Some(req.max_returned.unwrap_or(1000).clamp(1, 5000)),
            language: state.language().await,
        },
    )
    .await;

    match &outcome {
        Ok(result) => {
            job.succeed(&format!(
                "media evaluated: {}, moves required: {}",
                result.total_media, result.moves_required
            ))
            .await
        }
        Err(e) => job.fail(&e.to_string()).await,
    }

    outcome.map(Json)
}
