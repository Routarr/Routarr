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
    record_failure(state, mv, failed, None, direction, by).await;
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
        .execute(&mut *connection)
        .await?;
        log_execution(connection, by, direction, None, mv).await;
        Ok(())
    })
    .await;
    if let Err(e) = moved {
        error!(decision = %mv.decision_id, "The Arr took the move but recording it failed: {e}");
    }
}

/// A requested move the Arr was seen to finish. The proposals stored since for the title are
/// settled only by the call that confirms it, or a finish settled elsewhere would hide them.
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
        let confirmed =
            sqlx::query("UPDATE decisions SET status = ? WHERE id = ? AND status = 'requested'")
                .bind(status)
                .bind(&mv.decision_id)
                .execute(&mut *connection)
                .await?;
        if confirmed.rows_affected() > 0 {
            settle_proposals_since(connection, mv).await?;
        }
        Ok(())
    })
    .await;
    if let Err(e) = confirmed {
        error!(decision = %mv.decision_id, "Recording a finished move failed: {e}");
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
    let recorded = durably(&state.pool, async |connection| {
        write_outcome(connection, mv, new_path, &now, direction).await?;
        log_execution(connection, by, direction, None, mv).await;
        Ok(())
    })
    .await;
    if let Err(e) = recorded {
        error!(decision = %mv.decision_id, "The move succeeded but recording it failed: {e}");
    }
}

/// `write` in one commit that waits for the disk: under the pool's `synchronous = NORMAL` a power
/// cut can roll back the record of a move the Arr made, and its revert with it.
async fn durably<T>(
    pool: &SqlitePool,
    write: impl AsyncFnOnce(&mut SqliteConnection) -> AppResult<T>,
) -> AppResult<T> {
    use sqlx::Connection;
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA synchronous = FULL").execute(&mut *connection).await?;
    let written: AppResult<T> = async {
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        let written = write(&mut tx).await?;
        tx.commit().await?;
        Ok(written)
    }
    .await;
    let restored = sqlx::query("PRAGMA synchronous = NORMAL").execute(&mut *connection).await;
    let written = written?;
    restored?;
    Ok(written)
}

/// The two rows a successful move changes. `moved_at` stops a sync that read the Arr before the
/// move from putting the old path back (`upsert_media` in `services/sync.rs`).
async fn write_outcome(
    connection: &mut SqliteConnection,
    mv: &PendingMove,
    new_path: &str,
    now: &str,
    direction: MoveDirection,
) -> AppResult<()> {
    let decision = match direction {
        MoveDirection::Forward => sqlx::query(
            "UPDATE decisions SET status = 'applied', error_message = NULL, applied_at = ?
             WHERE id = ?",
        ),
        MoveDirection::Revert => {
            sqlx::query("UPDATE decisions SET status = 'skipped', reverted_at = ? WHERE id = ?")
        }
    };
    decision.bind(now).bind(&mv.decision_id).execute(&mut *connection).await?;
    sqlx::query(
        "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ? WHERE id = ?",
    )
    .bind(&mv.to)
    .bind(new_path)
    .bind(now)
    .bind(&mv.media_id)
    .execute(&mut *connection)
    .await?;
    settle_proposals_since(connection, mv).await
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

/// A move that failed, with the title's row as the Arr holds it once it undid the move, `held`.
pub(super) async fn record_failure(
    state: &AppState,
    mv: &PendingMove,
    failed: &Said,
    held: Option<&ArrMedia>,
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
    let now = format_timestamp(chrono::Utc::now());
    let recorded = durably(&state.pool, async |connection| {
        recorded.bind(&mv.decision_id).execute(&mut *connection).await?;
        if let Some(held) = held {
            sqlx::query(
                "UPDATE media SET current_root_folder = ?, current_path = ?, moved_at = ?
                  WHERE id = ?",
            )
            .bind(&held.root_folder_path)
            .bind(&held.path)
            .bind(&now)
            .bind(&mv.media_id)
            .execute(&mut *connection)
            .await?;
        }
        log_execution(connection, by, direction, Some(&failed.stored), mv).await;
        Ok(())
    })
    .await;
    if let Err(e) = recorded {
        error!(decision = %mv.decision_id, "Recording a failed move failed too: {e}");
    }
}

/// The execution log line of a move or a revert, `failed` with its error, in the commit of what
/// it tells, which a line refused leaves standing. Mind the names: `actor` holds the trigger, and
/// `subject` the name `Identity::actor()` gives, `NULL` when nobody asked or the mode names nobody.
async fn log_execution(
    connection: &mut SqliteConnection,
    by: &Attribution,
    direction: MoveDirection,
    failed: Option<&str>,
    mv: &PendingMove,
) {
    let written = sqlx::query(
        "INSERT INTO execution_logs (id, decision_id, action, details, success, error_message,
         instance_id, media_id, media_title, actor, subject, subject_key, job_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&mv.decision_id)
    .bind(direction.action())
    .bind(moved_between(mv))
    .bind(failed.is_none())
    .bind(failed)
    .bind(&mv.instance_id)
    .bind(&mv.media_id)
    .bind(&mv.media_title)
    .bind(&by.trigger)
    .bind(&by.subject)
    .bind(&by.key)
    .bind(&by.job)
    .execute(connection)
    .await;
    if let Err(e) = written {
        error!(decision = %mv.decision_id, "The execution log entry could not be written: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Totoro's move to the anime folder, as decision `decision_id` proposed it.
    fn totoro(decision_id: &str) -> PendingMove {
        PendingMove {
            decision_id: decision_id.into(),
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
        }
    }

    async fn propose(pool: &SqlitePool, decision_id: &str, status: &str) {
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                                    target_category, action, status)
             VALUES (?, 'm-1', 'Totoro', 'movie', 'inst-1', 'anime', 'move', ?)",
        )
        .bind(decision_id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn superseded(pool: &SqlitePool, decision_id: &str) -> bool {
        sqlx::query_scalar("SELECT superseded FROM decisions WHERE id = ?")
            .bind(decision_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// A finish seen for a move something else settled first confirms nothing, and the proposals
    /// stored since for the title stay to be applied.
    #[tokio::test]
    async fn a_finish_that_confirms_nothing_settles_no_proposal() {
        let pool = crate::db::test_pool().await;
        propose(&pool, "d-1", "failed").await;
        propose(&pool, "d-2", "pending").await;

        record_confirmed(&pool, &totoro("d-1"), MoveDirection::Forward).await;

        assert!(!superseded(&pool, "d-2").await, "a move nobody confirmed settled a proposal");
        sqlx::query("UPDATE decisions SET status = 'requested' WHERE id = 'd-1'")
            .execute(&pool)
            .await
            .unwrap();
        record_confirmed(&pool, &totoro("d-1"), MoveDirection::Forward).await;
        assert!(superseded(&pool, "d-2").await, "a move confirmed left its proposal standing");
    }

    /// The commits in the write-ahead log at `wal`, each one a wait for the disk under
    /// `synchronous = FULL`.
    fn commits_in(wal: &std::path::Path) -> usize {
        let log = std::fs::read(wal).unwrap_or_default();
        let Some(header) = log.get(..32) else { return 0 };
        let field = |bytes: &[u8], at: usize| -> [u8; 4] { bytes[at..at + 4].try_into().unwrap() };
        let page = u32::from_be_bytes(field(header, 8)) as usize;
        log[32..]
            .chunks_exact(24 + page)
            .filter(|frame| {
                field(frame, 8) == field(header, 16) && field(frame, 12) == field(header, 20)
            })
            .filter(|frame| field(frame, 4) != [0; 4])
            .count()
    }

    /// Every title an apply moves waits for the disk on each commit recording it, so a move the
    /// Arr made and its log line are written in one.
    #[tokio::test]
    async fn a_move_and_its_log_line_are_written_in_one_commit() {
        let dir = crate::tests::TempDir::new("record-commits");
        let mut config = crate::config::Config::for_tests();
        config.set_db_path(dir.join("routarr.db"));
        let pool = crate::db::init_pool(&config).await.unwrap();
        propose(&pool, "d-1", "requested").await;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&pool).await.unwrap();
        let state = AppState::for_tests_on(pool.clone()).with_config(config.clone());
        let (by, wal) = (Attribution::manual(None), dir.join("routarr.db-wal"));

        record_success(&state, &totoro("d-1"), "/movies/anime/Totoro", MoveDirection::Forward, &by)
            .await;
        assert_eq!(commits_in(&wal), 1, "a move made");
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&pool).await.unwrap();
        let failed = Said { stored: "ErrorMoveUndone".into(), said: "Undone".into() };
        record_failure(&state, &totoro("d-1"), &failed, None, MoveDirection::Forward, &by).await;
        assert_eq!(commits_in(&wal), 1, "a move failed");

        let logged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(logged, 2);
        pool.close().await;
    }

    /// The move log is read in every language, so an origin Routarr never knew is left blank
    /// rather than named in English.
    #[test]
    fn a_move_from_a_folder_routarr_did_not_know_leaves_the_origin_blank() {
        let mv = totoro("d-1");
        assert_eq!(moved_between(&mv), " → /movies/anime");
        let known = PendingMove { from: Some("/movies/standard".into()), ..mv };
        assert_eq!(moved_between(&known), "/movies/standard → /movies/anime");
    }
}
