//! The only code that writes to Radarr or Sonarr.
//!
//! Guardrails, in order: global dry-run, batch limit, the destinations'
//! reachability and free space, explicit confirmation past a threshold, and
//! per-decision revalidation right before the call. Each title is its own
//! call, which keeps its folder name and settles it on its own answer.

mod follow;
mod guards;
mod load;
mod record;

use tracing::{error, info};

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::registry::JobLock;
use crate::jobs::{Attribution, Detail, JobKind, Stop, detached};
use crate::services::notify;
use crate::state::AppState;

pub use follow::settle_requested;
pub(crate) use guards::REVERTIBLE;
pub use guards::{Confirmed, confirm};

use follow::{Following, Sent, follow};
use guards::{
    CapacityScope, guard_batch_limit, guard_capacity, guard_confirmation, guard_dry_run,
    guard_reachable,
};
use load::{
    applicable_from_simulation, current_proposals, distinct, load_pending_moves,
    load_revertible_moves, refuse_unknown_decisions,
};
use record::{fail, fail_batch, record_requested, record_sent, record_success};

/// Outcome of an apply or revert run.
#[derive(Debug, Default, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ApplyReport {
    pub requested: usize,
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Moves of files the Arr was still making when the apply stopped
    /// waiting. Their decisions are `requested`, and the next sync confirms or
    /// fails each.
    pub moving: usize,
    /// Selected proposals a newer simulation replaced with a different move,
    /// not applied. One replaced by the same move is applied through it.
    pub superseded: usize,
    /// Why the run ended before its last move, `null` when it ran through.
    /// What was not attempted is still pending, or still applied for a revert.
    pub stopped: Option<StopReason>,
    pub errors: Vec<ApplyError>,
}

/// Why a run that moves titles ended before its last move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The global dry-run was turned on.
    DryRun,
    /// Somebody cancelled the task.
    Cancelled,
    /// Routarr was stopping.
    Shutdown,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ApplyError {
    pub decision_id: String,
    pub media_title: String,
    pub message: String,
}

/// Columns shared by the apply and revert loaders.
type MoveRow = (String, String, String, String, Option<String>, String, i64);

/// One pending move, resolved from a decision row.
#[derive(Debug, Clone)]
struct PendingMove {
    decision_id: String,
    media_id: String,
    media_title: String,
    instance_id: String,
    arr_id: i64,
    from: Option<String>,
    to: String,
}

impl From<MoveRow> for PendingMove {
    fn from((decision_id, media_id, media_title, instance_id, from, to, arr_id): MoveRow) -> Self {
        Self { decision_id, media_id, media_title, instance_id, arr_id, from, to }
    }
}

/// Apply a set of decisions a person selected.
pub async fn apply_decisions(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    confirmed: &Confirmed,
    by: &Attribution,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    let asked = distinct(decision_ids);
    let (current, superseded) = current_proposals(&state.pool, &asked).await?;
    if current.is_empty() && superseded > 0 {
        let localizer = state.localizer().await;
        return Err(AppError::Conflict(localizer.translate("ErrorProposalsReplaced", &[])));
    }
    let decision_ids = current.as_slice();
    // The limit first: it is a count, so it costs no query, and it keeps a
    // list longer than one statement can bind from ever reaching one, since
    // the capacity guard spells the ids out. Then capacity before the
    // threshold, because a destination that cannot hold the plan is a graver
    // thing to be told than a count. Each asks under its own name and reads
    // only that name back, so answering one never answers the other.
    guard_batch_limit(state, decision_ids.len()).await?;
    // Reachability before capacity: a destination that is not answering at all
    // is a graver thing to be told than one that may be short of room, and the
    // second figure is unknown while the first is true.
    guard_reachable(state, CapacityScope::Decisions(decision_ids), confirmed).await?;
    guard_capacity(state, CapacityScope::Decisions(decision_ids), move_files, confirmed).await?;
    guard_confirmation(state, decision_ids.len(), confirmed).await?;
    refuse_unknown_decisions(state, decision_ids).await?;
    run_apply(state, decision_ids, move_files, by, superseded).await
}

/// Outcome of applying a whole simulation, slice by slice.
#[derive(Debug, Default, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchApplyReport {
    /// Everything the simulation proposed and that was still applicable.
    pub candidates: usize,
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Slices actually attempted, fewer than planned when one of them failed.
    pub batches_run: usize,
    pub batches_planned: usize,
    /// Moves of files the Arr was still making when the run stopped waiting,
    /// as in `ApplyReport`.
    pub moving: usize,
    /// Proposals of the simulation a newer one replaced with a different
    /// move, not applied.
    pub superseded: usize,
    /// True when the Arr refused a slice whole, one could not be read once
    /// others had moved titles, or the run was stopped, and the remaining
    /// ones were abandoned.
    pub stopped_early: bool,
    /// Why the run was stopped, `null` when it was not.
    pub stopped: Option<StopReason>,
    pub errors: Vec<ApplyError>,
}

/// Apply every move a simulation proposed, in slices of `batch_limit`.
///
/// Reclassifying a whole library at fifty per batch is forty trips through the
/// confirmation dialog, which is friction rather than a guardrail. Here
/// `batch_limit` is the slice size, and the guardrail that replaces it is a
/// confirmation always required, whatever `confirmation_threshold` says.
///
/// A slice the Arr refuses whole ends the run: an Arr that rejected fifty
/// moves will likely reject the next fifty, and the untried decisions stay
/// `pending` for the next simulation to repropose. A slice in which some moved
/// is not that, and the run goes on.
pub async fn apply_simulation_in_batches(
    state: &AppState,
    simulation_id: &str,
    move_files: bool,
    confirmed: &Confirmed,
    by: &Attribution,
) -> AppResult<BatchApplyReport> {
    guard_dry_run(state).await?;

    let localizer = state.localizer().await;
    let (ids, superseded) = applicable_from_simulation(&state.pool, simulation_id).await?;
    if ids.is_empty() && superseded > 0 {
        return Err(AppError::Conflict(localizer.translate("ErrorProposalsReplaced", &[])));
    }
    if ids.is_empty() {
        return Err(AppError::BadRequest(localizer.translate("ErrorNoSelection", &[])));
    }

    // This path always asks, and a sleeping or full destination is stated
    // *in* its one question rather than asked apart: a confirmation that omits
    // one is worse than none, because it looks like it was considered. Each
    // stated guardrail is named in `includes` and has to come back beside
    // `batch`, so a caller allowed to answer the count alone cannot wave the
    // destination through with it. The interface shows the question as it
    // comes and sends every name back, so a person is asked once.
    let mut includes = Vec::new();
    let mut facts = Vec::new();
    for check in [
        guard_reachable(state, CapacityScope::Simulation(simulation_id), confirmed).await,
        guard_capacity(state, CapacityScope::Simulation(simulation_id), move_files, confirmed)
            .await,
    ] {
        match check {
            Err(AppError::ConfirmationRequired { kind, message, .. }) => {
                includes.push(kind);
                facts.push(message);
            }
            Err(other) => return Err(other),
            Ok(()) => {}
        }
    }
    if !confirmed.has(confirm::BATCH) || !includes.is_empty() {
        // Two whole sentences: a clause appended to a translated one reads
        // as English grammar in every other language.
        let asked =
            if move_files { "ConfirmApplyAllItemsWithFiles" } else { "ConfirmApplyAllItems" };
        let mut message = localizer.translate(asked, &[("count", &ids.len().to_string())]);
        for fact in facts {
            message = format!("{message}\n\n{fact}");
        }
        return Err(AppError::ConfirmationRequired { kind: confirm::BATCH, includes, message });
    }

    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(localizer.translate("ErrorApplyInProgress", &[])));
    };

    let size: usize = state.setting("batch_limit", 50usize).await.max(1);
    let batches_planned = ids.len().div_ceil(size);

    let mut job = state
        .jobs
        .start(
            JobKind::Apply,
            by,
            None,
            Detail::new("JobDetailApplyingBatches")
                .with("count", ids.len())
                .with("batches", batches_planned),
        )
        .await?;

    let state = state.clone();
    let by = by.clone();
    let stop = job.stop();
    detached(&state.jobs.clone(), async move {
        let _lock = lock;
        let mut report = BatchApplyReport {
            candidates: ids.len(),
            batches_planned,
            superseded,
            ..Default::default()
        };

        for batch in ids.chunks(size) {
            if let Some(reason) = stop_reason(&state, &stop).await {
                report.stopped = Some(reason);
                report.stopped_early = true;
                break;
            }
            // The lock is already held, so this goes straight to the writer
            // rather than through run_apply, which would try to take it again.
            let outcome = match load_pending_moves(&state.pool, batch).await {
                Ok(moves) => {
                    report.skipped += batch.len() - moves.len();
                    report.batches_run += 1;
                    let direction = MoveDirection::Forward;
                    execute_moves(&state, moves, move_files, direction, &by, &stop).await
                }
                Err(e) => Err(e),
            };

            match outcome {
                Ok(slice) => {
                    report.applied += slice.applied;
                    report.failed += slice.failed;
                    report.moving += slice.moving;
                    report.errors.extend(slice.errors);
                    if slice.stopped.is_some() {
                        report.stopped = slice.stopped;
                        report.stopped_early = true;
                        break;
                    }
                    if slice.applied == 0 && slice.moving == 0 && slice.failed > 0 {
                        report.stopped_early = true;
                        break;
                    }
                }
                // Earlier slices moved titles, so the run ends as at a refused
                // slice and reports them: a bare error would hide moves that
                // happened, and the job would carry no report of them.
                Err(e) if report.applied > 0 => {
                    error!(
                        slices = report.batches_run,
                        "An apply of a whole simulation stopped: {e}"
                    );
                    report.stopped_early = true;
                    break;
                }
                Err(e) => {
                    job.fail(&e).await;
                    return Err(e);
                }
            }

            let attempted = report.applied + report.failed + report.skipped + report.moving;
            job.progress(attempted, ids.len()).await;
        }

        job.report(&report);
        let (applied, failed, moving) = (report.applied, report.failed, report.moving);
        let detail = stopped_detail(report.stopped, applied, failed, moving).unwrap_or_else(|| {
            let key = if moving > 0 {
                "JobDetailAppliedBatchesMoving"
            } else {
                "JobDetailAppliedBatches"
            };
            Detail::new(key)
                .with("applied", applied)
                .with("failed", failed)
                .with("moving", moving)
                .with("run", report.batches_run)
                .with("planned", report.batches_planned)
        });
        close_job(job, applied + moving, failed, report.stopped, detail).await;
        notify::send_later(
            &state,
            notify::Event::MovesCompleted {
                reverted: false,
                applied,
                failed,
                skipped: report.skipped,
                moving,
            },
        );

        Ok(report)
    })
    .await
}

/// Apply decisions a background run selected, with nobody watching.
///
/// Same writer, different gate. The confirmation threshold exists to make a
/// *person* stop and look. It means nothing here, and [`auto_apply`] has already
/// established that the set is small, in scope and file-free. The dry-run guard
/// still applies: it is the one switch that must override everything.
///
/// Never moves files on disk: see [`auto_apply`] for why that is a property of
/// the feature rather than an option.
///
/// [`auto_apply`]: crate::services::auto_apply
pub async fn apply_unattended(
    state: &AppState,
    decision_ids: &[String],
    by: &Attribution,
    turn: JobLock,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    run_locked(state, decision_ids, false, by, turn, 0).await
}

/// How long an unattended apply waits for the one running. The Arr queues a
/// disk move as its own command, so an apply holds the lock for its API calls
/// and its revalidation: minutes on a large library, not hours.
const UNATTENDED_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// The apply lock for an unattended apply, waited for rather than refused:
/// dropped, the correction of a film just added would wait for a sweep that
/// may never come, while the file lands in the folder the rules do not want.
pub async fn unattended_turn(state: &AppState) -> Option<JobLock> {
    state.jobs.lock_within("apply", UNATTENDED_WAIT).await
}

/// The shared body of both apply paths: take the lock, record a job, write.
async fn run_apply(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    by: &Attribution,
    superseded: usize,
) -> AppResult<ApplyReport> {
    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(
            state.localizer().await.translate("ErrorApplyInProgress", &[]),
        ));
    };
    run_locked(state, decision_ids, move_files, by, lock, superseded).await
}

/// An apply under a lock already held: record a job, write.
async fn run_locked(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    by: &Attribution,
    lock: JobLock,
    superseded: usize,
) -> AppResult<ApplyReport> {
    let mut job = state
        .jobs
        .start(
            JobKind::Apply,
            by,
            None,
            Detail::new("JobDetailApplying").with("count", decision_ids.len()),
        )
        .await?;

    let state = state.clone();
    let ids = decision_ids.to_vec();
    let by = by.clone();
    let stop = job.stop();
    detached(&state.jobs.clone(), async move {
        let _lock = lock;
        let moves = match load_pending_moves(&state.pool, &ids).await {
            Ok(moves) => moves,
            Err(e) => {
                job.fail(&e).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let direction = MoveDirection::Forward;
        let requested = ids.len() + superseded;
        let outcome = execute_moves(&state, moves, move_files, direction, &by, &stop)
            .await
            .map(|report| ApplyReport { requested, skipped, superseded, ..report });

        match &outcome {
            Ok(report) => {
                job.report(report);
                let detail =
                    ran_detail(report, "JobDetailApplied", "JobDetailAppliedMoving", "applied");
                close_job(
                    job,
                    report.applied + report.moving,
                    report.failed,
                    report.stopped,
                    detail,
                )
                .await;
                notify::send_later(&state, moves_completed(false, report));
            }
            Err(e) => job.fail(e).await,
        }

        outcome
    })
    .await
}

/// Roll applied decisions back to the root folder they came from.
pub async fn revert_decisions(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    confirmed: &Confirmed,
    by: &Attribution,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    let decision_ids = distinct(decision_ids);
    let decision_ids = decision_ids.as_slice();
    guard_batch_limit(state, decision_ids.len()).await?;
    // A revert writes into the folders its moves came from, and asks there what
    // an apply asks of its destinations, in the same order.
    guard_reachable(state, CapacityScope::Reverting(decision_ids), confirmed).await?;
    guard_capacity(state, CapacityScope::Reverting(decision_ids), move_files, confirmed).await?;
    guard_confirmation(state, decision_ids.len(), confirmed).await?;
    refuse_unknown_decisions(state, decision_ids).await?;

    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(
            state.localizer().await.translate("ErrorApplyInProgress", &[]),
        ));
    };

    let mut job = state
        .jobs
        .start(
            JobKind::Revert,
            by,
            None,
            Detail::new("JobDetailReverting").with("count", decision_ids.len()),
        )
        .await?;

    let state = state.clone();
    let ids = decision_ids.to_vec();
    let by = by.clone();
    let stop = job.stop();
    detached(&state.jobs.clone(), async move {
        let _lock = lock;
        let moves = match load_revertible_moves(&state.pool, &ids).await {
            Ok(moves) => moves,
            Err(e) => {
                job.fail(&e).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let direction = MoveDirection::Revert;
        let outcome = execute_moves(&state, moves, move_files, direction, &by, &stop)
            .await
            .map(|report| ApplyReport { requested: ids.len(), skipped, ..report });

        match &outcome {
            Ok(report) => {
                job.report(report);
                let detail =
                    ran_detail(report, "JobDetailReverted", "JobDetailRevertedMoving", "reverted");
                close_job(
                    job,
                    report.applied + report.moving,
                    report.failed,
                    report.stopped,
                    detail,
                )
                .await;
                notify::send_later(&state, moves_completed(true, report));
            }
            Err(e) => job.fail(e).await,
        }

        outcome
    })
    .await
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MoveDirection {
    Forward,
    Revert,
}

impl MoveDirection {
    /// The `action` an execution log line records.
    fn action(self) -> &'static str {
        match self {
            Self::Forward => "move",
            Self::Revert => "revert",
        }
    }
}

/// Send each move to its Arr, title by title, instance by instance in the
/// order the moves were given.
async fn execute_moves(
    state: &AppState,
    moves: Vec<PendingMove>,
    move_files: bool,
    direction: MoveDirection,
    by: &Attribution,
    stop: &Stop,
) -> AppResult<ApplyReport> {
    let mut report = ApplyReport::default();
    let mut following = Vec::new();

    'instances: for batch in by_instance(moves) {
        let instance = match state.instance(&batch[0].instance_id).await {
            Ok(instance) => instance,
            Err(e) => {
                fail_batch(state, &batch, &mut report, &e.to_string(), direction, by).await;
                continue;
            }
        };
        // "Enabled (synced and routed)": a proposal left from before the
        // switch was turned off must not reach an instance that is neither.
        if !instance.enabled {
            let refusal = state
                .localizer()
                .await
                .translate("ErrorInstanceDisabled", &[("name", &instance.name)]);
            fail_batch(state, &batch, &mut report, &refusal, direction, by).await;
            continue;
        }

        let adapter = match state.adapter(&instance) {
            Ok(a) => a,
            Err(e) => {
                fail_batch(state, &batch, &mut report, &e.to_string(), direction, by).await;
                continue;
            }
        };

        let mut sent = Vec::new();
        for mv in &batch {
            if let Some(reason) = stop_reason(state, stop).await {
                report.stopped = Some(reason);
                if !sent.is_empty() {
                    following.push(Following { adapter, arr: instance.name.clone(), sent });
                }
                break 'instances;
            }
            let outcome = match record_requested(&state.pool, mv, direction).await {
                Ok(()) => send(&adapter, mv, move_files).await.map_err(|e| e.to_string()),
                // Unrecorded, a move the Arr made is lost to a stop or a crash.
                Err(e) => Err(e.to_string()),
            };
            match outcome {
                Ok(path) if move_files => {
                    record_sent(state, mv, &path, direction, by).await;
                    sent.push(Sent { mv: mv.clone(), path });
                }
                Ok(path) => {
                    record_success(state, mv, &path, direction, by).await;
                    report.applied += 1;
                }
                Err(message) => fail(state, mv, &message, &mut report, direction, by).await,
            }
        }
        if !sent.is_empty() {
            following.push(Following { adapter, arr: instance.name.clone(), sent });
        }
    }

    // A server stopping has no minute to give: what is left is requested,
    // and the next sync settles it.
    if report.stopped == Some(StopReason::Shutdown) {
        report.moving += following.iter().map(|instance| instance.sent.len()).sum::<usize>();
    } else {
        follow(state, following, direction, by, &mut report).await;
    }

    info!(
        "{} move(s) applied, {} failed, {} still moving",
        report.applied, report.failed, report.moving
    );
    Ok(report)
}

/// Ask the Arr for one move. A write whose answer never came may have been
/// made all the same, so the title is read back before it counts as failed.
async fn send(adapter: &ArrAdapter, mv: &PendingMove, move_files: bool) -> AppResult<String> {
    match adapter.move_item(mv.arr_id, &mv.to, move_files).await {
        Err(e @ AppError::ExternalApi { status: 0, .. }) => {
            match adapter.get_media_one(mv.arr_id).await {
                Ok(Some(held)) => {
                    held.path.filter(|path| crate::paths::within(path, &mv.to)).ok_or(e)
                }
                _ => Err(e),
            }
        }
        sent => sent,
    }
}

/// The moves of each instance, instances and moves in the order given, so the
/// Arrs are called and the errors reported in one order from run to run.
fn by_instance(moves: Vec<PendingMove>) -> Vec<Vec<PendingMove>> {
    let mut batches: Vec<Vec<PendingMove>> = Vec::new();
    for mv in moves {
        match batches.iter_mut().find(|batch| batch[0].instance_id == mv.instance_id) {
            Some(batch) => batch.push(mv),
            None => batches.push(vec![mv]),
        }
    }
    batches
}

/// What the webhook is told when an apply or a revert finishes.
fn moves_completed(reverted: bool, report: &ApplyReport) -> notify::Event {
    notify::Event::MovesCompleted {
        reverted,
        applied: report.applied,
        failed: report.failed,
        skipped: report.skipped,
        moving: report.moving,
    }
}

/// Close a job on what it did. A cancelled run is cancelled. Nothing done
/// while something failed is a failure, whatever the count reads, or the
/// Tasks screen shows it in green.
async fn close_job(
    job: crate::jobs::JobHandle,
    done: usize,
    failed: usize,
    stopped: Option<StopReason>,
    detail: Detail,
) {
    match stopped {
        Some(StopReason::Cancelled) => job.cancelled(detail).await,
        _ if done == 0 && failed > 0 => job.fail_with(detail).await,
        _ => job.succeed(detail).await,
    }
}

/// Why the run must end before its next move: the server stopping, a
/// cancel, or the global dry-run turned on, the switch that overrides
/// everything.
async fn stop_reason(state: &AppState, stop: &Stop) -> Option<StopReason> {
    if stop.closing() {
        Some(StopReason::Shutdown)
    } else if stop.cancelled() {
        Some(StopReason::Cancelled)
    } else if state.bool_setting("global_dry_run", true).await {
        Some(StopReason::DryRun)
    } else {
        None
    }
}

/// What a run says on the Tasks screen when it stopped before its end.
fn stopped_detail(
    stopped: Option<StopReason>,
    done: usize,
    failed: usize,
    moving: usize,
) -> Option<Detail> {
    let key = match stopped? {
        StopReason::DryRun => "JobDetailStoppedByDryRun",
        StopReason::Cancelled => "JobDetailCancelled",
        StopReason::Shutdown => "JobDetailStoppedForShutdown",
    };
    Some(Detail::new(key).with("done", done).with("failed", failed).with("moving", moving))
}

/// What an apply or a revert says on the Tasks screen, `done` naming its
/// count in the key's own words.
fn ran_detail(
    report: &ApplyReport,
    key: &'static str,
    key_moving: &'static str,
    done: &str,
) -> Detail {
    stopped_detail(report.stopped, report.applied, report.failed, report.moving).unwrap_or_else(
        || {
            let key = if report.moving > 0 { key_moving } else { key };
            Detail::new(key)
                .with(done, report.applied)
                .with("failed", report.failed)
                .with("moving", report.moving)
        },
    )
}
