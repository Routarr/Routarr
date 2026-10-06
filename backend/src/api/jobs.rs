//! Tasks feed: the running and recent background jobs.

use super::{Json, Query};
use axum::extract::State;

use super::Path;
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::auth::Identity;
use crate::api::{Page, paginate};
use crate::error::{AppError, AppResult};
use crate::jobs::registry::render_detail;
use crate::localization::Localizer;
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Job {
    pub id: String,
    /// `sync`, `sync_all`, `enrich`, `simulate`, `preview` (a simulation that
    /// stores nothing), `apply`, `revert`, `backup`, `maintenance` or
    /// `scheduler`.
    pub kind: String,
    /// `running`, `success`, `failed`, or `cancelled` for an apply or a
    /// revert somebody stopped.
    pub status: String,
    /// What set the task off: `manual`, `schedule`, `webhook`, `api`, or `auto`
    /// for what the automation runs after a sync.
    pub trigger: String,
    /// Who asked: an application's name, or the person a sign-in mode names.
    /// An application key reads its own name and null for anyone else.
    pub subject: Option<String>,
    pub instance_id: Option<String>,
    /// What the task did, in the interface language.
    // Rendered from `detail_key` when the row has one.
    pub detail: Option<String>,
    #[serde(skip)]
    pub detail_key: Option<String>,
    #[serde(skip)]
    pub detail_params: Option<String>,
    pub progress_current: i64,
    pub progress_total: i64,
    pub error_message: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// What a finished task answered: the report the same call gives when the
    /// caller waits, an `ApplyReport`, a `BatchApplyReport` or a `SyncReport`,
    /// or for a simulation its `SimulationResult`. A stored simulation's comes
    /// without `decisions`, which `GET /decisions?simulation_id=` lists, and a
    /// preview's keeps the ones it returned. An apply or a revert that failed
    /// keeps the report of what it attempted. Null while the task runs, when
    /// any other task failed, for a kind no call starts, and in the list
    /// unless it asks for it.
    #[sqlx(skip)]
    pub result: Option<serde_json::Value>,
    #[serde(skip)]
    #[sqlx(rename = "result")]
    pub stored_result: Option<String>,
}

macro_rules! job_fields {
    () => {
        "id, kind, status, trigger, subject, instance_id, detail, detail_key, detail_params,
         progress_current, progress_total, error_message, started_at, finished_at"
    };
}

const JOB_COLUMNS: &str = concat!(job_fields!(), ", result");

/// The list's columns: a report holds up to thousands of decisions, and the
/// list is polled.
const LISTED_COLUMNS: &str = concat!(job_fields!(), ", NULL AS result");

impl Job {
    fn localized(mut self, localizer: &Localizer) -> Self {
        self.detail = render_detail(
            localizer,
            self.detail_key.as_deref(),
            self.detail_params.as_deref(),
            self.detail.take(),
        );
        self.result =
            self.stored_result.as_deref().and_then(|stored| serde_json::from_str(stored).ok());
        self
    }

    fn seen_by(self, identity: &Identity) -> Self {
        Self { subject: identity.shown_subject(self.subject), ..self }
    }
}

/// What a call answers when the caller asked not to wait: the task it started.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Accepted {
    /// Follow it with `GET /jobs/{job_id}`, whose `result` holds the report.
    pub job_id: String,
}

/// Whether the caller prefers not to wait (RFC 7240, `Prefer: respond-async`).
pub fn prefers_async(headers: &HeaderMap) -> bool {
    headers
        .get_all("prefer")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|preference| preference.trim().eq_ignore_ascii_case("respond-async"))
}

/// Run `work` on a task of its own, and answer the way the caller prefers.
///
/// Waited for, it answers the report. With `respond_async`, it answers 202 as
/// soon as the work has started its job, with that job's address, and a
/// refusal the work makes before starting one (a guardrail's question, a lock
/// held) still answers at once. The work runs to its end either way, whatever
/// the caller does after asking.
pub async fn answer<T>(
    state: &AppState,
    respond_async: bool,
    work: impl Future<Output = AppResult<T>> + Send + 'static,
) -> AppResult<Response>
where
    T: Serialize + Send + 'static,
{
    let (started, job) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(crate::jobs::announcing(started, work));
    if respond_async && let Ok(job_id) = job.await {
        let location = format!("{}/api/v1/jobs/{job_id}", state.config.base_path);
        return Ok((
            StatusCode::ACCEPTED,
            [(header::LOCATION, location), (PREFERENCE_APPLIED, "respond-async".to_string())],
            Json(Accepted { job_id }),
        )
            .into_response());
    }
    let report = task
        .await
        .map_err(|e| AppError::Internal(format!("the task ended before it reported: {e}")))??;
    Ok(Json(report).into_response())
}

/// Says the server honoured the preference (RFC 7240).
const PREFERENCE_APPLIED: HeaderName = HeaderName::from_static("preference-applied");

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JobQuery {
    /// Only the tasks in this status.
    pub status: Option<String>,
    /// Only the tasks of this kind.
    pub kind: Option<String>,
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
    /// `result` to have each task carry its report, as `GET /jobs/{job_id}`
    /// does. Left out, `result` is null in the list.
    pub include: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(query): Query<JobQuery>,
) -> AppResult<Json<Page<Job>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);

    let mut filters = String::new();
    let mut binds = Vec::new();
    if let Some(status) = query.status.clone() {
        filters.push_str(" AND status = ?");
        binds.push(status);
    }
    if let Some(kind) = query.kind.clone() {
        filters.push_str(" AND kind = ?");
        binds.push(kind);
    }

    let columns = match query.include.as_deref() {
        Some(asked) if asked.split(',').any(|part| part.trim() == "result") => JOB_COLUMNS,
        _ => LISTED_COLUMNS,
    };
    let list_sql = format!(
        "SELECT {columns} FROM jobs WHERE 1=1{filters} ORDER BY started_at DESC, id DESC LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM jobs WHERE 1=1{filters}");

    let mut list_query = sqlx::query_as::<_, Job>(AssertSqlSafe(list_sql.as_str()));
    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));

    for bind in &binds {
        list_query = list_query.bind(bind);
        count_query = count_query.bind(bind);
    }

    let jobs = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;
    let total = count_query.fetch_one(&state.pool).await?;
    let localizer = state.localizer().await;
    let jobs = jobs.into_iter().map(|job| job.localized(&localizer).seen_by(&identity)).collect();

    Ok(Json(Page::new(jobs, page, per_page, total)))
}

pub async fn get_one(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<Job>> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?");
    let job = sqlx::query_as::<_, Job>(AssertSqlSafe(sql.as_str()))
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Job {id} not found")))?;
    Ok(Json(job.localized(&state.localizer().await).seen_by(&identity)))
}

/// Ask a running apply or revert to stop before its next move. What already
/// reached the Arr stays done, and the task's report says where it stopped.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    if state.jobs.cancel(&id) {
        return Ok(StatusCode::ACCEPTED);
    }
    let known: Option<String> = sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    match known {
        None => Err(AppError::NotFound(format!("Job {id} not found"))),
        Some(_) => Err(AppError::Conflict(
            state.localizer().await.translate("ErrorTaskNotCancellable", &[]),
        )),
    }
}
