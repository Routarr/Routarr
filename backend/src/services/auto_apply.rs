//! Applying a routing decision with nobody watching.
//!
//! This is the one place in Routarr that writes to an Arr without a person
//! having clicked something, so it is deliberately the most restrictive.
//!
//! The case it exists for: Radarr fires `MovieAdded` the moment a film enters
//! the library, before anything has been downloaded. The webhook syncs it,
//! enriches it and evaluates the rules — and the right root folder is known
//! while the folder is still empty. Correcting it *then* costs one API call and
//! moves no bytes. Waiting for a human means the file lands in the wrong place
//! first and has to be moved afterwards.
//!
//! Everything here follows from that: if there is a file to move, this is no
//! longer the cheap pre-download correction, and the decision goes back to the
//! human queue.

use tracing::{debug, info, warn};

use crate::error::AppResult;
use crate::jobs::Attribution;
use crate::services::executor::{self, ApplyReport};
use crate::services::notify;
use crate::state::AppState;

/// What an unattended pass decided to do.
#[derive(Debug)]
pub enum AutoApplyOutcome {
    /// A guardrail said no. Carries the reason for the log line.
    Held(&'static str),
    /// Nothing in this run qualified.
    NothingToApply,
    /// More candidates than one unattended run may touch, so none were touched.
    OverCap {
        candidates: usize,
        cap: usize,
    },
    Applied(ApplyReport),
}

/// Consider the moves a simulation just proposed for unattended application.
///
/// `simulation_id` scopes the run: only decisions this very simulation wrote are
/// eligible, so a background pass can never pick up a proposal the user has been
/// sitting on. Errors are returned rather than swallowed; callers on a background
/// path log them and carry on, since a failed auto-apply must not fail the sync
/// or the webhook that triggered it.
pub async fn apply_simulation(
    state: &AppState,
    simulation_id: &str,
    trigger: &str,
) -> AppResult<AutoApplyOutcome> {
    let outcome = decide(state, simulation_id, trigger).await?;

    // Logged here rather than at each call site, so "I turned auto-apply on and
    // nothing happened" always has an answer in the log.
    match &outcome {
        AutoApplyOutcome::Applied(report) => {
            info!(
                trigger,
                applied = report.applied,
                failed = report.failed,
                "Auto-applied routing decisions"
            );
            // An unattended write that the Arr refused is exactly what nobody
            // is watching for.
            if report.failed > 0 {
                notify::send(
                    state,
                    notify::Event::AutoApplyFailed {
                        failed: report.failed,
                        applied: report.applied,
                        first_error: report
                            .errors
                            .first()
                            .map(|e| format!("{}: {}", e.media_title, e.message))
                            .unwrap_or_else(|| "unknown".into()),
                    },
                )
                .await;
            }
        }
        AutoApplyOutcome::OverCap { candidates, cap } => warn!(
            trigger,
            candidates,
            cap,
            "Auto-apply held back: too many moves for one unattended run. Apply them from the \
             Simulation screen"
        ),
        AutoApplyOutcome::Held(reason) => debug!(trigger, reason, "Auto-apply held back"),
        AutoApplyOutcome::NothingToApply => {
            debug!(trigger, "Auto-apply had nothing to do")
        }
    }

    Ok(outcome)
}

async fn decide(
    state: &AppState,
    simulation_id: &str,
    trigger: &str,
) -> AppResult<AutoApplyOutcome> {
    // Opt-in. A fresh install never writes on its own.
    if !state.bool_setting("auto_apply_enabled", false).await {
        return Ok(AutoApplyOutcome::Held("auto-apply is disabled"));
    }

    // Checked here as well as in the executor: this one is the master switch,
    // and it should be impossible to reach the writer with it on.
    if state.bool_setting("global_dry_run", true).await {
        return Ok(AutoApplyOutcome::Held("global dry-run is active"));
    }

    let candidates = eligible_decisions(state, simulation_id).await?;
    if candidates.is_empty() {
        return Ok(AutoApplyOutcome::NothingToApply);
    }

    // Over the ceiling, apply *nothing*. Silently doing the first 50 of 500 is
    // worse than doing none: the library ends up half-reorganised with no record
    // of the intent. A sweep that large is a library reclassification, which is
    // a deliberate human act.
    let cap: usize = state.setting("batch_limit", 50usize).await;
    if candidates.len() > cap {
        return Ok(AutoApplyOutcome::OverCap { candidates: candidates.len(), cap });
    }

    Ok(AutoApplyOutcome::Applied(
        executor::apply_unattended(state, &candidates, &Attribution::unattended(trigger)).await?,
    ))
}

/// The decisions from one simulation that may be written without asking.
///
/// Beyond the usual "pending, current, actionable" filter, the media must have
/// **no files on disk**. That single condition is what makes the feature safe:
/// with nothing to move, changing the root folder is a metadata edit that Radarr
/// applies instantly and that `POST /decisions/revert` undoes just as cheaply.
/// The moment files exist, the same edit either strands them at the old path or
/// starts a real disk move — neither belongs in an unattended pass.
async fn eligible_decisions(state: &AppState, simulation_id: &str) -> AppResult<Vec<String>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT d.id
           FROM decisions d
           JOIN media m ON m.id = d.media_id
          WHERE d.simulation_id = ?
            AND d.status = 'pending'
            AND d.superseded = 0
            AND d.action = 'move'
            AND d.target_root_folder IS NOT NULL
            AND m.has_files = 0
            -- And the destination answered the last time anyone looked.
            --
            -- The routing map keeps a folder the Arr reports unreachable, since
            -- a NAS that spins down is unknown rather than gone; the manual
            -- path asks about it at apply time. There is nobody to ask here, so
            -- an unattended pass leaves those items for the person who can
            -- answer instead of writing into a destination that is not there.
            AND EXISTS (
                SELECT 1 FROM root_folders rf
                 WHERE rf.instance_id = d.instance_id
                   AND rtrim(rf.path, '/') = rtrim(d.target_root_folder, '/')
                   AND rf.accessible = 1
            )
          ORDER BY d.media_title",
    )
    .bind(simulation_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(ids)
}
