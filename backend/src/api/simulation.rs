//! Dry-run simulation.

use super::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use crate::api::jobs::{answer, prefers_async};
use crate::error::AppResult;
use crate::jobs::{Attribution, Detail, JobKind};
use crate::models::*;
use crate::services::notify;
use crate::services::routing::{self, SimulationOptions};
use crate::state::AppState;

/// Run a simulation over the (optionally filtered) library.
///
/// `persist: false` writes no decision. The run is still recorded as a job.
pub async fn run(
    State(state): State<AppState>,
    // Whoever asked, so the decisions this writes name them. The middleware
    // puts one there for every protected route, so the extractor cannot fail.
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    headers: HeaderMap,
    Json(req): Json<SimulationRequest>,
) -> AppResult<Response> {
    let work = simulate(state.clone(), identity.attribution(), req);
    answer(&state, prefers_async(&headers), work).await
}

/// The queue of previews, and how long it may grow: the two that run beside
/// each other and two waiting their turn.
const PREVIEW_QUEUE: &str = "preview";
const MAX_QUEUED_PREVIEWS: usize = 2 * routing::MAX_CONCURRENT_LIBRARY_PASSES;

async fn simulate(
    state: AppState,
    by: Attribution,
    req: SimulationRequest,
) -> AppResult<SimulationResult> {
    // One *persisting* pass at a time (see `jobs::FULL_SIMULATION`). What the
    // lock protects is the writing: two passes each supersede the other's
    // pending decisions and the later commit wins. A run that persists
    // nothing supersedes nothing. What bounds it is the permit every pass
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
    // A preview waits for its pass rather than being refused, and with
    // `Prefer: respond-async` it answers before it holds one: without a bound,
    // a caller that does not wait queues them without end, and every apply
    // behind them waits too. The place is kept until the pass has run.
    let _queued = if req.persist {
        None
    } else {
        match state.jobs.try_wait(PREVIEW_QUEUE, MAX_QUEUED_PREVIEWS) {
            Some(place) => Some(place),
            None => {
                return Err(crate::error::AppError::Conflict(
                    state.localizer().await.translate("ErrorPreviewsWaiting", &[]),
                ));
            }
        }
    };

    let mut job =
        state.jobs.start(JobKind::Simulate, &by, None, Detail::new("JobDetailSimulating")).await?;

    let outcome = routing::run_simulation(
        &state.pool,
        SimulationOptions {
            trigger: by.trigger,
            subject: by.subject,
            instance_ids: req.instance_ids.unwrap_or_default(),
            media_ids: None,
            media_type: req.media_type,
            persist: req.persist,
            persist_unchanged: req.persist_unchanged,
            rules_override: None,
            // Returning 20 000 decisions in one response is a memory spike on
            // both ends. The counters still describe the whole library.
            max_returned: Some(req.max_returned.unwrap_or(1000).clamp(1, 5000)),
            language: state.language().await,
        },
    )
    .await;

    match &outcome {
        Ok(result) => {
            if req.persist {
                notify::send_later(
                    &state,
                    notify::Event::SimulationCompleted {
                        simulation_id: result.simulation_id.clone(),
                        total: result.total_media,
                        moves: result.moves_required,
                    },
                );
            }
            // The counts, without the decisions: a task row is not where a
            // thousand proposals are kept, and `/decisions` lists them.
            if let Ok(mut summary) = serde_json::to_value(result) {
                summary["decisions"] = serde_json::json!([]);
                summary["returned"] = serde_json::json!(0);
                job.report(&summary);
            }
            job.succeed(
                Detail::new("JobDetailSimulated")
                    .with("total", result.total_media)
                    .with("moves", result.moves_required),
            )
            .await
        }
        Err(e) => job.fail(e).await,
    }

    outcome
}
