//! Decision history and execution.

use super::{Json, Query};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use sqlx::AssertSqlSafe;

use crate::api::auth::Identity;
use crate::api::tasks::{answer, prefers_async};
use crate::api::{Page, paginate};
use crate::db::Conditions;
use crate::error::{AppError, AppResult};
use crate::jobs::Attribution;
use crate::models::*;
use crate::services::{executor, routing};
use crate::state::AppState;

/// A named row, since a tuple `query_as` target stops at 16 fields.
#[derive(Debug, sqlx::FromRow)]
struct DecisionRow {
    id: String,
    media_id: String,
    media_title: String,
    media_type: String,
    instance_id: String,
    instance_name: Option<String>,
    current_root_folder: Option<String>,
    current_category: Option<String>,
    target_root_folder: Option<String>,
    target_category: String,
    matched_rule_id: Option<String>,
    matched_rule_name: Option<String>,
    is_override: bool,
    reasons: String,
    alternatives: String,
    action: DecisionAction,
    status: DecisionStatus,
    error_message: Option<String>,
    decided_at: String,
    applied_at: Option<String>,
    confidence: f32,
    superseded: bool,
    simulation_id: Option<String>,
    reverted_at: Option<String>,
    actor: Option<String>,
    subject: Option<String>,
    subject_key: Option<String>,
    revertible: bool,
    in_library: bool,
}

const DECISION_COLUMNS: &str = "id, media_id, media_title, media_type, instance_id, instance_name,
     current_root_folder, target_root_folder, target_category, matched_rule_id, matched_rule_name,
     is_override, reasons, alternatives, action, status, error_message, decided_at, applied_at,
     confidence, superseded, simulation_id, reverted_at, actor, subject, subject_key";

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(query): Query<DecisionQuery>,
) -> AppResult<Json<Page<Decision>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);

    let mut conditions = Conditions::on("d");
    query.filter().narrow(&mut conditions);
    conditions
        .equal("status", query.status.as_deref())
        .equal("action", query.action.as_deref())
        .equal("simulation_id", query.simulation_id.as_deref())
        .from("decided_at", stored_time("since", query.since.as_deref())?.as_deref())
        .before("decided_at", stored_time("until", query.until.as_deref())?.as_deref());
    // Hidden by default: a superseded proposal cannot be applied.
    if !query.include_superseded.unwrap_or(false) {
        conditions.holds("d.superseded = 0");
    }
    match query.revertible {
        Some(true) => conditions.holds(executor::REVERTIBLE),
        Some(false) => conditions.fails(executor::REVERTIBLE),
        None => &mut conditions,
    };

    let list_sql = format!(
        "SELECT {DECISION_COLUMNS}, CASE WHEN {} THEN 1 ELSE 0 END AS revertible,
         EXISTS (SELECT 1 FROM media m WHERE m.id = d.media_id) AS in_library,
         (SELECT rf.category FROM root_folders rf
           WHERE rf.instance_id = d.instance_id AND rf.path = d.current_root_folder COLLATE path
             AND rf.category IS NOT NULL AND rf.category != '') AS current_category
         FROM decisions d WHERE 1=1{}
         ORDER BY decided_at DESC, media_title ASC, id ASC LIMIT ? OFFSET ?",
        executor::REVERTIBLE,
        conditions.sql()
    );
    let count_sql = format!("SELECT COUNT(*) FROM decisions d WHERE 1=1{}", conditions.sql());

    let mut list_query = sqlx::query_as::<_, DecisionRow>(AssertSqlSafe(list_sql.as_str()));
    for bind in conditions.binds() {
        list_query = list_query.bind(bind);
    }
    let rows = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;

    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in conditions.binds() {
        count_query = count_query.bind(bind);
    }
    let total = count_query.fetch_one(&state.pool).await?;

    let localizer = state.localizer().await;
    let decisions = rows
        .into_iter()
        .map(|row| decision_from_row(row, &localizer))
        .map(|decision| Decision {
            subject: identity.shown_subject(decision.subject, decision.subject_key.as_deref()),
            ..decision
        })
        .collect();
    Ok(Json(Page::new(decisions, page, per_page, total)))
}

/// A time a caller sent, in the shape the decisions store theirs. Its refusal stays English,
/// since a client sends the time and nobody types it.
fn stored_time(field: &str, raw: Option<&str>) -> AppResult<Option<String>> {
    raw.map(|raw| {
        routing::parse_timestamp(raw)
            .map(routing::format_timestamp)
            .ok_or_else(|| AppError::BadRequest(format!("{field} is not a time in RFC 3339.")))
    })
    .transpose()
}

/// Request to apply selected decisions.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ApplyAllRequest {
    /// The simulation whose decisions to apply.
    pub simulation_id: String,
    /// Only the moves this filter keeps, read on what the simulation proposed. None set
    /// applies every move.
    #[serde(default)]
    pub filter: DecisionFilter,
    /// Move the files on disk with each title. A key needs to be allowed to.
    #[serde(default)]
    pub move_files: bool,
    /// Always asked: `batch`, whose one question states the count, the filter and any
    /// sleeping or full destination, named in the answer's `includes`.
    // A mass operation by definition, so the confirmation threshold does not apply.
    #[serde(default)]
    pub confirm: executor::Confirmed,
}

/// Undo every applied move a filter matches, in slices.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevertAllRequest {
    /// Only the moves this filter keeps. None set takes every move a revert may still undo.
    #[serde(default)]
    pub filter: DecisionFilter,
    /// Only the moves this simulation proposed.
    pub simulation_id: Option<String>,
    /// Only the moves decided at this time or later, in RFC 3339.
    #[schema(format = DateTime)]
    pub since: Option<String>,
    /// Only the moves decided before this time, in RFC 3339.
    #[schema(format = DateTime)]
    pub until: Option<String>,
    /// Move the files on disk back with each title. A key needs to be allowed to.
    #[serde(default)]
    pub move_files: bool,
    /// Always asked: `batch`, as for an apply of a whole simulation.
    #[serde(default)]
    pub confirm: executor::Confirmed,
}

/// Request to roll a previously applied decision back to its original folder.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
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
            let (simulation, filter) = (&req.simulation_id, &req.filter);
            executor::apply_simulation_in_batches(
                &task,
                simulation,
                filter,
                req.move_files,
                &confirmed,
                &by,
            )
            .await
        })?;
    answer(&state, prefers_async(&headers), work).await
}

/// Undo every applied move a filter matches, in slices of `batch_limit`.
pub async fn revert_all(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(req): Json<RevertAllRequest>,
) -> AppResult<Response> {
    let scope = executor::RevertScope {
        filter: req.filter,
        simulation_id: req.simulation_id,
        since: stored_time("since", req.since.as_deref())?,
        until: stored_time("until", req.until.as_deref())?,
    };
    let task = state.clone();
    let work =
        on_behalf_of(identity, req.move_files, &req.confirm, move |confirmed, by| async move {
            executor::revert_in_batches(&task, &scope, req.move_files, &confirmed, &by).await
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

/// An executor call as `identity` may make it: refused first when the key may not move files,
/// with only the answers it may give, and a question it may not answer marked for a person.
/// The routes that move files share it, so none forgets a step.
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

/// A stored decision, its words said in `localizer`'s language.
fn decision_from_row(r: DecisionRow, localizer: &crate::localization::Localizer) -> Decision {
    let reason_lines = crate::services::routing::stored_reasons(&r.reasons, localizer);
    // The exception's name is the reader's word for it, whatever the run's was.
    let matched_rule_name = match r.is_override {
        true => Some(localizer.translate("ManualOverrideRuleName", &[])),
        false => r.matched_rule_name,
    };
    Decision {
        actor: r.actor,
        subject: r.subject,
        subject_key: r.subject_key,
        revertible: r.revertible,
        in_library: r.in_library,
        id: r.id,
        media_id: r.media_id,
        media_title: r.media_title,
        media_type: r.media_type,
        instance_id: r.instance_id,
        instance_name: r.instance_name,
        current_root_folder: r.current_root_folder,
        current_category: r.current_category,
        target_root_folder: r.target_root_folder,
        target_category: r.target_category,
        matched_rule_id: r.matched_rule_id,
        matched_rule_name,
        is_override: r.is_override,
        reasons: reason_lines.iter().map(|line| line.text.clone()).collect(),
        reason_lines,
        alternatives: crate::services::routing::stored_alternatives(&r.alternatives, localizer),
        stored_reasons: String::new(),
        stored_alternatives: String::new(),
        action: r.action,
        status: r.status,
        error_message: r.error_message.map(|stored| crate::localization::said(&stored, localizer)),
        decided_at: r.decided_at,
        applied_at: r.applied_at,
        confidence: r.confidence,
        superseded: r.superseded,
        simulation_id: r.simulation_id,
        reverted_at: r.reverted_at,
    }
}
