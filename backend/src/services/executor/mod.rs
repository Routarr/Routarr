//! The only code that writes to Radarr or Sonarr. Its guardrails, in order: global dry-run,
//! batch limit, the destinations' reachability and free space, confirmation past a threshold,
//! and revalidation just before the call. One call per title keeps its folder name.

mod follow;
mod guards;
mod load;
mod matching;
mod record;

use tracing::{error, info};

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::arr_moves::Repointed;
use crate::jobs::registry::JobLock;
use crate::jobs::{Attribution, Detail, JobKind, Stop, detached};
use crate::localization::Told;
use crate::models::DecisionFilter;
use crate::services::notify;
use crate::state::AppState;

pub use follow::settle_requested;
pub(crate) use guards::REVERTIBLE;
pub use guards::{Confirmed, confirm};
pub use matching::RevertScope;

use crate::services::connection::Said;
use follow::{Following, Sent, follow};
use guards::{
    CapacityScope, guard_batch_limit, guard_capacity, guard_confirmation, guard_dry_run,
    guard_reachable,
};
use load::{
    applicable_from_simulation, current_proposals, distinct, load_pending_moves,
    load_revertible_moves, refuse_unknown_decisions, revertible_matching,
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
    /// Selected decisions a newer simulation replaced with a different move,
    /// not applied. One replaced by the same move is applied through it.
    pub superseded: usize,
    /// Why the run ended before its last move, `null` when it ran through.
    /// What was not attempted is still pending, or still applied for a revert.
    pub stopped: Option<StopReason>,
    pub errors: Vec<ApplyError>,
    /// The first titles the run moved or tried to, which a notification names.
    #[serde(skip)]
    pub moved: Vec<Moved>,
}

/// How many titles a report keeps for a notification: past it, the task's log has the rest.
pub const MOVED_LISTED: usize = 50;

impl ApplyReport {
    fn note(&mut self, moved: Moved) {
        if self.moved.len() < MOVED_LISTED {
            self.moved.push(moved);
        }
    }
}

/// One title a run moved, or tried to, as a notification names it for a receiver to rescan its
/// folders. The paths are the Arr's, which a media server may mount elsewhere.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Moved {
    pub title: String,
    pub media_type: String,
    pub instance: Option<String>,
    pub instance_id: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub from_path: Option<String>,
    pub to_path: Option<String>,
    /// `applied`, `moving` while the Arr still carries the files, or `failed`.
    pub status: &'static str,
}

impl Moved {
    fn of(
        mv: &PendingMove,
        instance: Option<&str>,
        from: Option<String>,
        to: Option<String>,
        status: &'static str,
    ) -> Self {
        Self {
            title: mv.media_title.clone(),
            media_type: mv.media_type.clone(),
            instance: instance.map(str::to_string),
            instance_id: mv.instance_id.clone(),
            tmdb_id: mv.tmdb_id,
            tvdb_id: mv.tvdb_id,
            imdb_id: mv.imdb_id.clone(),
            from_path: from,
            to_path: to,
            status,
        }
    }
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
    /// `message` as a column stores it, which a notification says in its own
    /// language.
    #[serde(skip)]
    pub stored: String,
}

/// Columns shared by the apply and revert loaders.
type MoveRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

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
    media_type: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
}

impl From<MoveRow> for PendingMove {
    fn from(row: MoveRow) -> Self {
        let (decision_id, media_id, media_title, instance_id, from, to, arr_id) =
            (row.0, row.1, row.2, row.3, row.4, row.5, row.6);
        let (media_type, tmdb_id, tvdb_id, imdb_id) = (row.7, row.8, row.9, row.10);
        Self {
            decision_id,
            media_id,
            media_title,
            instance_id,
            arr_id,
            from,
            to,
            media_type,
            tmdb_id,
            tvdb_id,
            imdb_id,
        }
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
        return Err(AppError::Conflict {
            reason: "proposals_replaced",
            message: localizer.translate("ErrorProposalsReplaced", &[]),
        });
    }
    let decision_ids = current.as_slice();
    // A count first, so no list too long to bind reaches the guards that spell the ids out.
    guard_batch_limit(state, decision_ids.len()).await?;
    // Then the gravest first: a destination not answering, short of room, then the threshold.
    guard_reachable(state, CapacityScope::Decisions(decision_ids), confirmed).await?;
    guard_capacity(state, CapacityScope::Decisions(decision_ids), move_files, confirmed).await?;
    guard_confirmation(state, decision_ids.len(), confirmed).await?;
    refuse_unknown_decisions(state, decision_ids).await?;
    run_apply(state, decision_ids, move_files, by, superseded).await
}

/// Outcome of a run in slices: a whole simulation applied, or the moves a filter matches
/// reverted.
#[derive(Debug, Default, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchApplyReport {
    /// Everything the run covered that was still applicable, or still revertible.
    pub candidates: usize,
    /// The moves made, or undone for a revert.
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Slices actually attempted, fewer than planned when one of them failed.
    pub batches_run: usize,
    pub batches_planned: usize,
    /// Moves of files the Arr was still making when the run stopped waiting,
    /// as in `ApplyReport`.
    pub moving: usize,
    /// Decisions of the simulation a newer one replaced with a different
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

/// Apply every move a simulation proposed that `filter` keeps, in slices of `batch_limit`.
/// Batch by batch, a whole library is dozens of trips through the dialog, so one confirmation
/// is always asked instead, whatever `confirmation_threshold` says.
pub async fn apply_simulation_in_batches(
    state: &AppState,
    simulation_id: &str,
    filter: &DecisionFilter,
    move_files: bool,
    confirmed: &Confirmed,
    by: &Attribution,
) -> AppResult<BatchApplyReport> {
    guard_dry_run(state).await?;

    let localizer = state.localizer().await;
    let (ids, superseded) = applicable_from_simulation(&state.pool, simulation_id, filter).await?;
    if ids.is_empty() && superseded > 0 {
        return Err(AppError::Conflict {
            reason: "proposals_replaced",
            message: localizer.translate("ErrorProposalsReplaced", &[]),
        });
    }
    if ids.is_empty() {
        return Err(AppError::BadRequest(localizer.translate("ErrorNoSelection", &[])));
    }

    let terms = matching::terms(state, filter, &[], &localizer).await?;
    let asked = if move_files { "ConfirmApplyAllItemsWithFiles" } else { "ConfirmApplyAllItems" };
    let scope = CapacityScope::Simulation(simulation_id, filter);
    let question = Question { asked, count: ids.len(), terms };
    ask_once(state, scope, move_files, confirmed, question).await?;

    let lock = apply_lock(state).await?;
    run_in_slices(state, lock, ids, superseded, MoveDirection::Forward, move_files, by).await
}

/// Undo every applied move `scope` matches, in slices of `batch_limit`, asked as an apply
/// of a whole simulation is.
pub async fn revert_in_batches(
    state: &AppState,
    scope: &RevertScope,
    move_files: bool,
    confirmed: &Confirmed,
    by: &Attribution,
) -> AppResult<BatchApplyReport> {
    guard_dry_run(state).await?;

    let localizer = state.localizer().await;
    let matching = scope.conditions();
    let ids = revertible_matching(&state.pool, &matching).await?;
    if ids.is_empty() {
        return Err(AppError::BadRequest(localizer.translate("ErrorNothingToRevert", &[])));
    }

    let terms = scope.terms(state, &localizer).await?;
    let asked = if move_files { "ConfirmRevertAllItemsWithFiles" } else { "ConfirmRevertAllItems" };
    let question = Question { asked, count: ids.len(), terms };
    let weighed = CapacityScope::RevertingMatching(&matching);
    ask_once(state, weighed, move_files, confirmed, question).await?;

    let lock = apply_lock(state).await?;
    run_in_slices(state, lock, ids, 0, MoveDirection::Revert, move_files, by).await
}

/// What a run in slices asks: its key, how many moves, and the filter that chose them.
struct Question {
    asked: &'static str,
    count: usize,
    terms: Vec<String>,
}

/// Ask a run in slices its one question, always, stating a sleeping or full destination in it,
/// since a confirmation that omits one looks considered. Each is named in `includes` and must
/// come back beside `batch`, so a key allowed the count alone cannot wave it through.
async fn ask_once(
    state: &AppState,
    scope: CapacityScope<'_>,
    move_files: bool,
    confirmed: &Confirmed,
    question: Question,
) -> AppResult<()> {
    let mut includes = Vec::new();
    let mut facts = Vec::new();
    for check in [
        guard_reachable(state, scope, confirmed).await,
        guard_capacity(state, scope, move_files, confirmed).await,
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
    if confirmed.has(confirm::BATCH) && includes.is_empty() {
        return Ok(());
    }
    // Whole sentences: a clause appended to a translated one reads as English grammar.
    let localizer = state.localizer().await;
    let mut message =
        localizer.translate(question.asked, &[("count", &question.count.to_string())]);
    if !question.terms.is_empty() {
        let terms = question.terms.join(&localizer.translate("ListSeparator", &[]));
        let filtered = localizer.translate("ConfirmFilteredBy", &[("terms", &terms)]);
        message = format!("{message}\n\n{filtered}");
    }
    for fact in facts {
        message = format!("{message}\n\n{fact}");
    }
    Err(AppError::ConfirmationRequired { kind: confirm::BATCH, includes, message })
}

/// The apply lock, refused while another apply or revert holds it.
async fn apply_lock(state: &AppState) -> AppResult<JobLock> {
    match state.jobs.try_lock("apply") {
        Some(lock) => Ok(lock),
        None => Err(AppError::InProgress {
            reason: "apply_running",
            message: state.localizer().await.translate("ErrorApplyInProgress", &[]),
        }),
    }
}

/// Write `ids` slice by slice under the lock already held, as one job. A slice the Arr refuses
/// whole ends the run, since the next would likely fail too, and leaves the untried decisions as
/// they were. A slice in which some moved does not.
async fn run_in_slices(
    state: &AppState,
    lock: JobLock,
    ids: Vec<String>,
    superseded: usize,
    direction: MoveDirection,
    move_files: bool,
    by: &Attribution,
) -> AppResult<BatchApplyReport> {
    let size: usize = state.bounding_setting::<usize>("batch_limit").await?.max(1);
    let batches_planned = ids.len().div_ceil(size);

    let started = match direction {
        MoveDirection::Forward => (JobKind::Apply, "JobDetailApplyingBatches"),
        MoveDirection::Revert => (JobKind::Revert, "JobDetailRevertingBatches"),
    };
    let detail = Detail::new(started.1).with("count", ids.len()).with("batches", batches_planned);
    let mut job = state.jobs.start(started.0, by, None, detail).await?;

    let state = state.clone();
    let task = job.id.clone();
    let by = by.in_job(&task);
    let stop = job.stop();
    detached(&state.jobs.clone(), async move {
        let _lock = lock;
        let mut moved = Vec::new();
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
            let loaded = match direction {
                MoveDirection::Forward => load_pending_moves(&state.pool, batch).await,
                MoveDirection::Revert => load_revertible_moves(&state.pool, batch).await,
            };
            let outcome = match loaded {
                Ok(moves) => {
                    report.skipped += batch.len() - moves.len();
                    report.batches_run += 1;
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
                    let room = MOVED_LISTED.saturating_sub(moved.len());
                    moved.extend(slice.moved.into_iter().take(room));
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
                // Earlier slices wrote to the Arr, which a bare error would leave unreported.
                Err(e) if report.applied + report.moving + report.failed > 0 => {
                    error!(slices = report.batches_run, "A run in slices stopped: {e}");
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
            let (key, done) = match (direction, moving > 0) {
                (MoveDirection::Forward, false) => ("JobDetailAppliedBatches", "applied"),
                (MoveDirection::Forward, true) => ("JobDetailAppliedBatchesMoving", "applied"),
                (MoveDirection::Revert, false) => ("JobDetailRevertedBatches", "reverted"),
                (MoveDirection::Revert, true) => ("JobDetailRevertedBatchesMoving", "reverted"),
            };
            Detail::new(key)
                .with(done, applied)
                .with("failed", failed)
                .with("moving", moving)
                .with("run", report.batches_run)
                .with("planned", report.batches_planned)
        });
        close_job(job, applied + moving, failed, report.stopped, detail).await;
        notify::send_later(
            &state,
            notify::Event::MovesCompleted {
                reverted: direction == MoveDirection::Revert,
                applied,
                failed,
                skipped: report.skipped,
                moving,
                task_id: task,
                moves: moved,
            },
        );

        Ok(report)
    })
    .await
}

/// Apply decisions a background run selected, never moving files. No threshold, which is for a
/// person, since [`crate::services::auto_apply`] already bounds the set. The dry-run still holds.
pub async fn apply_unattended(
    state: &AppState,
    decision_ids: &[String],
    by: &Attribution,
    turn: JobLock,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    run_locked(state, decision_ids, false, by, turn, 0).await
}

/// How long an unattended apply waits for the one running. The Arr carries a disk move as its
/// own command, followed for `move_wait` at most, so an apply holds the lock minutes, not hours.
const UNATTENDED_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// The apply lock for an unattended apply, waited for rather than refused: dropped, the fix for
/// a film just added waits for a sweep that may never come, in a folder the rules do not want.
pub async fn unattended_turn(state: &AppState) -> Option<JobLock> {
    state.jobs.lock_within("apply", UNATTENDED_WAIT).await
}

/// Take the apply lock, refused while another run holds it, then apply under it.
async fn run_apply(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    by: &Attribution,
    superseded: usize,
) -> AppResult<ApplyReport> {
    let lock = apply_lock(state).await?;
    run_locked(state, decision_ids, move_files, by, lock, superseded).await
}

/// The body both apply paths share, under a lock already held: record a job, write.
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
    let task = job.id.clone();
    let by = by.in_job(&task);
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
                notify::send_later(&state, moves_completed(false, report, &task));
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
    // A revert asks of the folders its moves came from what an apply asks, in the same order.
    guard_reachable(state, CapacityScope::Reverting(decision_ids), confirmed).await?;
    guard_capacity(state, CapacityScope::Reverting(decision_ids), move_files, confirmed).await?;
    guard_confirmation(state, decision_ids.len(), confirmed).await?;
    refuse_unknown_decisions(state, decision_ids).await?;

    let lock = apply_lock(state).await?;

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
    let task = job.id.clone();
    let by = by.in_job(&task);
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
                notify::send_later(&state, moves_completed(true, report, &task));
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

/// Send each move to its Arr, title by title, instance by instance in the order given.
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
    let localizer = state.localizer().await;

    'instances: for batch in by_instance(moves) {
        let instance = match state.instance(&batch[0].instance_id).await {
            Ok(instance) => instance,
            Err(e) => {
                let failed = Said::of(&e, None, &localizer);
                fail_batch(state, &batch, &mut report, &failed, direction, by).await;
                note_failed(&mut report, &batch, None);
                continue;
            }
        };
        let named = Some(instance.name.as_str());
        // A proposal left from before the instance was disabled must not reach it.
        if !instance.enabled {
            let refusal = Told::new("ErrorInstanceDisabled").with("name", &instance.name);
            let failed = Said::told(&refusal, &localizer);
            fail_batch(state, &batch, &mut report, &failed, direction, by).await;
            note_failed(&mut report, &batch, named);
            continue;
        }

        let adapter = match state.adapter(&instance) {
            Ok(a) => a,
            Err(e) => {
                let failed = Said::of(&e, Some(&instance), &localizer);
                fail_batch(state, &batch, &mut report, &failed, direction, by).await;
                note_failed(&mut report, &batch, named);
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
                Ok(()) => send(&adapter, mv, move_files)
                    .await
                    .map_err(|e| Said::of(&e, Some(&instance), &localizer)),
                // Unrecorded, a move the Arr made is lost to a stop or a crash.
                Err(e) => Err(Said::of(&e, None, &localizer)),
            };
            match outcome {
                Ok(Repointed { from, to: path }) if move_files => {
                    record_sent(state, mv, &path, direction, by).await;
                    sent.push(Sent { mv: mv.clone(), path, from });
                }
                Ok(Repointed { from, to }) => {
                    record_success(state, mv, &to, direction, by).await;
                    report.applied += 1;
                    report.note(Moved::of(mv, named, from, Some(to), "applied"));
                }
                Err(failed) => {
                    fail(state, mv, &failed, &mut report, direction, by).await;
                    report.note(Moved::of(mv, named, None, None, "failed"));
                }
            }
        }
        if !sent.is_empty() {
            following.push(Following { adapter, arr: instance.name.clone(), sent });
        }
    }

    // A stopping server has no minute to give: what is left stays requested for the next sync.
    if report.stopped == Some(StopReason::Shutdown) {
        for instance in &following {
            for sent in &instance.sent {
                report.moving += 1;
                report.note(sent.moved(&instance.arr, "moving"));
            }
        }
    } else {
        follow(state, following, direction, by, &mut report).await;
    }

    info!(
        "{} move(s) applied, {} failed, {} still moving",
        report.applied, report.failed, report.moving
    );
    Ok(report)
}

/// Ask the Arr for one move. A write whose answer never came may have been made all the same,
/// so the title is read back before it counts as failed.
async fn send(adapter: &ArrAdapter, mv: &PendingMove, move_files: bool) -> AppResult<Repointed> {
    match adapter.move_item(mv.arr_id, &mv.to, move_files).await {
        Err(e @ AppError::ExternalApi { status: 0, .. }) => {
            match adapter.get_media_one(mv.arr_id).await {
                Ok(Some(held)) => held
                    .path
                    .filter(|path| crate::paths::within(path, &mv.to))
                    .map(|to| Repointed { from: None, to })
                    .ok_or(e),
                _ => Err(e),
            }
        }
        sent => sent,
    }
}

/// The moves of each instance, instances and moves in the order given, so the Arrs are called
/// and the errors reported in one order from run to run.
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

/// Each move of `batch` as a failure a notification names.
fn note_failed(report: &mut ApplyReport, batch: &[PendingMove], instance: Option<&str>) {
    for mv in batch {
        report.note(Moved::of(mv, instance, None, None, "failed"));
    }
}

/// The notification an apply or a revert sends when it finishes.
fn moves_completed(reverted: bool, report: &ApplyReport, task: &str) -> notify::Event {
    notify::Event::MovesCompleted {
        reverted,
        applied: report.applied,
        failed: report.failed,
        skipped: report.skipped,
        moving: report.moving,
        task_id: task.to_string(),
        moves: report.moved.clone(),
    }
}

/// Nothing done while something failed is a failure, or the Tasks screen shows it in green.
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

/// Why the run must end before its next move: the server stopping, a cancel, or the global
/// dry-run turned on, the switch that overrides everything.
async fn stop_reason(state: &AppState, stop: &Stop) -> Option<StopReason> {
    if stop.closing() {
        Some(StopReason::Shutdown)
    } else if stop.cancelled() {
        Some(StopReason::Cancelled)
    } else if state.bool_setting("global_dry_run").await {
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

/// What an apply or a revert says on the Tasks screen, `done` naming its count in the key's
/// own words.
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
