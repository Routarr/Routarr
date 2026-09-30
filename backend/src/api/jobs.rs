//! Tasks feed: the running and recent background jobs.

use super::{Json, Query};
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::{Page, paginate};
use crate::error::{AppError, AppResult};
use crate::jobs::registry::render_detail;
use crate::localization::Localizer;
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub trigger: String,
    /// Who asked: an application's name, or the person a sign-in mode names.
    pub subject: Option<String>,
    pub instance_id: Option<String>,
    /// In the interface language, from `detail_key` when the row has one.
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
}

const JOB_COLUMNS: &str =
    "id, kind, status, trigger, subject, instance_id, detail, detail_key, detail_params,
     progress_current, progress_total, error_message, started_at, finished_at";

impl Job {
    fn localized(mut self, localizer: &Localizer) -> Self {
        self.detail = render_detail(
            localizer,
            self.detail_key.as_deref(),
            self.detail_params.as_deref(),
            self.detail.take(),
        );
        self
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct JobQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

pub async fn list(
    State(state): State<AppState>,
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

    let list_sql = format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE 1=1{filters} ORDER BY started_at DESC LIMIT ? OFFSET ?"
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
    let jobs = jobs.into_iter().map(|job| job.localized(&localizer)).collect();

    Ok(Json(Page::new(jobs, page, per_page, total)))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Job>> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?");
    let job = sqlx::query_as::<_, Job>(AssertSqlSafe(sql.as_str()))
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Job {id} not found")))?;
    Ok(Json(job.localized(&state.localizer().await)))
}
