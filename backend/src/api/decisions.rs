//! Decision history and execution.

use super::{Json, Query};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use sqlx::AssertSqlSafe;

use crate::api::auth::Identity;
use crate::api::jobs::{answer, prefers_async};
use crate::api::{Page, paginate};
use crate::error::AppResult;
use crate::jobs::Attribution;
use crate::models::*;
use crate::services::executor;
use crate::state::AppState;

/// sqlx only implements `FromRow` for tuples up to 16 fields, and a decision has
/// more columns than that, so the row is a named struct.
#[derive(Debug, sqlx::FromRow)]
struct DecisionRow {
    id: String,
    media_id: String,
    media_title: String,
    media_type: String,
    instance_id: String,
    instance_name: Option<String>,
    current_root_folder: Option<String>,
    target_root_folder: Option<String>,
    target_category: String,
    matched_rule_id: Option<String>,
    matched_rule_name: Option<String>,
    is_override: bool,
    reasons: String,
    alternatives: String,
    action: String,
    status: String,
    error_message: Option<String>,
    decided_at: String,
    applied_at: Option<String>,
    confidence: f32,
    superseded: bool,
    simulation_id: Option<String>,
    reverted_at: Option<String>,
    actor: Option<String>,
    subject: Option<String>,
    revertible: bool,
}

const DECISION_COLUMNS: &str = "id, media_id, media_title, media_type, instance_id, instance_name,
     current_root_folder, target_root_folder, target_category, matched_rule_id, matched_rule_name,
     is_override, reasons, alternatives, action, status, error_message, decided_at, applied_at,
     confidence, superseded, simulation_id, reverted_at, actor, subject";

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(query): Query<DecisionQuery>,
) -> AppResult<Json<Page<Decision>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);

    let mut filters = String::new();
    let mut binds: Vec<String> = Vec::new();

    let push = |clause: &str, value: String, filters: &mut String, binds: &mut Vec<String>| {
        filters.push_str(clause);
        binds.push(value);
    };

    if let Some(v) = query.instance_id.clone() {
        push(" AND instance_id = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.media_type.clone() {
        push(" AND media_type = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.status.clone() {
        push(" AND status = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.category.clone() {
        push(" AND target_category = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.action.clone() {
        push(" AND action = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.simulation_id.clone() {
        push(" AND simulation_id = ?", v, &mut filters, &mut binds);
    }
    if let Some(v) = query.search.clone() {
        push(
            " AND media_title LIKE ? ESCAPE '\\'",
            format!("%{}%", crate::db::escape_like(&v)),
            &mut filters,
            &mut binds,
        );
    }
    // Superseded proposals are noise by default: they can no longer be applied.
    if !query.include_superseded.unwrap_or(false) {
        filters.push_str(" AND superseded = 0");
    }

    let list_sql = format!(
        "SELECT {DECISION_COLUMNS}, CASE WHEN {} THEN 1 ELSE 0 END AS revertible
         FROM decisions d WHERE 1=1{filters}
         ORDER BY decided_at DESC, media_title ASC LIMIT ? OFFSET ?",
        executor::REVERTIBLE
    );
    let count_sql = format!("SELECT COUNT(*) FROM decisions WHERE 1=1{filters}");

    let mut list_query = sqlx::query_as::<_, DecisionRow>(AssertSqlSafe(list_sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
    }
    let rows = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;

    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in &binds {
        count_query = count_query.bind(bind);
    }
    let total = count_query.fetch_one(&state.pool).await?;

    let decisions = rows
        .into_iter()
        .map(decision_from_row)
        .map(|decision| Decision { subject: identity.shown_subject(decision.subject), ..decision })
        .collect();
    Ok(Json(Page::new(decisions, page, per_page, total)))
}

/// Request to apply selected decisions.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplyDecisionsRequest {
    /// The pending decisions to apply.
    pub decision_ids: Vec<String>,
    /// Move the files on disk with each title. A key needs to be allowed to.
    #[serde(default)]
    pub move_files: bool,
    /// The guardrails the caller has looked at and accepts, by name.
    /// Answering one question answers no other.
    #[serde(default)]
    pub confirm: executor::Confirmed,
}

/// Apply everything one simulation proposed, in slices.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplyAllRequest {
    /// The simulation whose proposals to apply.
    pub simulation_id: String,
    /// Move the files on disk with each title. A key needs to be allowed to.
    #[serde(default)]
    pub move_files: bool,
    /// Always asked: `batch`, whose one question states the count and any
    /// sleeping or full destination, named in the answer's `includes`.
    // A mass operation by definition, so the confirmation threshold has
    // nothing to say about it.
    #[serde(default)]
    pub confirm: executor::Confirmed,
}

/// Request to roll a previously applied decision back to its original folder.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RevertDecisionsRequest {
    /// The applied decisions to move back.
    pub decision_ids: Vec<String>,
    /// Move the files on disk with each title. A key needs to be allowed to.
    #[serde(default)]
    pub move_files: bool,
    /// The guardrails the caller has looked at, by name, as for an apply.
    #[serde(default)]
    pub confirm: executor::Confirmed,
}

pub async fn apply(
    State(state): State<AppState>,
    // Who is moving files. The middleware puts an identity on every protected
    // request, so this cannot fail, and this is the write worth attributing.
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(req): Json<ApplyDecisionsRequest>,
) -> AppResult<Response> {
    let task = state.clone();
    let work =
        on_behalf_of(identity, req.move_files, &req.confirm, move |confirmed, by| async move {
            executor::apply_decisions(&task, &req.decision_ids, req.move_files, &confirmed, &by)
                .await
        })?;
    answer(&state, prefers_async(&headers), work).await
}

/// Apply every move a simulation proposed, in slices of `batch_limit`.
pub async fn apply_all(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(req): Json<ApplyAllRequest>,
) -> AppResult<Response> {
    let task = state.clone();
    let work =
        on_behalf_of(identity, req.move_files, &req.confirm, move |confirmed, by| async move {
            let (simulation, move_files) = (&req.simulation_id, req.move_files);
            executor::apply_simulation_in_batches(&task, simulation, move_files, &confirmed, &by)
                .await
        })?;
    answer(&state, prefers_async(&headers), work).await
}

/// Undo previously applied moves.
pub async fn revert(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(req): Json<RevertDecisionsRequest>,
) -> AppResult<Response> {
    let task = state.clone();
    let work =
        on_behalf_of(identity, req.move_files, &req.confirm, move |confirmed, by| async move {
            executor::revert_decisions(&task, &req.decision_ids, req.move_files, &confirmed, &by)
                .await
        })?;
    answer(&state, prefers_async(&headers), work).await
}

/// An executor call as `identity` may make it: a key that may not move files is
/// refused before anything runs, the answers it was not given are dropped, and
/// a question it may not answer comes back marked for a person. One function
/// for the three routes that move files, so none of them forgets a step.
fn on_behalf_of<T, F>(
    identity: Identity,
    move_files: bool,
    sent: &executor::Confirmed,
    run: impl FnOnce(executor::Confirmed, Attribution) -> F,
) -> AppResult<impl Future<Output = AppResult<T>> + Send + 'static>
where
    F: Future<Output = AppResult<T>> + Send + 'static,
{
    identity.may_move_files(move_files)?;
    let work = run(identity.answerable(sent), identity.attribution());
    Ok(async move { work.await.map_err(|e| identity.refer(e)) })
}

fn decision_from_row(r: DecisionRow) -> Decision {
    Decision {
        actor: r.actor,
        subject: r.subject,
        revertible: r.revertible,
        id: r.id,
        media_id: r.media_id,
        media_title: r.media_title,
        media_type: r.media_type,
        instance_id: r.instance_id,
        instance_name: r.instance_name,
        current_root_folder: r.current_root_folder,
        target_root_folder: r.target_root_folder,
        target_category: r.target_category,
        matched_rule_id: r.matched_rule_id,
        matched_rule_name: r.matched_rule_name,
        is_override: r.is_override,
        reasons: serde_json::from_str(&r.reasons).unwrap_or_default(),
        alternatives: serde_json::from_str(&r.alternatives).unwrap_or_default(),
        action: r.action,
        status: r.status,
        error_message: r.error_message,
        decided_at: r.decided_at,
        applied_at: r.applied_at,
        confidence: r.confidence,
        superseded: r.superseded,
        simulation_id: r.simulation_id,
        reverted_at: r.reverted_at,
    }
}
