//! What a move leaves in the database: the decision, the title's row and the
//! execution log.

use sqlx::{SqliteConnection, SqlitePool};
use tracing::error;
use uuid::Uuid;

use super::{ApplyError, ApplyReport, MoveDirection, PendingMove};
use crate::error::AppResult;
use crate::integrations::adapter::ArrMedia;
use crate::jobs::Attribution;
use crate::services::connection::Said;
use crate::services::routing::format_timestamp;
use crate::state::AppState;

pub(super) async fn fail_batch(
    state: &AppState,
    batch: &[PendingMove],
    report: &mut ApplyReport,
    failed: &Said,
    direction: MoveDirection,
    by: &Attribution,
) {
    for mv in batch {
        fail(state, mv, failed, report, direction, by).await;
    }
}

pub(super) async fn fail(
    state: &AppState,
    mv: &PendingMove,
    failed: &Said,
    report: &mut ApplyReport,
    direction: MoveDirection,
    by: &Attribution,
) {
    record_failure(state, mv, failed, direction, by).await;
    count_failure(report, mv, failed);
}

pub(super) fn count_failure(report: &mut ApplyReport, mv: &PendingMove, failed: &Said) {
    report.failed += 1;
    report.errors.push(ApplyError {
        decision_id: mv.decision_id.clone(),
        media_title: mv.media_title.clone(),
        message: failed.said.clone(),
        stored: failed.stored.clone(),
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
    durably(pool, async |connection| {
        requested.bind(now).bind(&mv.decision_id).execute(connection).await?;
        Ok(())
    })
    .await
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
    let moved = durably(&state.pool, async |connection| {
        sqlx::query(
            "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
        )
        .bind(&mv.to)
        .bind(path)
        .bind(&now)
        .bind(&mv.media_id)
        .execute(connection)
        .await?;
        Ok(())
    })
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
    let confirmed = durably(pool, async |connection| {
        sqlx::query("UPDATE decisions SET status = ? WHERE id = ? AND status = 'requested'")
            .bind(status)
            .bind(&mv.decision_id)
            .execute(&mut *connection)
            .await?;
        settle_proposals_since(connection, mv).await
    })
    .await;
    if let Err(e) = confirmed {
        error!(decision = %mv.decision_id, "Recording a finished move failed: {e}");
    }
}

/// The title's row as the Arr holds it after undoing a move.
pub(super) async fn record_held(pool: &SqlitePool, mv: &PendingMove, held: &ArrMedia) {
    let now = format_timestamp(chrono::Utc::now());
    let written = durably(pool, async |connection| {
        sqlx::query(
            "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
        )
        .bind(&held.root_folder_path)
        .bind(&held.path)
        .bind(now)
        .bind(&mv.media_id)
        .execute(connection)
        .await?;
        Ok(())
    })
    .await;
    if let Err(e) = written {
        error!(decision = %mv.decision_id, "Recording where the Arr holds the title failed: {e}");
    }
}

/// What the log says a move did or was to do, the origin empty when Routarr did not know it.
pub(super) fn moved_between(mv: &PendingMove) -> String {
    format!("{} → {}", mv.from.as_deref().unwrap_or_default(), mv.to)
}

/// Record a move the Arr made, the decision and the title's row together: an `applied` decision
/// beside a stale path proposes the same move again and shows the title as misplaced.
pub(super) async fn record_success(
    state: &AppState,
    mv: &PendingMove,
    new_path: &str,
    direction: MoveDirection,
    by: &Attribution,
) {
    let now = format_timestamp(chrono::Utc::now());
    let pool = &state.pool;

    if let Err(e) = record_outcome(pool, mv, new_path, &now, direction).await {
        error!(decision = %mv.decision_id, "The move succeeded but recording it failed: {e}");
    }

    log_execution(pool, by, direction.action(), &moved_between(mv), true, None, mv).await;
}

/// `write` in commits that wait for the disk: under the pool's `synchronous = NORMAL` a power cut
/// can roll back the record of a move the Arr made, and its revert with it.
async fn durably<T>(
    pool: &SqlitePool,
    write: impl AsyncFnOnce(&mut SqliteConnection) -> AppResult<T>,
) -> AppResult<T> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA synchronous = FULL").execute(&mut *connection).await?;
    let written = write(&mut connection).await;
    let restored = sqlx::query("PRAGMA synchronous = NORMAL").execute(&mut *connection).await;
    let written = written?;
    restored?;
    Ok(written)
}

/// The two rows a successful move changes, in one commit. `moved_at` stops a sync that read the
/// Arr before the move from putting the old path back (`upsert_media` in `services/sync.rs`).
pub(super) async fn record_outcome(
    pool: &SqlitePool,
    mv: &PendingMove,
    new_path: &str,
    now: &str,
    direction: MoveDirection,
) -> AppResult<()> {
    durably(pool, async |connection| write_outcome(connection, mv, new_path, now, direction).await)
        .await
}

async fn write_outcome(
    connection: &mut SqliteConnection,
    mv: &PendingMove,
    new_path: &str,
    now: &str,
    direction: MoveDirection,
) -> AppResult<()> {
    use sqlx::Connection;
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
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
    settle_proposals_since(&mut tx, mv).await?;
    tx.commit().await?;
    Ok(())
}

/// A run stored while the move was on its way proposed it again: made, it settles them.
async fn settle_proposals_since(
    connection: &mut SqliteConnection,
    mv: &PendingMove,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE decisions SET superseded = 1
          WHERE media_id = ? AND id <> ? AND status = 'pending' AND superseded = 0",
    )
    .bind(&mv.media_id)
    .bind(&mv.decision_id)
    .execute(connection)
    .await?;
    Ok(())
}

pub(super) async fn record_failure(
    state: &AppState,
    mv: &PendingMove,
    failed: &Said,
    direction: MoveDirection,
    by: &Attribution,
) {
    let recorded = match direction {
        MoveDirection::Forward => sqlx::query(
            "UPDATE decisions SET status = 'failed', error_message = ?, applied_at = NULL
             WHERE id = ?",
        )
        .bind(&failed.stored),
        // A failed revert leaves the move in place: applied again, the next Revert finds it.
        MoveDirection::Revert => {
            sqlx::query("UPDATE decisions SET status = 'applied', reverted_at = NULL WHERE id = ?")
        }
    };
    let recorded = durably(&state.pool, async |connection| {
        recorded.bind(&mv.decision_id).execute(connection).await?;
        Ok(())
    })
    .await;
    if let Err(e) = recorded {
        error!(decision = %mv.decision_id, "Recording a failed move failed too: {e}");
    }

    let stored = Some(failed.stored.as_str());
    log_execution(&state.pool, by, direction.action(), &moved_between(mv), false, stored, mv).await;
}

/// One execution log line for a move or a revert. Mind the names: `actor` holds the trigger,
/// and `subject` the name `Identity::actor()` gives, `NULL` when nobody asked or the mode names
/// nobody.
pub(super) async fn log_execution(
    pool: &SqlitePool,
    by: &Attribution,
    action: &str,
    details: &str,
    success: bool,
    error: Option<&str>,
    mv: &PendingMove,
) {
    let written = durably(pool, async |connection| {
        sqlx::query(
            "INSERT INTO execution_logs (id, decision_id, action, details, success, error_message,
         instance_id, media_id, media_title, actor, subject, subject_key, job_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&by.key)
        .bind(&by.job)
        .execute(connection)
        .await?;
        Ok(())
    })
    .await;
    if let Err(e) = written {
        error!(decision = %mv.decision_id, "The execution log entry could not be written: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The move log is read in every language, so an origin Routarr never knew is left blank
    /// rather than named in English.
    #[test]
    fn a_move_from_a_folder_routarr_did_not_know_leaves_the_origin_blank() {
        let mv = PendingMove {
            decision_id: "d-1".into(),
            media_id: "m-1".into(),
            media_title: "Totoro".into(),
            instance_id: "inst-1".into(),
            arr_id: 10,
            from: None,
            to: "/movies/anime".into(),
            media_type: "movie".into(),
            tmdb_id: None,
            tvdb_id: None,
            imdb_id: None,
        };
        assert_eq!(moved_between(&mv), " → /movies/anime");
        let known = PendingMove { from: Some("/movies/standard".into()), ..mv };
        assert_eq!(moved_between(&known), "/movies/standard → /movies/anime");
    }
}
