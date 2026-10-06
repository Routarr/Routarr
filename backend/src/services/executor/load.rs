//! The decisions a run writes, read again and revalidated just before it.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashMap;
use tracing::{info, warn};

use super::guards::REVERTIBLE;
use super::{MoveRow, PendingMove};
use crate::error::{AppError, AppResult};
use crate::services::routing;
use crate::state::AppState;

/// Each id once, in the order first given: an id sent twice is one decision.
pub(super) fn distinct(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().filter(|id| seen.insert(id.as_str())).cloned().collect()
}

/// The proposals to apply for `ids`, and how many were replaced.
///
/// A scheduled pass after each sync supersedes every pending proposal and
/// writes it again, so a proposal a person is reviewing may have a successor
/// proposing the very same move: that one is applied in its place, and
/// revalidated as any other. A proposal now replaced by another move, or by
/// none, is counted and left. Any other id passes as given.
pub(super) async fn current_proposals(
    pool: &SqlitePool,
    ids: &[String],
) -> AppResult<(Vec<String>, usize)> {
    let mut successors: HashMap<String, Option<String>> = HashMap::new();
    for chunk in ids.chunks(routing::BIND_CHUNK) {
        let sql = format!(
            "SELECT old.id,
                    (SELECT cur.id FROM decisions cur
                      WHERE cur.media_id = old.media_id
                        AND cur.status = 'pending' AND cur.superseded = 0
                        AND cur.action = 'move'
                        AND cur.current_root_folder IS old.current_root_folder COLLATE path
                        AND cur.target_root_folder = old.target_root_folder COLLATE path)
               FROM decisions old
              WHERE old.id IN ({})
                AND old.status = 'pending' AND old.superseded = 1 AND old.action = 'move'",
            crate::db::placeholders(chunk.len())
        );
        let mut query = sqlx::query_as::<_, (String, Option<String>)>(AssertSqlSafe(sql.as_str()));
        for id in chunk {
            query = query.bind(id);
        }
        successors.extend(query.fetch_all(pool).await?);
    }
    let mut superseded = 0;
    let mut current = Vec::with_capacity(ids.len());
    for id in ids {
        match successors.get(id) {
            Some(Some(successor)) => current.push(successor.clone()),
            Some(None) => superseded += 1,
            None => current.push(id.clone()),
        }
    }
    Ok((distinct(&current), superseded))
}

/// Refuse ids that name no decision, before a task starts: a run over nothing
/// answering success tells a script its call worked. In English, since the
/// interface sent the ids and no one typed them. After the guardrails, which
/// ask about the moves that exist.
pub(super) async fn refuse_unknown_decisions(
    state: &AppState,
    decision_ids: &[String],
) -> AppResult<()> {
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

/// The current proposals standing for a simulation's moves: each move it
/// proposed and still pending, or the identical move a newer pass proposed in
/// its place (see [`current_proposals`]). One parameter, the simulation id.
pub(super) const CURRENT_FOR_RUN: &str = "SELECT cur.id FROM decisions run
       JOIN decisions cur
         ON cur.media_id = run.media_id
        AND cur.status = 'pending' AND cur.superseded = 0 AND cur.action = 'move'
        AND cur.current_root_folder IS run.current_root_folder COLLATE path
        AND cur.target_root_folder = run.target_root_folder COLLATE path
      WHERE run.simulation_id = ? AND run.action = 'move' AND run.status = 'pending'
        AND run.target_root_folder IS NOT NULL";

/// The moves a simulation proposed that are still worth attempting, and how
/// many of its proposals were replaced by another move.
pub(super) async fn applicable_from_simulation(
    pool: &SqlitePool,
    simulation_id: &str,
) -> AppResult<(Vec<String>, usize)> {
    let sql = format!(
        "SELECT d.id FROM decisions d WHERE d.id IN ({CURRENT_FOR_RUN})
          ORDER BY d.instance_id, d.target_root_folder, d.media_title"
    );
    let ids: Vec<String> =
        sqlx::query_scalar(AssertSqlSafe(sql.as_str())).bind(simulation_id).fetch_all(pool).await?;
    let proposed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM decisions
          WHERE simulation_id = ? AND action = 'move' AND status = 'pending'
            AND target_root_folder IS NOT NULL",
    )
    .bind(simulation_id)
    .fetch_one(pool)
    .await?;
    let superseded = usize::try_from(proposed).unwrap_or_default().saturating_sub(ids.len());
    Ok((ids, superseded))
}

/// Load decisions that are still safe to apply.
///
/// Revalidated at apply time rather than trusted from the simulation: the rules,
/// the mappings or the library may have changed since, and a superseded proposal
/// must never be executed. A proposal the current rules no longer send to the
/// same folder is skipped and retired, since nothing else retires it when a
/// rule or a mapping changes and it would otherwise stay on screen as pending,
/// skipped again on every apply.
pub(super) async fn load_pending_moves(
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

pub(super) async fn retire(pool: &SqlitePool, decision_ids: &[&str]) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    routing::supersede_decisions(&mut tx, decision_ids).await?;
    tx.commit().await?;
    Ok(())
}

/// Load applied decisions that can still be rolled back.
pub(super) async fn load_revertible_moves(
    pool: &SqlitePool,
    ids: &[String],
) -> AppResult<Vec<PendingMove>> {
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
pub(super) fn in_given_order(rows: Vec<MoveRow>, ids: &[String]) -> Vec<PendingMove> {
    let position: HashMap<&str, usize> =
        ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let mut moves: Vec<PendingMove> = rows.into_iter().map(PendingMove::from).collect();
    moves.sort_by_key(|mv| position.get(mv.decision_id.as_str()).copied());
    moves
}
