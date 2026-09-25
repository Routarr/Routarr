//! The only code that writes to Radarr/Sonarr.
//!
//! Guardrails, in order: global dry-run, batch limit, explicit confirmation past
//! a threshold, and per-decision revalidation right before the call. Moves are
//! grouped so a batch of 200 films landing in the same folder is one Radarr call
//! rather than 200.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, JobKind};
use crate::models::Instance;
use crate::services::routing::{self, format_timestamp};
use crate::services::rule_engine::normalize_path;
use crate::state::AppState;

/// Outcome of an apply or revert run.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ApplyReport {
    pub requested: usize,
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<ApplyError>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyError {
    pub decision_id: String,
    pub media_title: String,
    pub message: String,
}

/// Columns shared by the apply and revert loaders.
type MoveRow = (String, String, String, String, Option<String>, String, i64, Option<String>);

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
    /// Full on-disk path as Routarr last saw it, used to rebuild it after a move.
    current_path: Option<String>,
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

    /// Every name above, for the one caller that answers all of them.
    ///
    /// `Confirmed::all()` is built from this rather than listing them again:
    /// written twice, a guardrail added to the module and forgotten in the list
    /// refuses a caller that answered every question, and says nothing about
    /// why. Test-only, because in the application answering everything at once
    /// is the blanket flag `Confirmed` replaced.
    #[cfg(test)]
    pub const ALL: &[&str] = &[CAPACITY, THRESHOLD, BATCH, UNREACHABLE];
}

/// Which refusals the caller has looked at and accepted.
///
/// A boolean here meant answering one question answered every question: the
/// three guardrails that ask all read the same flag, so confirming a capacity
/// shortfall silently waved the batch threshold through as well.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct Confirmed(Vec<String>);

impl Confirmed {
    /// Nothing has been answered yet.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn has(&self, kind: &str) -> bool {
        self.0.iter().any(|answered| answered == kind)
    }

    /// Every question answered — for tests, which assert what happens *after*
    /// the asking. Not available to the application, or it would be the
    /// blanket flag this type replaced.
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
    // list longer than one statement can bind from ever reaching one — the
    // capacity guard spells the ids out. Then capacity before the threshold,
    // because a destination that cannot hold the plan is a graver thing to be
    // told than a count. Answering one no longer answers the other: each asks
    // under its own name and reads only that name back.
    guard_batch_limit(state, decision_ids.len()).await?;
    // Reachability before capacity: a destination that is not answering at all
    // is a graver thing to be told than one that may be short of room, and the
    // second figure is unknown while the first is true.
    guard_reachable(state, CapacityScope::Decisions(decision_ids), confirmed).await?;
    guard_capacity(state, CapacityScope::Decisions(decision_ids), confirmed).await?;
    guard_confirmation(state, decision_ids.len(), confirmed).await?;
    run_apply(state, decision_ids, move_files, by).await
}

/// Outcome of applying a whole simulation, slice by slice.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct BatchApplyReport {
    /// Everything the simulation proposed and that was still applicable.
    pub candidates: usize,
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Slices actually attempted — fewer than planned when one of them failed.
    pub batches_run: usize,
    pub batches_planned: usize,
    /// True when a slice failed and the remaining ones were abandoned.
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
/// A slice that fails ends the run: an Arr that rejected fifty moves will
/// likely reject the next fifty, and the untried decisions stay `pending` for
/// the next simulation to repropose.
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

    if !confirmed.has(confirm::BATCH) {
        // This path always asks, so a second gate on capacity would be waved
        // through by the same flag. The shortfall is carried *into* the one
        // question instead — a confirmation that omits the graver fact is worse
        // than no confirmation, because it looks like the fact was considered.
        let mut message = localizer.translate(
            "ErrorConfirmationRequired",
            &[("count", &ids.len().to_string()), ("threshold", "0")],
        );
        // Both graver facts are carried *into* the one question rather than
        // left for a second round trip: a confirmation that omits one looks
        // like it was considered.
        for check in [
            guard_reachable(state, CapacityScope::Simulation(simulation_id), &Confirmed::none())
                .await,
            guard_capacity(state, CapacityScope::Simulation(simulation_id), &Confirmed::none())
                .await,
        ] {
            if let Err(AppError::ConfirmationRequired { message: fact, .. }) = check {
                message = format!("{message}\n\n{fact}");
            }
        }
        return Err(AppError::ConfirmationRequired { kind: confirm::BATCH, message });
    }

    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(localizer.translate("ErrorApplyInProgress", &[])));
    };

    let size: usize = state.setting("batch_limit", 50usize).await.max(1);
    let batches_planned = ids.len().div_ceil(size);

    let job = state
        .jobs
        .start(
            JobKind::Apply,
            &by.trigger,
            None,
            &format!("Applying {} decision(s) in {} batch(es)", ids.len(), batches_planned),
        )
        .await?;

    let state = state.clone();
    let by = by.clone();
    detached(async move {
        let _lock = lock;
        let mut report =
            BatchApplyReport { candidates: ids.len(), batches_planned, ..Default::default() };

        for batch in ids.chunks(size) {
            // The lock is already held, so this goes straight to the writer
            // rather than through run_apply, which would try to take it again.
            let moves = match load_pending_moves(&state.pool, batch).await {
                Ok(moves) => moves,
                Err(e) => {
                    job.fail(&e.to_string()).await;
                    return Err(e);
                }
            };
            report.skipped += batch.len() - moves.len();

            let outcome =
                execute_moves(&state, moves, move_files, MoveDirection::Forward, &by).await;
            report.batches_run += 1;

            match outcome {
                Ok(slice) => {
                    report.applied += slice.applied;
                    report.failed += slice.failed;
                    report.errors.extend(slice.errors);
                    if slice.failed > 0 {
                        report.stopped_early = true;
                        break;
                    }
                }
                Err(e) => {
                    job.fail(&e.to_string()).await;
                    return Err(e);
                }
            }

            job.progress(report.applied + report.failed + report.skipped, ids.len()).await;
        }

        job.succeed(&format!(
            "{} applied, {} failed, {} of {} batch(es)",
            report.applied, report.failed, report.batches_run, report.batches_planned
        ))
        .await;

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
/// *person* stop and look; it means nothing here, and [`auto_apply`] has already
/// established that the set is small, in scope and file-free. The dry-run guard
/// still applies — it is the one switch that must override everything.
///
/// Never moves files on disk: see [`auto_apply`] for why that is a property of
/// the feature rather than an option.
///
/// [`auto_apply`]: crate::services::auto_apply
pub async fn apply_unattended(
    state: &AppState,
    decision_ids: &[String],
    by: &Attribution,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    run_apply(state, decision_ids, false, by).await
}

/// Run the writing part of an apply on a task of its own, and wait for it.
///
/// The request future is dropped the moment the client hangs up — a browser
/// navigating away, an Arr whose webhook timed out — and it is dropped at its
/// next await, which can be the one between the Arr performing a move and
/// this recording it. A move the Arr has performed must be recorded whatever
/// the caller does next. Spawned, the work runs to its end holding the lock
/// and the job handle, so a second apply still waits its turn and the Tasks
/// screen sees this one finish.
async fn detached<T, F>(work: F) -> AppResult<T>
where
    F: std::future::Future<Output = AppResult<T>> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(work)
        .await
        .map_err(|e| AppError::Internal(format!("the apply task ended before it reported: {e}")))?
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

    let job = state
        .jobs
        .start(
            JobKind::Apply,
            &by.trigger,
            None,
            &format!("Applying {} decision(s)", decision_ids.len()),
        )
        .await?;

    let state = state.clone();
    let ids = decision_ids.to_vec();
    let by = by.clone();
    detached(async move {
        let _lock = lock;
        let moves = match load_pending_moves(&state.pool, &ids).await {
            Ok(moves) => moves,
            Err(e) => {
                job.fail(&e.to_string()).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let outcome = execute_moves(&state, moves, move_files, MoveDirection::Forward, &by).await;

        match &outcome {
            Ok(report) => {
                job.succeed(&format!("{} applied, {} failed", report.applied, report.failed)).await
            }
            Err(e) => job.fail(&e.to_string()).await,
        }

        outcome.map(|mut report| {
            report.requested = ids.len();
            report.skipped = skipped;
            report
        })
    })
    .await
}

/// Roll applied decisions back to the root folder they came from.
///
/// Spec: "possibilité d'annuler ou de rejouer certaines opérations".
pub async fn revert_decisions(
    state: &AppState,
    decision_ids: &[String],
    move_files: bool,
    by: &Attribution,
) -> AppResult<ApplyReport> {
    guard_dry_run(state).await?;
    guard_batch_limit(state, decision_ids.len()).await?;

    let Some(lock) = state.jobs.try_lock("apply") else {
        return Err(AppError::Conflict(
            state.localizer().await.translate("ErrorApplyInProgress", &[]),
        ));
    };

    let job = state
        .jobs
        .start(
            JobKind::Revert,
            &by.trigger,
            None,
            &format!("Reverting {} move(s)", decision_ids.len()),
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
                job.fail(&e.to_string()).await;
                return Err(e);
            }
        };
        let skipped = ids.len() - moves.len();

        let outcome = execute_moves(&state, moves, move_files, MoveDirection::Revert, &by).await;

        match &outcome {
            Ok(report) => {
                job.succeed(&format!("{} reverted, {} failed", report.applied, report.failed)).await
            }
            Err(e) => job.fail(&e.to_string()).await,
        }

        outcome.map(|mut report| {
            report.requested = ids.len();
            report.skipped = skipped;
            report
        })
    })
    .await
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MoveDirection {
    Forward,
    Revert,
}

/// Group by (instance, target folder) and issue one bulk call per group.
async fn execute_moves(
    state: &AppState,
    moves: Vec<PendingMove>,
    move_files: bool,
    direction: MoveDirection,
    by: &Attribution,
) -> AppResult<ApplyReport> {
    let mut report = ApplyReport::default();
    if moves.is_empty() {
        return Ok(report);
    }

    let refresh_after_move = state.bool_setting("refresh_after_move", true).await;

    let mut groups: HashMap<(String, String), Vec<PendingMove>> = HashMap::new();
    for mv in moves {
        groups.entry((mv.instance_id.clone(), mv.to.clone())).or_default().push(mv);
    }

    // Cache instances so a 300-decision batch does not re-read the same row.
    let mut instances: HashMap<String, Instance> = HashMap::new();

    for ((instance_id, target), batch) in groups {
        let instance = match instances.get(&instance_id) {
            Some(i) => i.clone(),
            None => match state.instance(&instance_id).await {
                Ok(i) => {
                    instances.insert(instance_id.clone(), i.clone());
                    i
                }
                Err(e) => {
                    fail_batch(state, &batch, &mut report, &e.to_string(), by).await;
                    continue;
                }
            },
        };

        let adapter = match state.adapter(&instance) {
            Ok(a) => a,
            Err(e) => {
                fail_batch(state, &batch, &mut report, &e.to_string(), by).await;
                continue;
            }
        };

        let arr_ids: Vec<i64> = batch.iter().map(|m| m.arr_id).collect();
        let results = adapter.move_to_root_folder(&arr_ids, &target, move_files).await;
        let outcomes: HashMap<i64, bool> = results.iter().map(|(id, r)| (*id, r.is_ok())).collect();
        let errors: HashMap<i64, String> = results
            .into_iter()
            .filter_map(|(id, r)| r.err().map(|e| (id, e.to_string())))
            .collect();

        let mut succeeded_ids = Vec::new();

        for mv in &batch {
            if outcomes.get(&mv.arr_id).copied().unwrap_or(false) {
                succeeded_ids.push(mv.arr_id);
                record_success(state, mv, &target, direction, by).await;
                report.applied += 1;
            } else {
                let message = errors
                    .get(&mv.arr_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown failure".to_string());
                record_failure(state, mv, &message, by).await;
                report.failed += 1;
                report.errors.push(ApplyError {
                    decision_id: mv.decision_id.clone(),
                    media_title: mv.media_title.clone(),
                    message,
                });
            }
        }

        // Spec: "rafraîchissement ou rescan si nécessaire". Best effort — a
        // failed rescan does not invalidate a successful move.
        if refresh_after_move
            && !succeeded_ids.is_empty()
            && let Err(e) = adapter.refresh(&succeeded_ids).await
        {
            error!("Refresh command failed on '{}': {e}", instance.name);
        }
    }

    info!("{} move(s) applied, {} failed", report.applied, report.failed);
    Ok(report)
}

async fn fail_batch(
    state: &AppState,
    batch: &[PendingMove],
    report: &mut ApplyReport,
    message: &str,
    by: &Attribution,
) {
    for mv in batch {
        record_failure(state, mv, message, by).await;
        report.failed += 1;
        report.errors.push(ApplyError {
            decision_id: mv.decision_id.clone(),
            media_title: mv.media_title.clone(),
            message: message.to_string(),
        });
    }
}

/// Mark the decision applied *and* update the local media row.
///
/// Without the second write the next simulation would keep proposing the same
/// move until the following sync, and the UI would show the media as misplaced.
async fn record_success(
    state: &AppState,
    mv: &PendingMove,
    target: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    let now = format_timestamp(chrono::Utc::now());
    let pool = &state.pool;

    // Rewrite the local path so it reflects reality until the next sync.
    // Computed in Rust rather than SQL: doing the substring arithmetic in SQLite
    // silently produced `/movies/animeTitle` whenever the stored root folder
    // carried a trailing slash.
    let new_path = mv.current_path.as_deref().map(|path| relocate(path, target));

    // The decision and the media row in one transaction: an `applied` decision
    // beside a stale path reproposes a move that already happened.
    if let Err(e) = record_outcome(pool, mv, target, new_path.as_deref(), &now, direction).await {
        error!(decision = %mv.decision_id, "The move succeeded but recording it failed: {e}");
    }

    log_execution(
        pool,
        by,
        &mv.decision_id,
        match direction {
            MoveDirection::Forward => "move",
            MoveDirection::Revert => "revert",
        },
        &format!("{} → {}", mv.from.as_deref().unwrap_or("(unknown)"), target),
        true,
        None,
        mv,
    )
    .await;
}

/// The two rows a successful move changes, written together.
///
/// `moved_at` is what stops a synchronisation that read the Arr *before* this
/// move from putting the old path back — see migration 007.
async fn record_outcome(
    pool: &SqlitePool,
    mv: &PendingMove,
    target: &str,
    new_path: Option<&str>,
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
    .bind(target)
    .bind(new_path)
    .bind(now)
    .bind(&mv.media_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn record_failure(state: &AppState, mv: &PendingMove, message: &str, by: &Attribution) {
    if let Err(e) =
        sqlx::query("UPDATE decisions SET status = 'failed', error_message = ? WHERE id = ?")
            .bind(message)
            .bind(&mv.decision_id)
            .execute(&state.pool)
            .await
    {
        error!(decision = %mv.decision_id, "Recording a failed move failed too: {e}");
    }

    log_execution(&state.pool, by, &mv.decision_id, "move", "failed", false, Some(message), mv)
        .await;
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
/// A count, so it runs before any guard that spells the ids out in a query —
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
/// instead, at the one moment it can be answered — and asked rather than
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
    let selected = match scope {
        CapacityScope::Decisions([]) => return Ok(()),
        CapacityScope::Decisions(ids) => {
            format!("d.id IN ({})", crate::db::placeholders(ids.len()))
        }
        CapacityScope::Simulation(_) => "d.simulation_id = ?".to_string(),
    };
    let sql = format!(
        "SELECT DISTINCT tgt.path, tgt.last_accessible_at
         FROM decisions d
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND rtrim(tgt.path, '/') = rtrim(d.target_root_folder, '/')
         WHERE {selected}
           AND d.status = 'pending' AND d.superseded = 0 AND d.action = 'move'
           AND tgt.accessible = 0
         ORDER BY tgt.path
         LIMIT 1"
    );
    let mut query = sqlx::query_as::<_, (String, Option<String>)>(AssertSqlSafe(sql.as_str()));
    match scope {
        CapacityScope::Decisions(ids) => {
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
            kind: confirm::UNREACHABLE,
            message: localizer.translate(
                "ErrorTargetUnreachable",
                &[("path", &path), ("since", last_seen.as_deref().unwrap_or("-"))],
            ),
        });
    }
    Ok(())
}

/// What a capacity check weighs: a person's selection, or a whole run.
#[derive(Clone, Copy)]
enum CapacityScope<'a> {
    Decisions(&'a [String]),
    Simulation(&'a str),
}

/// Refuse a plan a destination cannot hold.
///
/// A batch that overruns its destination fails partway through at the Arr and
/// leaves the library half-moved — and hard to notice, since the Arr records
/// the new path whether or not the file arrived.
///
/// Only bytes that cross a filesystem count: two folders reporting the same
/// free space are almost certainly one volume, where a move is a rename. Two
/// *different* volumes that happen to report the same figure — two full disks,
/// say — read as one and are waved through; the same-volume test is evidence,
/// not proof, which is what the confirmation below is for.
///
/// The figure is `root_folders.free_space` as the last sync stored it, not as
/// the disk stands now. A download since then makes it optimistic, so the guard
/// bounds a plan against a recent past rather than the present.
/// `rtrim` on both sides of the join because the source path comes from the
/// Arr's payload and the destination from our table.
///
/// `ConfirmationRequired`, not a refusal: the same-volume test is evidence
/// rather than proof, so being wrong costs one click.
async fn guard_capacity(
    state: &AppState,
    scope: CapacityScope<'_>,
    confirmed: &Confirmed,
) -> AppResult<()> {
    if confirmed.has(confirm::CAPACITY) {
        return Ok(());
    }
    // A whole run is selected by its id rather than by listing its decisions:
    // a library-sized run has more of them than one statement can bind.
    let selected = match scope {
        CapacityScope::Decisions([]) => return Ok(()),
        CapacityScope::Decisions(ids) => {
            format!("d.id IN ({})", crate::db::placeholders(ids.len()))
        }
        CapacityScope::Simulation(_) => "d.simulation_id = ?".to_string(),
    };
    let sql = format!(
        "SELECT d.target_root_folder,
                SUM(CASE WHEN tgt.free_space IS NOT NULL AND tgt.free_space = src.free_space
                         THEN 0 ELSE COALESCE(m.size_on_disk, 0) END),
                MAX(tgt.free_space),
                MAX(CASE WHEN tgt.origin = 'declared' THEN 1 ELSE 0 END)
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND rtrim(tgt.path, '/') = rtrim(d.target_root_folder, '/')
         LEFT JOIN root_folders src
              ON src.instance_id = d.instance_id
             AND rtrim(src.path, '/') = rtrim(d.current_root_folder, '/')
         WHERE {selected}
           AND d.status = 'pending' AND d.superseded = 0 AND d.action = 'move'
         GROUP BY d.instance_id, d.target_root_folder"
    );
    let mut query =
        sqlx::query_as::<_, (String, i64, Option<i64>, i64)>(AssertSqlSafe(sql.as_str()));
    match scope {
        CapacityScope::Decisions(ids) => {
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
/// Binary steps, because the figure is compared with what a file manager shows
/// — and the symbol and the decimal mark come from `localization`, not from
/// here. Spelled locally this printed `2.2 TiB` where the root-folders table
/// printed `2,2 To` off the same division: one figure, two vocabularies, on two
/// screens an operator reads together. `frontend/src/api/format.ts` is the
/// other half of that agreement.
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
async fn load_pending_moves(pool: &SqlitePool, ids: &[String]) -> AppResult<Vec<PendingMove>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = crate::db::placeholders(ids.len());

    let sql = format!(
        "SELECT d.id, d.media_id, d.media_title, d.instance_id, d.current_root_folder,
                d.target_root_folder, m.arr_id, m.current_path
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         WHERE d.id IN ({placeholders})
           AND d.status = 'pending'
           AND d.superseded = 0
           AND d.action = 'move'
           AND d.target_root_folder IS NOT NULL
           AND m.current_root_folder IS d.current_root_folder"
    );
    // The last clause is the revalidation: a decision names the folder the
    // item was in when it was proposed, and an item moved since — by hand, or
    // by an apply the row already reflects — is not the item it describes.
    // `IS`, so two nulls compare equal.
    let mut query = sqlx::query_as::<_, MoveRow>(AssertSqlSafe(sql.as_str()));
    for id in ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let media_ids: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
    let targets = routing::current_targets(pool, &media_ids).await?;
    let (current, stale): (Vec<MoveRow>, Vec<MoveRow>) = rows.into_iter().partition(|row| {
        targets
            .get(&row.1)
            .and_then(Option::as_deref)
            .is_some_and(|target| normalize_path(target) == normalize_path(&row.5))
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

    Ok(current
        .into_iter()
        .map(|(decision_id, media_id, media_title, instance_id, from, to, arr_id, current_path)| {
            PendingMove {
                decision_id,
                media_id,
                media_title,
                instance_id,
                arr_id,
                from,
                to,
                current_path,
            }
        })
        .collect())
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
                d.current_root_folder, m.arr_id, m.current_path
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         WHERE d.id IN ({placeholders})
           AND d.status = 'applied'
           AND d.reverted_at IS NULL
           AND d.current_root_folder IS NOT NULL"
    );
    let mut query = sqlx::query_as::<_, MoveRow>(AssertSqlSafe(sql.as_str()));
    for id in ids {
        query = query.bind(id);
    }

    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(decision_id, media_id, media_title, instance_id, from, to, arr_id, current_path)| {
            PendingMove {
                decision_id,
                media_id,
                media_title,
                instance_id,
                arr_id,
                from,
                to,
                current_path,
            }
        })
        .collect())
}

/// Re-root a media path under a new root folder, keeping its own folder name.
///
/// The folder name is whatever the Arr already uses; only the prefix changes.
fn relocate(current_path: &str, new_root: &str) -> String {
    let trimmed = current_path.trim_end_matches(['/', '\\']);
    let separator = if trimmed.contains('\\') && !trimmed.contains('/') { '\\' } else { '/' };

    // A path with no separator at all is itself the folder name.
    let name = trimmed.rsplit(separator).next().unwrap_or_default();
    let root = new_root.trim_end_matches(['/', '\\']);

    if name.is_empty() { root.to_string() } else { format!("{root}{separator}{name}") }
}

#[allow(clippy::too_many_arguments)]
async fn log_execution(
    pool: &SqlitePool,
    by: &Attribution,
    decision_id: &str,
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
    .bind(decision_id)
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
        error!(decision = %decision_id, "The execution log entry could not be written: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;
    use crate::localization::Localizer;

    /// The two halves of the application name a size the same way.
    ///
    /// This helper printed IEC symbols — `2.2 TiB` — while the interface's
    /// `formatBytes` printed the reader's own, `2.2 TB` in English and
    /// `2,2 To` in French, off the same division by 1024. The number agreed and
    /// the label did not, so a capacity refusal and the root-folders table
    /// described one figure in two vocabularies. The unit comes from the
    /// dictionary now, and `frontend/src/api/format.test.ts` pins the other
    /// side of the same claim.
    #[test]
    fn a_size_is_named_as_the_interface_names_it() {
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("en")), "2.2 TB");
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("fr")), "2,2 To");
        assert_eq!(human_bytes(5_368_709_120, &Localizer::new("ru")), "5,0 ГБ");
        assert_eq!(human_bytes(512, &Localizer::new("fr")), "512 o");
        // Where `Intl` answers with a word, the symbol is derived — the same
        // rule the interface applies, so neither side says `512 byte`.
        assert_eq!(human_bytes(512, &Localizer::new("nl")), "512 B");
    }

    /// Every guardrail name reaches `confirm::ALL`.
    ///
    /// Read out of this file rather than listed again: a name added to the
    /// module and forgotten in the slice refuses a caller that answered every
    /// question, which is how `UNREACHABLE` was missed — the edit meant to add
    /// it targeted a literal `cargo fmt` had reflowed, so it did nothing and
    /// said nothing.
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

    use super::*;

    #[test]
    fn relocates_under_the_new_root() {
        assert_eq!(
            relocate("/movies/standard/Totoro (1988)", "/movies/anime"),
            "/movies/anime/Totoro (1988)"
        );
    }

    #[test]
    fn tolerates_trailing_slashes_on_either_side() {
        // The SQL version this replaced produced "/movies/animeTotoro (1988)".
        assert_eq!(
            relocate("/movies/standard/Totoro (1988)/", "/movies/anime/"),
            "/movies/anime/Totoro (1988)"
        );
    }

    #[test]
    fn keeps_only_the_leaf_folder_name() {
        assert_eq!(
            relocate("/mnt/pool/movies/standard/Akira (1988)", "/tank/anime"),
            "/tank/anime/Akira (1988)"
        );
    }

    #[test]
    fn handles_windows_paths() {
        assert_eq!(relocate("D:\\media\\movies\\Akira", "E:\\anime"), "E:\\anime\\Akira");
    }

    #[test]
    fn degrades_to_the_root_when_there_is_no_folder_name() {
        assert_eq!(relocate("Akira", "/tank/anime"), "/tank/anime/Akira");
        assert_eq!(relocate("/", "/tank/anime"), "/tank/anime");
    }
}
