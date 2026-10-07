//! The security log as the owner reads it: the events `services::audit`
//! stored, newest first. No application key reaches it, whatever its scopes.

use super::{Json, Query};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::logs::csv_field;
use crate::api::{Page, paginate};
use crate::error::AppResult;
use crate::services::audit::{Kind, Outcome};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct SecurityEvent {
    pub id: i64,
    pub at: String,
    pub kind: String,
    pub outcome: String,
    /// Who asked, as the authentication mode named them.
    pub subject: Option<String>,
    pub client: Option<String>,
    /// What happened: a dictionary key, read with `params`.
    pub message: String,
    pub params: serde_json::Map<String, serde_json::Value>,
    /// How many more like it the same address sent in the minute after.
    pub repeated: i64,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    at: String,
    kind: String,
    outcome: String,
    subject: Option<String>,
    client: Option<String>,
    message: String,
    params: String,
    repeated: i64,
}

impl From<Row> for SecurityEvent {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            at: row.at,
            kind: row.kind,
            outcome: row.outcome,
            subject: row.subject,
            client: row.client,
            message: row.message,
            params: serde_json::from_str(&row.params).unwrap_or_default(),
            repeated: row.repeated,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SecurityLogQuery {
    pub kind: Option<Kind>,
    pub outcome: Option<Outcome>,
    /// Part of a name or an address.
    pub search: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

const COLUMNS: &str = "id, at, kind, outcome, subject, client, message, params, repeated";

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<SecurityLogQuery>,
) -> AppResult<Json<Page<SecurityEvent>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);
    let (filters, binds) = build_filters(&query);

    let list_sql = format!(
        "SELECT {COLUMNS} FROM security_events WHERE 1=1{filters}
         ORDER BY at DESC, id DESC LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM security_events WHERE 1=1{filters}");
    let mut list_query = sqlx::query_as::<_, Row>(AssertSqlSafe(list_sql.as_str()));
    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
        count_query = count_query.bind(bind);
    }

    let rows = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;
    let total = count_query.fetch_one(&state.pool).await?;
    let events = rows.into_iter().map(SecurityEvent::from).collect();
    Ok(Json(Page::new(events, page, per_page, total)))
}

/// Download the filtered log as CSV, each event said in the configured
/// language.
pub async fn export(
    State(state): State<AppState>,
    Query(query): Query<SecurityLogQuery>,
) -> AppResult<impl IntoResponse> {
    let (filters, binds) = build_filters(&query);
    let sql = format!(
        "SELECT {COLUMNS} FROM security_events WHERE 1=1{filters}
         ORDER BY at DESC, id DESC LIMIT 50000"
    );
    let mut list_query = sqlx::query_as::<_, Row>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
    }
    let rows = list_query.fetch_all(&state.pool).await?;
    let localizer = state.localizer().await;

    let mut csv = String::from("at,event,outcome,subject,client,repeated,message\n");
    for row in rows {
        let event = SecurityEvent::from(row);
        let params: Vec<(String, String)> = event
            .params
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect();
        let params: Vec<(&str, &str)> =
            params.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(&event.at),
            csv_field(&event.kind),
            csv_field(&event.outcome),
            csv_field(event.subject.as_deref().unwrap_or("")),
            csv_field(event.client.as_deref().unwrap_or("")),
            event.repeated,
            csv_field(&localizer.translate(&event.message, &params)),
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"routarr-security-log.csv\""),
    );
    Ok((headers, csv))
}

fn build_filters(query: &SecurityLogQuery) -> (String, Vec<String>) {
    let mut filters = String::new();
    let mut binds = Vec::new();
    if let Some(kind) = query.kind {
        filters.push_str(" AND kind = ?");
        binds.push(kind.as_str().to_string());
    }
    if let Some(outcome) = query.outcome {
        filters.push_str(" AND outcome = ?");
        binds.push(outcome.as_str().to_string());
    }
    if let Some(search) = query.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        filters.push_str(" AND (subject LIKE ? ESCAPE '\\' OR client LIKE ? ESCAPE '\\')");
        let pattern = format!("%{}%", crate::db::escape_like(search));
        binds.push(pattern.clone());
        binds.push(pattern);
    }
    (filters, binds)
}
