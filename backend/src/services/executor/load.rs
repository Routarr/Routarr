//! The decisions a run writes, read again and revalidated just before it.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashMap;
use tracing::{info, warn};

use super::guards::REVERTIBLE;
use super::{MoveRow, PendingMove};
use crate::db::Conditions;
use crate::error::{AppError, AppResult};
use crate::models::DecisionFilter;
use crate::services::routing;
use crate::state::AppState;

/// Each id once, in the order first given: an id sent twice is one decision.
pub(super) fn distinct(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().filter(|id| seen.insert(id.as_str())).cloned().collect()
}

/// The proposals to apply for `ids`, and how many were replaced. A pass after each sync writes
/// every proposal again: an identical successor is applied in its place, one replaced by
/// another move or by none is counted and left, and any other id passes as given.
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

/// Refuse ids that name no decision before a task starts: a run over nothing would answer
/// success. In English, since the interface sent the ids and nobody typed them. Called after
/// the guardrails, which ask about the moves that exist.
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

/// The moves a simulation proposed that `filter` keeps, as SQL over `decisions run` and the
/// values it binds. The filter reads the simulation's own rows, what a person reviewed.
fn proposed_by_run(simulation_id: &str, filter: &DecisionFilter) -> (String, Vec<String>) {
    let mut run = Conditions::on("run");
    filter.narrow(&mut run);
    let sql = format!(
        "run.simulation_id = ? AND run.action = 'move' AND run.status = 'pending'
         AND run.target_root_folder IS NOT NULL{}",
        run.sql()
    );
    (sql, std::iter::once(simulation_id.to_string()).chain(run.binds().iter().cloned()).collect())
}

/// The current proposals standing for the moves a simulation proposed and `filter` keeps:
/// each still pending, or the identical move a newer pass proposed in its place (see
/// [`current_proposals`]). A newer pass can only take titles out of the set. Its SQL and binds.
pub(super) fn current_for_run(
    simulation_id: &str,
    filter: &DecisionFilter,
) -> (String, Vec<String>) {
    let (proposed, binds) = proposed_by_run(simulation_id, filter);
    let sql = format!(
        "SELECT cur.id FROM decisions run
           JOIN decisions cur
             ON cur.media_id = run.media_id
            AND cur.status = 'pending' AND cur.superseded = 0 AND cur.action = 'move'
            AND cur.current_root_folder IS run.current_root_folder COLLATE path
            AND cur.target_root_folder = run.target_root_folder COLLATE path
          WHERE {proposed}"
    );
    (sql, binds)
}

/// The moves a simulation proposed and `filter` keeps that are still worth attempting, and
/// how many of them were replaced by another move.
pub(super) async fn applicable_from_simulation(
    pool: &SqlitePool,
    simulation_id: &str,
    filter: &DecisionFilter,
) -> AppResult<(Vec<String>, usize)> {
    let (current, binds) = current_for_run(simulation_id, filter);
    let sql = format!(
        "SELECT d.id FROM decisions d WHERE d.id IN ({current})
          ORDER BY d.instance_id, d.target_root_folder, d.media_title"
    );
    let mut query = sqlx::query_scalar::<_, String>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        query = query.bind(bind);
    }
    let ids = query.fetch_all(pool).await?;

    let (proposed, binds) = proposed_by_run(simulation_id, filter);
    let sql = format!("SELECT COUNT(*) FROM decisions run WHERE {proposed}");
    let mut query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        query = query.bind(bind);
    }
    let proposed = query.fetch_one(pool).await?;
    let superseded = usize::try_from(proposed).unwrap_or_default().saturating_sub(ids.len());
    Ok((ids, superseded))
}

/// The applied moves a revert may undo that `matching`, written on `decisions d`, keeps,
/// instance by instance and the latest first.
pub(super) async fn revertible_matching(
    pool: &SqlitePool,
    matching: &Conditions,
) -> AppResult<Vec<String>> {
    let sql = format!(
        "SELECT d.id FROM decisions d WHERE {REVERTIBLE}{}
          ORDER BY d.instance_id, d.applied_at DESC, d.media_title, d.id",
        matching.sql()
    );
    let mut query = sqlx::query_scalar::<_, String>(AssertSqlSafe(sql.as_str()));
    for bind in matching.binds() {
        query = query.bind(bind);
    }
    Ok(query.fetch_all(pool).await?)
}

/// The decisions still safe to apply, revalidated now since the rules, mappings or library may
/// have changed. A proposal the rules no longer send to the same folder is skipped and retired,
/// or it stays pending on screen, skipped again on every apply.
pub(super) async fn load_pending_moves(
    pool: &SqlitePool,
    ids: &[String],
) -> AppResult<Vec<PendingMove>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = crate::db::placeholders(ids.len());

    let sql = format!(
        "SELECT d.id, d.media_id, d.media_title, d.instance_id, d.current_root_folder,
                d.target_root_folder, m.arr_id, m.media_type, m.tmdb_id, m.tvdb_id, m.imdb_id
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         WHERE d.id IN ({placeholders})
           AND d.status = 'pending'
           AND d.superseded = 0
           AND d.action = 'move'
           AND d.target_root_folder IS NOT NULL
           AND m.current_root_folder IS d.current_root_folder COLLATE path"
    );
    // The last clause drops a title moved since it was proposed. `IS` matches a null to a null.
    let mut query = sqlx::query_as::<_, MoveRow>(AssertSqlSafe(sql.as_str()));
    for id in ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let media_ids: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
    let targets = routing::revalidated_targets(pool, &media_ids).await?;
    let (current, stale): (Vec<MoveRow>, Vec<MoveRow>) = rows.into_iter().partition(|row| {
        targets
            .get(&row.1)
            .and_then(Option::as_deref)
            .is_some_and(|target| crate::paths::key(target) == crate::paths::key(&row.5))
    });

    if !stale.is_empty() {
        let stale_ids: Vec<&str> = stale.iter().map(|row| row.0.as_str()).collect();
        // Only the skip keeps the library safe, so a failed retirement fails no current move.
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
    let mut tx = crate::db::write_transaction(pool).await?;
    routing::supersede_decisions(&mut tx, decision_ids).await?;
    tx.commit().await?;
    Ok(())
}

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
                d.current_root_folder, m.arr_id, m.media_type, m.tmdb_id, m.tvdb_id, m.imdb_id
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
