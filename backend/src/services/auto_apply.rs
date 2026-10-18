//! Applying a routing decision with nobody watching, the most restrictive writer in Routarr.
//! Only a title with no file yet qualifies: right after `MovieAdded` a move costs one API call
//! and moves no bytes. A title with a file goes back to the human queue.

use tracing::{debug, info, warn};

use crate::error::AppResult;
use crate::jobs::Attribution;
use crate::services::executor::{self, ApplyReport};
use crate::services::notify;
use crate::state::AppState;

/// What an unattended pass decided to do.
#[derive(Debug)]
pub enum AutoApplyOutcome {
    /// A guardrail said no.
    Held(Hold),
    /// Nothing in this run qualified.
    NothingToApply,
    /// More candidates than one unattended run may touch, so none were touched.
    OverCap {
        candidates: usize,
        cap: usize,
    },
    Applied(ApplyReport),
}

/// Consider the moves a simulation just proposed for unattended application. Only decisions
/// this simulation wrote are eligible, never a proposal the user is sitting on. A background
/// caller logs the error and carries on, so a failed auto-apply never fails its sync.
pub async fn apply_simulation(
    state: &AppState,
    simulation_id: &str,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    let outcome = decide(state, simulation_id, by).await?;
    let trigger = by.trigger.as_str();

    // Logged here, so an auto-apply that did nothing always has an answer in the log.
    match &outcome {
        AutoApplyOutcome::Applied(report) => {
            info!(
                trigger,
                applied = report.applied,
                failed = report.failed,
                "Auto-applied routing decisions"
            );
            // On its own task, or a slow receiver holds back the webhook's delivery.
            if let Some(first) = report.errors.first() {
                notify::send_later(
                    state,
                    notify::Event::AutoApplyFailed {
                        failed: report.failed,
                        applied: report.applied,
                        title: first.media_title.clone(),
                        error: first.stored.clone(),
                    },
                );
            }
        }
        AutoApplyOutcome::OverCap { candidates, cap } => warn!(
            trigger,
            candidates,
            cap,
            "Auto-apply held back: too many moves for one unattended run. Apply them from the \
             Simulation screen"
        ),
        AutoApplyOutcome::Held(Hold::TurnNotReached) => warn!(
            trigger,
            "Auto-apply held back: another apply kept running past the wait, the moves stay pending"
        ),
        AutoApplyOutcome::Held(Hold::RuleTestsFailing { failed, total }) => warn!(
            trigger,
            failed, total, "Auto-apply held back: rule tests are failing, the moves stay pending"
        ),
        AutoApplyOutcome::Held(hold) => debug!(trigger, ?hold, "Auto-apply held back"),
        AutoApplyOutcome::NothingToApply => {
            debug!(trigger, "Auto-apply had nothing to do")
        }
    }
    held_back(state, &outcome);

    Ok(outcome)
}

/// Why an unattended pass wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hold {
    Disabled,
    DryRun,
    /// A rule test fails: a rule change broke a routing someone pinned.
    RuleTestsFailing {
        failed: usize,
        total: usize,
    },
    /// An apply kept running past the wait, and the moves stay pending.
    TurnNotReached,
}

/// Keep the last pass's verdict on the moves it held back for being too many,
/// which `/status` warns of, and say so once when it starts holding them.
/// Another verdict on the library clears it, a guardrail's leaves it.
fn held_back(state: &AppState, outcome: &AutoApplyOutcome) {
    let mut held = state.auto_apply_held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match outcome {
        AutoApplyOutcome::OverCap { candidates, cap } => {
            if held.replace((*candidates, *cap)).is_none() {
                notify::send_later(
                    state,
                    notify::Event::AutoApplyHeld { candidates: *candidates, cap: *cap },
                );
            }
        }
        AutoApplyOutcome::Applied(_) | AutoApplyOutcome::NothingToApply => *held = None,
        AutoApplyOutcome::Held(_) => {}
    }
}

async fn decide(
    state: &AppState,
    simulation_id: &str,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    // Opt-in. A fresh install never writes on its own.
    if !state.bool_setting("auto_apply_enabled").await {
        return Ok(AutoApplyOutcome::Held(Hold::Disabled));
    }

    // Checked before the executor too: the writer must be unreachable with dry run on.
    if state.bool_setting("global_dry_run").await {
        return Ok(AutoApplyOutcome::Held(Hold::DryRun));
    }

    let candidates = eligible_decisions(state, simulation_id).await?;
    if candidates.is_empty() {
        return Ok(AutoApplyOutcome::NothingToApply);
    }

    let tests = crate::services::rule_tests::check(state).await?;
    if tests.failed > 0 {
        let (failed, total) = (tests.failed, tests.total);
        return Ok(AutoApplyOutcome::Held(Hold::RuleTestsFailing { failed, total }));
    }

    // Over the cap, apply nothing, or the library is left half reorganised with no record.
    let cap: usize = state.bounding_setting::<usize>("batch_limit").await?;
    if candidates.len() > cap {
        return Ok(AutoApplyOutcome::OverCap { candidates: candidates.len(), cap });
    }

    // The turn first: a "no file yet" read before the wait may be stale when the apply runs.
    let Some(turn) = executor::unattended_turn(state).await else {
        return Ok(AutoApplyOutcome::Held(Hold::TurnNotReached));
    };
    let without_files = still_without_files(state, candidates).await;
    if without_files.is_empty() {
        return Ok(AutoApplyOutcome::NothingToApply);
    }

    Ok(AutoApplyOutcome::Applied(
        executor::apply_unattended(state, &without_files, &Attribution::automatic(by), turn)
            .await?,
    ))
}

/// A decision an unattended pass may write, and what reading its item again takes.
#[derive(sqlx::FromRow)]
struct Candidate {
    decision_id: String,
    instance_id: String,
    arr_id: i64,
    media_title: String,
}

/// The decisions from one simulation that may be written without asking: pending, current,
/// actionable and with no file on disk. With a file, the same edit strands it at the old path
/// or starts a real disk move.
async fn eligible_decisions(state: &AppState, simulation_id: &str) -> AppResult<Vec<Candidate>> {
    let candidates = sqlx::query_as(
        "SELECT d.id AS decision_id, d.instance_id, m.arr_id, d.media_title
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
                   AND rf.path = d.target_root_folder COLLATE path
                   AND rf.accessible = 1
            )
            -- And its move did not fail within the day. An Arr that refused it
            -- refuses it again at the next pass, and a retry every pass is a
            -- failed row, a log and a notification every quarter of an hour.
            -- A person can still apply it.
            AND NOT EXISTS (
                SELECT 1 FROM decisions f
                 WHERE f.media_id = d.media_id AND f.status = 'failed'
                   AND f.decided_at > datetime('now', '-1 day')
            )
          ORDER BY d.media_title",
    )
    .bind(simulation_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(candidates)
}

/// The candidates the Arr still reports without a file. A film downloaded since the last sync
/// has a file the database does not know, which `moveFiles: false` would leave behind. An item
/// that cannot be read again is left to a person.
async fn still_without_files(state: &AppState, candidates: Vec<Candidate>) -> Vec<String> {
    let mut kept = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match reads_without_files(state, &candidate).await {
            Ok(true) => kept.push(candidate.decision_id),
            Ok(false) => info!(
                item = %candidate.media_title,
                "Auto-apply held back a move: the Arr reports a file, or no longer has the item"
            ),
            Err(e) => warn!(
                item = %candidate.media_title,
                "Auto-apply held back a move: the item could not be read again: {e}"
            ),
        }
    }
    kept
}

async fn reads_without_files(state: &AppState, candidate: &Candidate) -> AppResult<bool> {
    let instance = state.instance(&candidate.instance_id).await?;
    let item = state.adapter(&instance)?.get_media_one(candidate.arr_id).await?;
    Ok(item.is_some_and(|item| !item.has_files))
}
