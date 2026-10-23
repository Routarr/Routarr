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
    /// Nothing was written, and the instances listed had more moves than one unattended run may
    /// touch.
    OverCap(Vec<OverCap>),
    /// The moves of the instances within the cap, the others listed.
    Applied { report: ApplyReport, over_cap: Vec<OverCap> },
}

/// An instance whose moves one unattended run left, for being more than `batch_limit`.
#[derive(Debug, Clone, PartialEq)]
pub struct OverCap {
    pub instance_id: String,
    pub instance: String,
    pub candidates: usize,
    pub cap: usize,
}

impl AutoApplyOutcome {
    fn over_cap(&self) -> &[OverCap] {
        match self {
            Self::OverCap(held) | Self::Applied { over_cap: held, .. } => held,
            Self::Held(_) | Self::NothingToApply => &[],
        }
    }
}

/// Consider the moves a simulation just proposed for unattended application. Only decisions
/// this simulation wrote are eligible, never a proposal the user is sitting on. A background
/// caller logs the error and carries on, so a failed auto-apply never fails its sync.
pub async fn apply_simulation(
    state: &AppState,
    simulation_id: &str,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    apply(state, simulation_id, None, by).await
}

/// [`apply_simulation`] for a simulation of `instance` alone, as a webhook's: the verdicts on the
/// other instances stand.
pub async fn apply_instance_simulation(
    state: &AppState,
    simulation_id: &str,
    instance: &str,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    apply(state, simulation_id, Some(instance), by).await
}

async fn apply(
    state: &AppState,
    simulation_id: &str,
    instance: Option<&str>,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    let outcome = decide(state, simulation_id, by).await?;
    let trigger = by.trigger.as_str();

    for held in outcome.over_cap() {
        warn!(
            trigger,
            instance = %held.instance,
            candidates = held.candidates,
            cap = held.cap,
            "Auto-apply held back: too many moves for one unattended run. Apply them from the \
             Simulation screen"
        );
    }
    // Logged here, so an auto-apply that did nothing always has an answer in the log.
    match &outcome {
        AutoApplyOutcome::Applied { report, .. } => {
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
        AutoApplyOutcome::OverCap(_) => {}
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
    held_back(state, instance, &outcome);

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

/// Keep, per instance, the last pass's verdict on the moves it held back for being too many,
/// which `/status` warns of, and say so once when an instance starts being held. A verdict
/// replaces the ones on the instances its simulation covered, a guardrail's leaves them.
fn held_back(state: &AppState, instance: Option<&str>, outcome: &AutoApplyOutcome) {
    if matches!(outcome, AutoApplyOutcome::Held(_)) {
        return;
    }
    let mut held = state.auto_apply_held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = std::mem::take(&mut *held);
    *held = match instance {
        Some(id) => before.iter().filter(|(held, _)| *held != id).map(clone_entry).collect(),
        None => Default::default(),
    };
    for over in outcome.over_cap() {
        held.insert(over.instance_id.clone(), over.clone());
        if !before.contains_key(&over.instance_id) {
            notify::send_later(
                state,
                notify::Event::AutoApplyHeld {
                    instance: over.instance.clone(),
                    candidates: over.candidates,
                    cap: over.cap,
                },
            );
        }
    }
}

fn clone_entry((id, over): (&String, &OverCap)) -> (String, OverCap) {
    (id.clone(), over.clone())
}

async fn decide(
    state: &AppState,
    simulation_id: &str,
    by: &Attribution,
) -> AppResult<AutoApplyOutcome> {
    if let Some(held) = switched_off(state).await {
        return Ok(AutoApplyOutcome::Held(held));
    }
    let candidates = eligible_decisions(state, simulation_id).await?;
    if candidates.is_empty() {
        return Ok(AutoApplyOutcome::NothingToApply);
    }
    if let Some(held) = failing_tests(state).await? {
        return Ok(AutoApplyOutcome::Held(held));
    }

    // An instance over the cap gets nothing, or its library is left half reorganised with no
    // record. Counted per instance, so one backlog does not hold another instance's moves.
    let cap: usize = state.bounding_setting::<usize>("batch_limit").await?;
    let (candidates, over_cap) = within_cap(candidates, cap);
    if candidates.is_empty() {
        return Ok(AutoApplyOutcome::OverCap(over_cap));
    }

    // The turn first: a "no file yet" read before the wait may be stale when the apply runs.
    let Some(turn) = executor::unattended_turn(state).await else {
        return Ok(AutoApplyOutcome::Held(Hold::TurnNotReached));
    };
    // Asked again: the wait lasts minutes, in which a person may switch it all off.
    if let Some(held) = switched_off(state).await {
        return Ok(AutoApplyOutcome::Held(held));
    }
    if let Some(held) = failing_tests(state).await? {
        return Ok(AutoApplyOutcome::Held(held));
    }
    // Each Arr is asked again: the stored reachability can be a day old, with nobody to answer.
    let instances: std::collections::BTreeSet<&str> =
        candidates.iter().map(|candidate| candidate.instance_id.as_str()).collect();
    for instance_id in instances {
        if let Err(e) = crate::services::sync::refresh_root_folders(state, instance_id).await {
            debug!(instance = %instance_id, "The root folders could not be read again: {e}");
        }
    }
    let still: std::collections::HashSet<String> = eligible_decisions(state, simulation_id)
        .await?
        .into_iter()
        .map(|candidate| candidate.decision_id)
        .collect();
    let candidates: Vec<Candidate> =
        candidates.into_iter().filter(|candidate| still.contains(&candidate.decision_id)).collect();
    let without_files = still_without_files(state, candidates).await;
    if without_files.is_empty() {
        return Ok(if over_cap.is_empty() {
            AutoApplyOutcome::NothingToApply
        } else {
            AutoApplyOutcome::OverCap(over_cap)
        });
    }

    let report =
        executor::apply_unattended(state, &without_files, &Attribution::automatic(by), turn)
            .await?;
    Ok(AutoApplyOutcome::Applied { report, over_cap })
}

/// The switches that keep every move waiting for a person. Opt-in, so a fresh install never
/// writes on its own, and the dry run is checked before the executor too.
async fn switched_off(state: &AppState) -> Option<Hold> {
    if !state.bool_setting("auto_apply_enabled").await {
        return Some(Hold::Disabled);
    }
    state.bool_setting("global_dry_run").await.then_some(Hold::DryRun)
}

/// A rule test that fails: a rule change broke a routing someone pinned.
async fn failing_tests(state: &AppState) -> AppResult<Option<Hold>> {
    let tests = crate::services::rule_tests::check(state).await?;
    Ok((tests.failed > 0)
        .then_some(Hold::RuleTestsFailing { failed: tests.failed, total: tests.total }))
}

/// The candidates of the instances with no more than `cap` of them, and the instances with more.
fn within_cap(candidates: Vec<Candidate>, cap: usize) -> (Vec<Candidate>, Vec<OverCap>) {
    let mut per_instance: std::collections::BTreeMap<String, Vec<Candidate>> = Default::default();
    for candidate in candidates {
        per_instance.entry(candidate.instance_id.clone()).or_default().push(candidate);
    }
    let (mut kept, mut over) = (Vec::new(), Vec::new());
    for (instance_id, group) in per_instance {
        if group.len() > cap {
            over.push(OverCap {
                instance: group[0].instance_name.clone(),
                instance_id,
                candidates: group.len(),
                cap,
            });
        } else {
            kept.extend(group);
        }
    }
    (kept, over)
}

/// A decision an unattended pass may write, and what reading its item again takes.
#[derive(sqlx::FromRow)]
struct Candidate {
    decision_id: String,
    instance_id: String,
    instance_name: String,
    arr_id: i64,
    media_title: String,
}

/// The decisions from one simulation that may be written without asking: on an instance left to
/// automatic application, pending, current, actionable and with no file on disk. With a file,
/// the same edit strands it at the old path or starts a real disk move.
async fn eligible_decisions(state: &AppState, simulation_id: &str) -> AppResult<Vec<Candidate>> {
    let candidates = sqlx::query_as(
        "SELECT d.id AS decision_id, d.instance_id, i.name AS instance_name, m.arr_id,
                d.media_title
           FROM decisions d
           JOIN media m ON m.id = d.media_id
           JOIN instances i ON i.id = d.instance_id AND i.auto_apply = 1
          WHERE d.simulation_id = ?
            AND d.status = 'pending'
            AND d.superseded = 0
            AND d.action = 'move'
            AND d.target_root_folder IS NOT NULL
            AND m.has_files = 0
            -- And the destination answered at the last look: a sleeping NAS stays mapped,
            -- and with nobody here to ask, its titles are left for a person.
            AND EXISTS (
                SELECT 1 FROM root_folders rf
                 WHERE rf.instance_id = d.instance_id
                   AND rf.path = d.target_root_folder COLLATE path
                   AND rf.accessible = 1
            )
            -- And its move did not fail within the day: an Arr that refused it refuses it again,
            -- and a retry every pass is a failed row and a notification each time. A person can
            -- still apply it. Read from the move's log: a decision keeps no time of its failure.
            AND NOT EXISTS (
                SELECT 1 FROM execution_logs f
                 WHERE f.media_id = d.media_id AND f.action = 'move' AND f.success = 0
                   AND f.executed_at > datetime('now', '-1 day')
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
