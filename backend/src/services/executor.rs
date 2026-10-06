//! The only code that writes to Radarr or Sonarr.
//!
//! Guardrails, in order: global dry-run, batch limit, the destinations'
//! reachability and free space, explicit confirmation past a threshold, and
//! per-decision revalidation right before the call. Each title is its own
//! call, which keeps its folder name and settles it on its own answer.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::{ArrAdapter, ArrMedia};
use crate::integrations::arr_moves::{CommandState, MoveCommand};
use crate::jobs::registry::JobLock;
use crate::jobs::{Attribution, Detail, JobKind, detached};
use crate::services::notify;
use crate::services::routing::{self, format_timestamp};
use crate::state::AppState;

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
    pub errors: Vec<ApplyError>,
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

/// The names a guardrail asks under.
///
/// A caller that has looked at one refusal sends its name back, and only that
/// one is lifted. Written as constants rather than as literals at each site
/// because the string crosses the wire and comes back: a typo on one side is a
/// guardrail that can never be answered.
pub mod confirm {
    /// The destination cannot hold the plan.
    pub const CAPACITY: &str = "capacity";
    /// More decisions than the configured threshold.
    pub const THRESHOLD: &str = "threshold";
    /// A whole simulation, whose one question already states both facts above.
    pub const BATCH: &str = "batch";
    /// The destination is not answering right now.
    pub const UNREACHABLE: &str = "unreachable";

    /// Every name above: what an application key may be allowed to answer,
    /// and what `Confirmed::all()` builds from in tests. Written once, because
    /// a guardrail added to the module and forgotten in a second list is one
    /// no key can ever be given, and one the tests never answer.
    pub const ALL: &[&str] = &[CAPACITY, THRESHOLD, BATCH, UNREACHABLE];
}

/// The guardrails the caller has looked at and accepts: `capacity`,
/// `threshold`, `batch` or `unreachable`.
// A list of names, never a boolean: with one flag read by every guardrail
// that asks, answering one question answers them all, and confirming a
// capacity shortfall silently waves the batch threshold through as well.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct Confirmed(Vec<String>);

impl Confirmed {
    /// Nothing answered yet, for tests that assert the first question asked.
    #[cfg(test)]
    pub fn none() -> Self {
        Self::default()
    }

    pub fn has(&self, kind: &str) -> bool {
        self.0.iter().any(|answered| answered == kind)
    }

    /// The answers `keep` accepts, the others dropped as if never sent.
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self(self.0.iter().filter(|answered| keep(answered)).cloned().collect())
    }

    /// Every question answered, for tests, which assert what happens *after*
    /// the asking. Not available to the application, where it would be the
    /// blanket flag this type exists to prevent.
    #[cfg(test)]
    pub fn all() -> Self {
        Self(confirm::ALL.iter().map(|kind| (*kind).to_string()).collect())
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
    run_apply(state, decision_ids, move_files, by).await
}

/// Refuse ids that name no decision, before a task starts: a run over nothing
/// answering success tells a script its call worked. In English, since the
/// interface sent the ids and no one typed them. After the guardrails, which
/// ask about the moves that exist.
async fn refuse_unknown_decisions(state: &AppState, decision_ids: &[String]) -> AppResult<()> {
    let mut known = std::collections::HashSet::new();
    for chunk in decision_ids.chunks(routing::BIND_CHUNK) {
        let sql = format!(
            "SELECT id FROM decisions WHERE id IN ({})",
            crate::db::placeholders(chunk.len())
        );
        let mut query = sqlx::query_scalar::<_, String>(AssertSqlSafe(sql.as_str()));
        for id in chunk {
            query = query.bind(id);
        }
        known.extend(query.fetch_all(&state.pool).await?);
    }
    let unknown: Vec<&str> =
        decision_ids.iter().filter(|id| !known.contains(*id)).map(String::as_str).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(AppError::BadRequest(format!("No decision has the id {}.", unknown.join(", "))))
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
    /// True when the Arr refused a slice whole, or one could not be read once
    /// others had moved titles, and the remaining ones were abandoned.
    pub stopped_early: bool,
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
    let ids = applicable_from_simulation(&state.pool, simulation_id).await?;
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
    detached(async move {
        let _lock = lock;
        let mut report =
            BatchApplyReport { candidates: ids.len(), batches_planned, ..Default::default() };
        // One for the run: a slice reloads the routing context only when what
        // it reads changed since the previous one.
        let mut revalidation = routing::Revalidation::default();

        for batch in ids.chunks(size) {
            // The lock is already held, so this goes straight to the writer
            // rather than through run_apply, which would try to take it again.
            let outcome = match load_pending_moves(&state.pool, batch, &mut revalidation).await {
                Ok(moves) => {
                    report.skipped += batch.len() - moves.len();
                    report.batches_run += 1;
                    execute_moves(&state, moves, move_files, MoveDirection::Forward, &by).await
                }
                Err(e) => Err(e),
            };

            match outcome {
                Ok(slice) => {
                    report.applied += slice.applied;
                    report.failed += slice.failed;
                    report.errors.extend(slice.errors);
                    if slice.applied == 0 && slice.failed > 0 {
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

            job.progress(report.applied + report.failed + report.skipped, ids.len()).await;
        }

        job.report(&report);
        close_job(
            job,
            report.applied,
            report.failed,
            Detail::new("JobDetailAppliedBatches")
                .with("applied", report.applied)
                .with("failed", report.failed)
                .with("run", report.batches_run)
                .with("planned", report.batches_planned),
        )
        .await;
        notify::send_later(
            &state,
            notify::Event::MovesCompleted {
                reverted: false,
                applied: report.applied,
                failed: report.failed,
                skipped: report.skipped,
            },
        );

        Ok(report)
    })
    .await
}

/// The moves a simulation proposed that are still worth attempting.
async fn applicable_from_simulation(
    pool: &SqlitePool,
    simulation_id: &str,
) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM decisions
          WHERE simulation_id = ?
            AND status = 'pending'
            AND superseded = 0
            AND action = 'move'
            AND target_root_folder IS NOT NULL
          ORDER BY instance_id, target_root_folder, media_title",
    )
    .bind(simulation_id)
    .fetch_all(pool)
    .await?)
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
    run_locked(state, decision_ids, false, by, turn).await
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
) -> AppResult<ApplyReport> {
    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(
            state.localizer().await.translate("ErrorApplyInProgress", &[]),
        ));
    };
    run_locked(state, decision_ids, move_files, by, lock).await
}

/// An apply under a lock already held: record a job, write.
async fn run_locked(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    by: &Attribution,
    lock: JobLock,
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
    detached(async move {
        let _lock = lock;
        let moves = match load_pending_moves(
            &state.pool,
            &ids,
            &mut routing::Revalidation::default(),
        )
        .await
        {
            Ok(moves) => moves,
            Err(e) => {
                job.fail(&e).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let outcome = execute_moves(&state, moves, move_files, MoveDirection::Forward, &by)
            .await
            .map(|report| ApplyReport { requested: ids.len(), skipped, ..report });

        match &outcome {
            Ok(report) => {
                job.report(report);
                let detail = Detail::new("JobDetailApplied")
                    .with("applied", report.applied)
                    .with("failed", report.failed);
                close_job(job, report.applied, report.failed, detail).await;
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
    detached(async move {
        let _lock = lock;
        let moves = match load_revertible_moves(&state.pool, &ids).await {
            Ok(moves) => moves,
            Err(e) => {
                job.fail(&e).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let outcome = execute_moves(&state, moves, move_files, MoveDirection::Revert, &by)
            .await
            .map(|report| ApplyReport { requested: ids.len(), skipped, ..report });

        match &outcome {
            Ok(report) => {
                job.report(report);
                let detail = Detail::new("JobDetailReverted")
                    .with("reverted", report.applied)
                    .with("failed", report.failed);
                close_job(job, report.applied, report.failed, detail).await;
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
) -> AppResult<ApplyReport> {
    let mut report = ApplyReport::default();
    let mut following = Vec::new();

    for batch in by_instance(moves) {
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

    follow(state, following, direction, by, &mut report).await;

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

/// A move with files the Arr took, carried in a command of its own.
struct Sent {
    mv: PendingMove,
    path: String,
}

/// The moves with files one instance took, followed together.
struct Following {
    adapter: ArrAdapter,
    /// The instance's name, for a reason that names it.
    arr: String,
    sent: Vec<Sent>,
}

/// Follow the moves of files the Arrs took until each command ends, then read
/// each title back: in its target the move is applied, anywhere else the Arr
/// undid it. One still running at the deadline stays `requested`.
async fn follow(
    state: &AppState,
    mut following: Vec<Following>,
    direction: MoveDirection,
    by: &Attribution,
    report: &mut ApplyReport,
) {
    if following.is_empty() {
        return;
    }
    let wait = state.config.move_wait;
    let deadline = tokio::time::Instant::now() + wait;
    let pause = (wait / 20).clamp(Duration::from_millis(50), Duration::from_secs(2));
    let localizer = state.localizer().await;
    loop {
        for instance in &mut following {
            if instance.sent.is_empty() {
                continue;
            }
            let commands = match instance.adapter.move_commands().await {
                Ok(commands) => commands,
                Err(e) => {
                    warn!(arr = %instance.arr, "The Arr's commands could not be read: {e}");
                    continue;
                }
            };
            let mut running = Vec::new();
            for sent in std::mem::take(&mut instance.sent) {
                let command = latest_move(&commands, sent.mv.arr_id, Some(&sent.path));
                let held = match command.map(|c| &c.state) {
                    Some(CommandState::Running) => {
                        running.push(sent);
                        continue;
                    }
                    Some(CommandState::Ended(_)) => None,
                    _ => match instance.adapter.get_media_one(sent.mv.arr_id).await {
                        Ok(held) => Some(held),
                        // Read again at the next round, or by the next sync.
                        Err(_) => {
                            running.push(sent);
                            continue;
                        }
                    },
                };
                let target = &sent.mv.to;
                match undone(&localizer, &instance.arr, command, held.as_ref(), target, UNDONE) {
                    None => {
                        record_confirmed(&state.pool, &sent.mv, direction).await;
                        report.applied += 1;
                    }
                    Some(reason) => {
                        record_failure(state, &sent.mv, &reason, direction, by).await;
                        if let Some(Some(held)) = &held {
                            record_held(&state.pool, &sent.mv, held).await;
                        }
                        count_failure(report, &sent.mv, reason);
                    }
                }
            }
            instance.sent = running;
        }
        let left: usize = following.iter().map(|instance| instance.sent.len()).sum();
        if left == 0 {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            report.moving += left;
            return;
        }
        tokio::time::sleep(pause).await;
    }
}

/// The latest command moving title `item`, to `path` when it names one: a
/// title moved and moved back within minutes has both commands listed.
fn latest_move<'a>(
    commands: &'a [MoveCommand],
    item: i64,
    path: Option<&str>,
) -> Option<&'a MoveCommand> {
    commands
        .iter()
        .filter(|command| command.item == item)
        .filter(|command| match (&command.destination, path) {
            (Some(destination), Some(path)) => crate::paths::same(destination, path),
            _ => true,
        })
        .max_by_key(|command| command.id)
}

/// Why a move the Arr took did not happen, `None` when it did: its command
/// failed, or the title is elsewhere than `target`, which `elsewhere` words,
/// or gone. `held` is the title as the Arr holds it now, `Some(None)` once it
/// holds it no more.
fn undone(
    localizer: &crate::localization::Localizer,
    arr: &str,
    command: Option<&MoveCommand>,
    held: Option<&Option<ArrMedia>>,
    target: &str,
    elsewhere: &str,
) -> Option<String> {
    if let Some(MoveCommand { state: CommandState::Ended(status), message, .. }) = command {
        return Some(localizer.translate(
            "ErrorMoveCommandEnded",
            &[("arr", arr), ("status", status), ("message", message.as_deref().unwrap_or("-"))],
        ));
    }
    match held {
        Some(None) => Some(localizer.translate("ErrorMoveTitleGone", &[("arr", arr)])),
        Some(Some(media)) => {
            let path = media.path.as_deref().unwrap_or_default();
            (!crate::paths::within(path, target))
                .then(|| localizer.translate(elsewhere, &[("arr", arr), ("path", path)]))
        }
        None => None,
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

async fn fail_batch(
    state: &AppState,
    batch: &[PendingMove],
    report: &mut ApplyReport,
    message: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    for mv in batch {
        fail(state, mv, message, report, direction, by).await;
    }
}

async fn fail(
    state: &AppState,
    mv: &PendingMove,
    message: &str,
    report: &mut ApplyReport,
    direction: MoveDirection,
    by: &Attribution,
) {
    record_failure(state, mv, message, direction, by).await;
    count_failure(report, mv, message.to_string());
}

fn count_failure(report: &mut ApplyReport, mv: &PendingMove, message: String) {
    report.failed += 1;
    report.errors.push(ApplyError {
        decision_id: mv.decision_id.clone(),
        media_title: mv.media_title.clone(),
        message,
    });
}

/// The move about to be sent, recorded first: a stop or a crash between the
/// Arr's answer and the record then leaves a decision the next sync settles
/// from what the Arr holds, rather than a move nobody recorded.
async fn record_requested(
    pool: &SqlitePool,
    mv: &PendingMove,
    direction: MoveDirection,
) -> AppResult<()> {
    let now = format_timestamp(chrono::Utc::now());
    let requested = match direction {
        MoveDirection::Forward => sqlx::query(
            "UPDATE decisions SET status = 'requested', error_message = NULL, applied_at = ?
             WHERE id = ?",
        ),
        MoveDirection::Revert => {
            sqlx::query("UPDATE decisions SET status = 'requested', reverted_at = ? WHERE id = ?")
        }
    };
    requested.bind(now).bind(&mv.decision_id).execute(pool).await?;
    Ok(())
}

/// A move with files the Arr took. It holds the new path already and moves
/// the files in a command of its own, so the title's row follows the Arr while
/// the decision stays `requested` until the command is seen to end.
async fn record_sent(
    state: &AppState,
    mv: &PendingMove,
    path: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    let now = format_timestamp(chrono::Utc::now());
    let moved = sqlx::query(
        "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
    )
    .bind(&mv.to)
    .bind(path)
    .bind(&now)
    .bind(&mv.media_id)
    .execute(&state.pool)
    .await;
    if let Err(e) = moved {
        error!(decision = %mv.decision_id, "The Arr took the move but recording it failed: {e}");
    }
    log_execution(&state.pool, by, direction.action(), &moved_between(mv), true, None, mv).await;
}

/// A requested move the Arr was seen to finish.
async fn record_confirmed(pool: &SqlitePool, mv: &PendingMove, direction: MoveDirection) {
    let status = match direction {
        MoveDirection::Forward => "applied",
        MoveDirection::Revert => "skipped",
    };
    let confirmed =
        sqlx::query("UPDATE decisions SET status = ? WHERE id = ? AND status = 'requested'")
            .bind(status)
            .bind(&mv.decision_id)
            .execute(pool)
            .await;
    if let Err(e) = confirmed {
        error!(decision = %mv.decision_id, "Recording a finished move failed: {e}");
    }
}

/// The title's row as the Arr holds it after undoing a move.
async fn record_held(pool: &SqlitePool, mv: &PendingMove, held: &ArrMedia) {
    let now = format_timestamp(chrono::Utc::now());
    let written = sqlx::query(
        "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
    )
    .bind(&held.root_folder_path)
    .bind(&held.path)
    .bind(now)
    .bind(&mv.media_id)
    .execute(pool)
    .await;
    if let Err(e) = written {
        error!(decision = %mv.decision_id, "Recording where the Arr holds the title failed: {e}");
    }
}

/// What the log says a move did.
fn moved_between(mv: &PendingMove) -> String {
    format!("{} → {}", mv.from.as_deref().unwrap_or("(unknown)"), mv.to)
}

/// Mark the decision applied *and* update the local media row.
///
/// Without the second write the next simulation would keep proposing the same
/// move until the following sync, and the UI would show the media as misplaced.
async fn record_success(
    state: &AppState,
    mv: &PendingMove,
    new_path: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    let now = format_timestamp(chrono::Utc::now());
    let pool = &state.pool;

    // The decision and the media row in one transaction: an `applied` decision
    // beside a stale path reproposes a move that already happened.
    if let Err(e) = record_outcome(pool, mv, new_path, &now, direction).await {
        error!(decision = %mv.decision_id, "The move succeeded but recording it failed: {e}");
    }

    log_execution(pool, by, direction.action(), &moved_between(mv), true, None, mv).await;
}

/// The two rows a successful move changes, written together.
///
/// `moved_at` is what stops a synchronisation that read the Arr *before* this
/// move from putting the old path back (`upsert_media` in `services/sync.rs`).
async fn record_outcome(
    pool: &SqlitePool,
    mv: &PendingMove,
    new_path: &str,
    now: &str,
    direction: MoveDirection,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let decision = match direction {
        MoveDirection::Forward => sqlx::query(
            "UPDATE decisions SET status = 'applied', error_message = NULL, applied_at = ?
             WHERE id = ?",
        ),
        MoveDirection::Revert => {
            sqlx::query("UPDATE decisions SET status = 'skipped', reverted_at = ? WHERE id = ?")
        }
    };
    decision.bind(now).bind(&mv.decision_id).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
    )
    .bind(&mv.to)
    .bind(new_path)
    .bind(now)
    .bind(&mv.media_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn record_failure(
    state: &AppState,
    mv: &PendingMove,
    message: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    let recorded = match direction {
        MoveDirection::Forward => sqlx::query(
            "UPDATE decisions SET status = 'failed', error_message = ?, applied_at = NULL
             WHERE id = ?",
        )
        .bind(message),
        // A revert that fails leaves the move it tried to undo in place: the
        // decision is applied again, so the next Revert still finds it.
        MoveDirection::Revert => {
            sqlx::query("UPDATE decisions SET status = 'applied', reverted_at = NULL WHERE id = ?")
        }
    };
    if let Err(e) = recorded.bind(&mv.decision_id).execute(&state.pool).await {
        error!(decision = %mv.decision_id, "Recording a failed move failed too: {e}");
    }

    log_execution(&state.pool, by, direction.action(), "failed", false, Some(message), mv).await;
}

/// How a move the Arr took and then undid is worded: its files could not
/// follow. A sync finding a requested move undone cannot tell whether it was
/// ever made, and says so.
const UNDONE: &str = "ErrorMoveUndone";
const NOT_MADE: &str = "ErrorMoveNotMade";

/// A move an apply left `requested`, as a sync settles it.
#[derive(sqlx::FromRow)]
struct RequestedRow {
    decision_id: String,
    media_id: String,
    media_title: String,
    current_root_folder: Option<String>,
    target_root_folder: Option<String>,
    reverting: bool,
    /// `None` once the sync has removed the title the Arr no longer holds.
    arr_id: Option<i64>,
}

/// Settle the moves an apply left `requested` on `instance` once a sync has
/// run: a title the Arr holds in its target with no move running is applied,
/// one elsewhere or gone failed. Each title is read from the Arr rather than
/// from its row, which keeps a move made within the second of the sync's read.
pub async fn settle_requested(
    state: &AppState,
    instance: &crate::models::Instance,
    by: &Attribution,
) -> AppResult<()> {
    let rows: Vec<RequestedRow> = sqlx::query_as(
        "SELECT d.id AS decision_id, d.media_id, d.media_title, d.current_root_folder,
                d.target_root_folder, d.reverted_at IS NOT NULL AS reverting, m.arr_id
           FROM decisions d
           LEFT JOIN media m ON m.id = d.media_id
          WHERE d.instance_id = ? AND d.status = 'requested'",
    )
    .bind(&instance.id)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    // An apply running follows the moves it requested itself.
    let Some(_turn) = state.jobs.try_lock("apply") else {
        return Ok(());
    };
    let adapter = state.adapter(instance)?;
    let commands = adapter.move_commands().await?;
    let localizer = state.localizer().await;

    for row in rows {
        let (direction, from, to) = if row.reverting {
            (MoveDirection::Revert, row.target_root_folder, row.current_root_folder)
        } else {
            (MoveDirection::Forward, row.current_root_folder, row.target_root_folder)
        };
        let Some(to) = to else { continue };
        let mv = PendingMove {
            decision_id: row.decision_id,
            media_id: row.media_id,
            media_title: row.media_title,
            instance_id: instance.id.clone(),
            arr_id: row.arr_id.unwrap_or_default(),
            from,
            to,
        };
        let command = row.arr_id.and_then(|item| latest_move(&commands, item, None));
        let held = match (row.arr_id, command.map(|c| &c.state)) {
            (_, Some(CommandState::Running)) => continue,
            (_, Some(CommandState::Ended(_))) => None,
            (None, _) => Some(None),
            (Some(item), _) => match adapter.get_media_one(item).await {
                Ok(held) => Some(held),
                Err(e) => {
                    warn!(instance = %instance.name, "A requested move could not be read: {e}");
                    continue;
                }
            },
        };
        match undone(&localizer, &instance.name, command, held.as_ref(), &mv.to, NOT_MADE) {
            None => record_confirmed(&state.pool, &mv, direction).await,
            Some(reason) => {
                record_failure(state, &mv, &reason, direction, by).await;
                if let Some(Some(held)) = &held {
                    record_held(&state.pool, &mv, held).await;
                }
            }
        }
    }
    Ok(())
}

/// What the webhook is told when an apply or a revert finishes.
fn moves_completed(reverted: bool, report: &ApplyReport) -> notify::Event {
    notify::Event::MovesCompleted {
        reverted,
        applied: report.applied,
        failed: report.failed,
        skipped: report.skipped,
    }
}

/// Close a job on what it did. Nothing done while something failed is a
/// failure, whatever the count reads, or the Tasks screen shows it in green.
async fn close_job(job: crate::jobs::JobHandle, done: usize, failed: usize, detail: Detail) {
    if done == 0 && failed > 0 {
        job.fail_with(detail).await;
    } else {
        job.succeed(detail).await;
    }
}

/// Refuse everything while the global dry-run switch is on.
async fn guard_dry_run(state: &AppState) -> AppResult<()> {
    if state.bool_setting("global_dry_run", true).await {
        let localizer = state.localizer().await;
        return Err(AppError::BadRequest(localizer.translate("ErrorDryRunEnabled", &[])));
    }
    Ok(())
}

/// Enforce the batch ceiling: nothing, and never more than `batch_limit`.
///
/// A count, so it runs before any guard that spells the ids out in a query:
/// `batch_limit` is capped at a thousand, and SQLite binds 32 766 parameters
/// at most, so a list this has passed always fits one statement.
async fn guard_batch_limit(state: &AppState, count: usize) -> AppResult<()> {
    let localizer = state.localizer().await;

    if count == 0 {
        return Err(AppError::BadRequest(localizer.translate("ErrorNoSelection", &[])));
    }

    let batch_limit: usize = state.setting("batch_limit", 50usize).await;
    if count > batch_limit {
        return Err(AppError::BadRequest(localizer.translate(
            "ErrorBatchLimit",
            &[("count", &count.to_string()), ("limit", &batch_limit.to_string())],
        )));
    }
    Ok(())
}

/// Ask a person to look before a batch above the threshold moves anything.
async fn guard_confirmation(
    state: &AppState,
    count: usize,
    confirmed: &Confirmed,
) -> AppResult<()> {
    let localizer = state.localizer().await;
    let threshold: usize = state.setting("confirmation_threshold", 10usize).await;
    if count > threshold && !confirmed.has(confirm::THRESHOLD) {
        return Err(AppError::ConfirmationRequired {
            includes: Vec::new(),
            kind: confirm::THRESHOLD,
            message: localizer.translate(
                "ErrorConfirmationRequired",
                &[("count", &count.to_string()), ("threshold", &threshold.to_string())],
            ),
        });
    }

    Ok(())
}

/// Refuse to write into a destination the Arr says it cannot reach.
///
/// A root folder on a NAS that spins down is reported inaccessible, and the
/// routing map deliberately keeps it: unknown is not gone, and a plan built
/// while the disk was awake must survive the nap. The question is asked here
/// instead, at the one moment it can be answered. It is asked rather than
/// refused, because a NAS that wakes on access cannot be told from a dead disk,
/// and refusing outright would make the product unusable for those
/// installations.
async fn guard_reachable(
    state: &AppState,
    scope: CapacityScope<'_>,
    confirmed: &Confirmed,
) -> AppResult<()> {
    if confirmed.has(confirm::UNREACHABLE) {
        return Ok(());
    }
    let Some(Weighed { rows, to, .. }) = scope.weighed() else {
        return Ok(());
    };
    let sql = format!(
        "SELECT DISTINCT tgt.path, tgt.last_accessible_at
         FROM decisions d
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND tgt.path = {to} COLLATE path
         WHERE {rows}
           AND tgt.accessible = 0
         ORDER BY tgt.path
         LIMIT 1"
    );
    let mut query = sqlx::query_as::<_, (String, Option<String>)>(AssertSqlSafe(sql.as_str()));
    match scope {
        CapacityScope::Decisions(ids) | CapacityScope::Reverting(ids) => {
            for id in ids {
                query = query.bind(id);
            }
        }
        CapacityScope::Simulation(id) => query = query.bind(id),
    }

    if let Some((path, last_seen)) = query.fetch_optional(&state.pool).await? {
        let localizer = state.localizer().await;
        // The date rather than a verdict: twenty minutes reads as a nap and
        // three days as a fault, and the operator is the one who knows their
        // hardware.
        return Err(AppError::ConfirmationRequired {
            includes: Vec::new(),
            kind: confirm::UNREACHABLE,
            message: localizer.translate(
                "ErrorTargetUnreachable",
                &[("path", &path), ("since", last_seen.as_deref().unwrap_or("-"))],
            ),
        });
    }
    Ok(())
}

/// What a guard weighs: a person's selection, a whole run, or moves undone.
#[derive(Clone, Copy)]
enum CapacityScope<'a> {
    Decisions(&'a [String]),
    Simulation(&'a str),
    Reverting(&'a [String]),
}

/// The moves a guard weighs, as SQL over `decisions d`.
struct Weighed {
    /// Which decisions, narrowed to the moves still to write.
    rows: String,
    /// The folder each move writes into.
    to: &'static str,
    /// The folder each move leaves.
    from: &'static str,
}

/// The proposals an apply writes.
const PROPOSED: &str = "d.status = 'pending' AND d.superseded = 0 AND d.action = 'move'";

/// The moves a revert may undo: applied, not undone yet, knowing where they
/// came from, and the latest of their title's standing moves. Undoing an
/// older one would send the title back to its first folder and skip the ones
/// between. The title must still be where the move put it, or the revert
/// pulls it out of a folder somebody chose since. The folder it goes back to
/// must still be one of its instance's, since the guards weigh a destination
/// through its `root_folders` row and ask nothing about a folder without one.
/// The decisions list reads it too, to draw a Revert button on exactly the
/// rows it lets through.
pub(crate) const REVERTIBLE: &str =
    "d.status = 'applied' AND d.reverted_at IS NULL AND d.current_root_folder IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM decisions later
                      WHERE later.media_id = d.media_id AND later.id <> d.id
                        AND later.status = 'applied' AND later.reverted_at IS NULL
                        AND later.applied_at > d.applied_at)
     AND EXISTS (SELECT 1 FROM media here
                  WHERE here.id = d.media_id
                    AND here.current_root_folder = d.target_root_folder COLLATE path)
     AND EXISTS (SELECT 1 FROM root_folders back
                  WHERE back.instance_id = d.instance_id
                    AND back.path = d.current_root_folder COLLATE path)";

impl CapacityScope<'_> {
    /// `None` for an empty selection, which there is nothing to weigh in.
    fn weighed(self) -> Option<Weighed> {
        let listed = |ids: &[String]| format!("d.id IN ({})", crate::db::placeholders(ids.len()));
        let forward = |rows: String| Weighed {
            rows: format!("{rows} AND {PROPOSED}"),
            to: "d.target_root_folder",
            from: "d.current_root_folder",
        };
        match self {
            CapacityScope::Decisions([]) | CapacityScope::Reverting([]) => None,
            CapacityScope::Decisions(ids) => Some(forward(listed(ids))),
            // A whole run is selected by its id rather than by listing its
            // decisions: a library-sized run has more of them than one
            // statement can bind.
            CapacityScope::Simulation(_) => Some(forward("d.simulation_id = ?".to_string())),
            // Back to the folder each move came from.
            CapacityScope::Reverting(ids) => Some(Weighed {
                rows: format!("{} AND {REVERTIBLE}", listed(ids)),
                to: "d.current_root_folder",
                from: "d.target_root_folder",
            }),
        }
    }
}

/// Refuse a plan a destination cannot hold.
///
/// A batch that overruns its destination fails partway through at the Arr and
/// leaves the library half-moved, and hard to notice, since the Arr records
/// the new path whether or not the file arrived.
///
/// Only bytes that cross a filesystem count: two folders reporting the same
/// free space are almost certainly one volume, where a move is a rename. Two
/// *different* volumes that happen to report the same figure (two full disks,
/// say) read as one and are waved through. The same-volume test is evidence,
/// not proof, which is what the confirmation below is for.
///
/// The figure is `root_folders.free_space` as the last sync stored it, not as
/// the disk stands now. A download since then makes it optimistic, so the guard
/// bounds a plan against a recent past rather than the present.
/// Joined through the path collation because the source path comes from the
/// Arr's payload and the destination from our table, each closed or not by a
/// separator, and on Windows in either case.
///
/// `ConfirmationRequired`, not a refusal: the same-volume test is evidence
/// rather than proof, so being wrong costs one click.
async fn guard_capacity(
    state: &AppState,
    scope: CapacityScope<'_>,
    move_files: bool,
    confirmed: &Confirmed,
) -> AppResult<()> {
    // With the files left where they are no byte moves, and a question with
    // no stake teaches people to answer yes to the ones that have one.
    if !move_files || confirmed.has(confirm::CAPACITY) {
        return Ok(());
    }
    let Some(Weighed { rows, to, from }) = scope.weighed() else {
        return Ok(());
    };
    let sql = format!(
        "SELECT {to},
                SUM(CASE WHEN tgt.free_space IS NOT NULL AND tgt.free_space = src.free_space
                         THEN 0 ELSE COALESCE(m.size_on_disk, 0) END),
                MAX(tgt.free_space),
                MAX(CASE WHEN tgt.origin = 'declared' THEN 1 ELSE 0 END)
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND tgt.path = {to} COLLATE path
         LEFT JOIN root_folders src
              ON src.instance_id = d.instance_id
             AND src.path = {from} COLLATE path
         WHERE {rows}
         GROUP BY d.instance_id, {to}"
    );
    let mut query =
        sqlx::query_as::<_, (String, i64, Option<i64>, i64)>(AssertSqlSafe(sql.as_str()));
    match scope {
        CapacityScope::Decisions(ids) | CapacityScope::Reverting(ids) => {
            for id in ids {
                query = query.bind(id);
            }
        }
        CapacityScope::Simulation(id) => query = query.bind(id),
    }

    for (path, incoming, free, declared) in query.fetch_all(&state.pool).await? {
        let Some(free) = free else {
            // Nothing to weigh. A folder an Arr reports has simply not
            // published a figure, and asking about that on every apply would
            // make the question meaningless. A *declared* one with no figure is
            // a different case: it sits under no known root, so nothing
            // upstream will catch a full disk either, and silence there is the
            // guard failing quietly rather than passing.
            if declared == 1 && incoming > 0 {
                let localizer = state.localizer().await;
                return Err(AppError::ConfirmationRequired {
                    includes: Vec::new(),
                    kind: confirm::CAPACITY,
                    message: localizer.translate(
                        "ErrorCapacityUnknown",
                        &[("path", &path), ("needed", &human_bytes(incoming, &localizer))],
                    ),
                });
            }
            continue;
        };
        if incoming > free {
            let localizer = state.localizer().await;
            return Err(AppError::ConfirmationRequired {
                includes: Vec::new(),
                kind: confirm::CAPACITY,
                message: localizer.translate(
                    "ErrorNotEnoughSpace",
                    &[
                        ("path", &path),
                        ("needed", &human_bytes(incoming, &localizer)),
                        ("free", &human_bytes(free, &localizer)),
                    ],
                ),
            });
        }
    }
    Ok(())
}

/// Bytes as an operator reads them, in the vocabulary the interface uses.
///
/// Binary steps, because the figure is compared with what a file manager shows.
/// The symbol and the decimal mark come from `localization`, not from here:
/// spelled locally, they would name one figure in two vocabularies, here and in
/// the root-folders table, on two screens an operator reads together.
/// `frontend/src/api/format.ts` is the other half of that agreement.
fn human_bytes(bytes: i64, localizer: &crate::localization::Localizer) -> String {
    let units = crate::localization::byte_units(localizer.language());
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", bytes.max(0), units[0]);
    }
    let separator = crate::localization::decimal_separator(localizer.language());
    format!("{} {}", format!("{value:.1}").replace('.', &separator.to_string()), units[unit])
}

/// Load decisions that are still safe to apply.
///
/// Revalidated at apply time rather than trusted from the simulation: the rules,
/// the mappings or the library may have changed since, and a superseded proposal
/// must never be executed. A proposal the current rules no longer send to the
/// same folder is skipped and retired, since nothing else retires it when a
/// rule or a mapping changes and it would otherwise stay on screen as pending,
/// skipped again on every apply.
async fn load_pending_moves(
    pool: &SqlitePool,
    ids: &[String],
    revalidation: &mut routing::Revalidation,
) -> AppResult<Vec<PendingMove>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = crate::db::placeholders(ids.len());

    let sql = format!(
        "SELECT d.id, d.media_id, d.media_title, d.instance_id, d.current_root_folder,
                d.target_root_folder, m.arr_id
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         WHERE d.id IN ({placeholders})
           AND d.status = 'pending'
           AND d.superseded = 0
           AND d.action = 'move'
           AND d.target_root_folder IS NOT NULL
           AND m.current_root_folder IS d.current_root_folder COLLATE path"
    );
    // The last clause is the revalidation: a decision names the folder the
    // item was in when it was proposed, and an item moved since (by hand, or
    // by an apply the row already reflects) is not the item it describes.
    // `IS`, so two nulls compare equal, and through the path collation, since
    // an Arr reports a root folder closed by its separator and a title's root
    // without one.
    let mut query = sqlx::query_as::<_, MoveRow>(AssertSqlSafe(sql.as_str()));
    for id in ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let media_ids: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
    let targets = revalidation.targets(pool, &media_ids).await?;
    let (current, stale): (Vec<MoveRow>, Vec<MoveRow>) = rows.into_iter().partition(|row| {
        targets
            .get(&row.1)
            .and_then(Option::as_deref)
            .is_some_and(|target| crate::paths::key(target) == crate::paths::key(&row.5))
    });

    if !stale.is_empty() {
        let stale_ids: Vec<&str> = stale.iter().map(|row| row.0.as_str()).collect();
        // The skip is what keeps the library safe, and the retirement only
        // takes the proposals off the screen. A database busy past its timeout
        // must not fail the moves that are still current.
        match retire(pool, &stale_ids).await {
            Ok(()) => {
                info!(retired = stale.len(), "Proposals the rules no longer justify were retired")
            }
            Err(e) => warn!("Could not retire the proposals the rules no longer justify: {e}"),
        }
    }

    Ok(in_given_order(current, ids))
}

async fn retire(pool: &SqlitePool, decision_ids: &[&str]) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    routing::supersede_decisions(&mut tx, decision_ids).await?;
    tx.commit().await?;
    Ok(())
}

/// Load applied decisions that can still be rolled back.
async fn load_revertible_moves(pool: &SqlitePool, ids: &[String]) -> AppResult<Vec<PendingMove>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = crate::db::placeholders(ids.len());

    let sql = format!(
        "SELECT d.id, d.media_id, d.media_title, d.instance_id, d.target_root_folder,
                d.current_root_folder, m.arr_id
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         WHERE d.id IN ({placeholders})
           AND {REVERTIBLE}"
    );
    let mut query = sqlx::query_as::<_, MoveRow>(AssertSqlSafe(sql.as_str()));
    for id in ids {
        query = query.bind(id);
    }

    Ok(in_given_order(query.fetch_all(pool).await?, ids))
}

/// The moves in the order their decisions were asked for: an `IN` list
/// answers in the order of the index it reads.
fn in_given_order(rows: Vec<MoveRow>, ids: &[String]) -> Vec<PendingMove> {
    let position: HashMap<&str, usize> =
        ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let mut moves: Vec<PendingMove> = rows.into_iter().map(PendingMove::from).collect();
    moves.sort_by_key(|mv| position.get(mv.decision_id.as_str()).copied());
    moves
}

/// One line of the audit trail for a move or a revert.
///
/// Mind the column names: `actor` holds the trigger (`manual`, `schedule`,
/// `webhook`), and `subject` the person the authentication mode named, which
/// `Identity::actor()` supplies. `NULL` in `subject` means nobody asked, or the
/// mode names nobody.
async fn log_execution(
    pool: &SqlitePool,
    by: &Attribution,
    action: &str,
    details: &str,
    success: bool,
    error: Option<&str>,
    mv: &PendingMove,
) {
    let written = sqlx::query(
        "INSERT INTO execution_logs (id, decision_id, action, details, success, error_message,
         instance_id, media_id, media_title, actor, subject)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&mv.decision_id)
    .bind(action)
    .bind(details)
    .bind(success)
    .bind(error)
    .bind(&mv.instance_id)
    .bind(&mv.media_id)
    .bind(&mv.media_title)
    .bind(&by.trigger)
    .bind(&by.subject)
    .execute(pool)
    .await;
    if let Err(e) = written {
        error!(decision = %mv.decision_id, "The execution log entry could not be written: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;
    use crate::localization::Localizer;

    /// The two halves of the application name a size the same way.
    ///
    /// The interface's `formatBytes` prints the reader's own symbols (`2.2 TB`
    /// in English, `2,2 To` in French) off a division by 1024. A capacity
    /// refusal and the root-folders table name the same figure, so a label that
    /// differs describes one figure in two vocabularies. The unit comes from
    /// the dictionary, and `frontend/src/api/format.test.ts` pins the other
    /// side of the same claim.
    #[test]
    fn a_size_is_named_as_the_interface_names_it() {
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("en")), "2.2 TB");
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("fr")), "2,2 To");
        assert_eq!(human_bytes(5_368_709_120, &Localizer::new("ru")), "5,0 ГБ");
        assert_eq!(human_bytes(512, &Localizer::new("fr")), "512 o");
        // Where `Intl` answers with a word, the symbol is derived. The interface
        // applies the same rule, so neither side says `512 byte`.
        assert_eq!(human_bytes(512, &Localizer::new("nl")), "512 B");
    }

    /// Every guardrail name reaches `confirm::ALL`.
    ///
    /// Read out of this file rather than listed again: a name added to the
    /// module and forgotten in the slice refuses a caller that answered every
    /// question, and nothing else in the build says so.
    #[test]
    fn every_guardrail_name_is_in_the_list() {
        const SOURCE: &str = include_str!("executor.rs");
        let module = SOURCE
            .split_once("pub mod confirm {")
            .expect("the confirm module")
            .1
            .split_once("\n}")
            .expect("its closing brace")
            .0;

        let names: Vec<&str> = module
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub const "))
            .filter_map(|rest| rest.split_once(':'))
            .map(|(name, _)| name.trim())
            .filter(|name| *name != "ALL")
            .collect();

        // The count is the guard on the parser: stop matching and every
        // assertion below passes having read nothing.
        assert!(names.len() >= 4, "only {} name(s) parsed: {names:?}", names.len());

        let listed = module
            .split_once("pub const ALL: &[&str] = &[")
            .expect("the list")
            .1
            .split_once(']')
            .expect("its bracket")
            .0;
        for name in names {
            assert!(listed.contains(name), "{name} is a guardrail nobody can answer");
        }
    }
}
