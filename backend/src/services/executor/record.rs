//! What a move leaves in the database: the decision, the title's row and the
//! execution log.

use sqlx::SqlitePool;
use tracing::error;
use uuid::Uuid;

use super::{ApplyError, ApplyReport, MoveDirection, PendingMove};
use crate::error::AppResult;
use crate::integrations::adapter::ArrMedia;
use crate::jobs::Attribution;
use crate::services::routing::format_timestamp;
use crate::state::AppState;

pub(super) async fn fail_batch(
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

pub(super) async fn fail(
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

pub(super) fn count_failure(report: &mut ApplyReport, mv: &PendingMove, message: String) {
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
pub(super) async fn record_requested(
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
pub(super) async fn record_sent(
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
pub(super) async fn record_confirmed(
    pool: &SqlitePool,
    mv: &PendingMove,
    direction: MoveDirection,
) {
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
pub(super) async fn record_held(pool: &SqlitePool, mv: &PendingMove, held: &ArrMedia) {
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
pub(super) fn moved_between(mv: &PendingMove) -> String {
    format!("{} → {}", mv.from.as_deref().unwrap_or("(unknown)"), mv.to)
}

/// Mark the decision applied *and* update the local media row.
///
/// Without the second write the next simulation would keep proposing the same
/// move until the following sync, and the UI would show the media as misplaced.
pub(super) async fn record_success(
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
pub(super) async fn record_outcome(
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

pub(super) async fn record_failure(
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

/// One line of the audit trail for a move or a revert.
///
/// Mind the column names: `actor` holds the trigger (`manual`, `schedule`,
/// `webhook`), and `subject` the person the authentication mode named, which
/// `Identity::actor()` supplies. `NULL` in `subject` means nobody asked, or the
/// mode names nobody.
pub(super) async fn log_execution(
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
