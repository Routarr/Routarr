//! The paths the contract renamed, still answered under their old names until their removal
//! before 1.0, and marked deprecated on every answer. One table feeds the router's aliases, the
//! application key grants and the contract, so an old path can never reach more than its new one.
//! Each call is counted against its caller, so the operator learns which script to move.

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use sqlx::SqlitePool;

use crate::api::auth::Identity;
use crate::state::AppState;

/// Each old path template beside the one that replaced it, under `/api/v1`.
pub const DEPRECATED_PATHS: &[(&str, &str)] = &[
    ("/overrides", "/exceptions"),
    ("/overrides/external", "/exceptions/external"),
    ("/overrides/{id}", "/exceptions/{id}"),
    ("/jobs", "/tasks"),
    ("/jobs/{id}", "/tasks/{id}"),
    ("/jobs/{id}/cancel", "/tasks/{id}/cancel"),
    ("/logs", "/move-log"),
    ("/logs/export", "/move-log/export"),
    ("/media", "/library"),
    ("/media/facets", "/library/facets"),
    ("/media/{id}", "/library/{id}"),
    ("/media/{id}/explain", "/library/{id}/explain"),
    ("/media/{id}/keep", "/library/{id}/keep"),
    ("/metadata/providers", "/sources"),
    ("/metadata/refresh", "/sources/refresh"),
];

/// When the old paths were deprecated, an RFC 9745 date: 21 October 2026.
const DEPRECATED_AT: &str = "@1792540800";

/// Where a person reads which path replaced each one.
const DEPRECATION_PAGE: &str =
    "<https://routarr.app/api/#deprecated>; rel=\"deprecation\"; type=\"text/html\"";

/// The path that replaced the deprecated `template`, `None` for a current one.
pub fn replacement(template: &str) -> Option<&'static str> {
    DEPRECATED_PATHS.iter().find(|(old, _)| *old == template).map(|(_, new)| *new)
}

/// `template` as its current path: an old one's replacement, any other itself.
pub fn current(template: &str) -> &str {
    replacement(template).unwrap_or(template)
}

/// The route a request matched, its prefix up to `/api/v1` left out.
pub fn route_of(request: &Request) -> Option<&str> {
    let matched = request.extensions().get::<MatchedPath>()?.as_str();
    Some(matched.rsplit_once("/api/v1").map_or(matched, |(_, route)| route))
}

/// On an answer to a deprecated path, `Deprecation` and `Link` (RFC 9745), and a `Location`
/// written under the old names the caller used, so a script following it stays on them. Runs
/// behind `authenticate`, whose `Identity` names the caller counted.
pub async fn signal(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let called = route_of(&request).and_then(|route| Some((route.to_owned(), replacement(route)?)));
    if let Some((route, new)) = &called {
        let identity = request.extensions().get::<Identity>();
        record(&state.pool, identity, request.method().as_str(), route, new).await;
    }
    let mut response = next.run(request).await;
    if called.is_some() {
        let headers = response.headers_mut();
        headers.insert("deprecation", HeaderValue::from_static(DEPRECATED_AT));
        headers.append(header::LINK, HeaderValue::from_static(DEPRECATION_PAGE));
        let renamed = headers
            .get(header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(under_old_names)
            .and_then(|location| HeaderValue::from_str(&location).ok());
        if let Some(location) = renamed {
            headers.insert(header::LOCATION, location);
        }
    }
    response
}

/// Counts a call to the old path `route` against its caller, and logs it once a day per caller
/// and route. A count that fails is logged and never fails the call.
async fn record(
    pool: &SqlitePool,
    identity: Option<&Identity>,
    method: &str,
    route: &str,
    new: &str,
) {
    let (kind, id, name) = match identity.and_then(|identity| identity.application.as_ref()) {
        Some(grant) => ("application", grant.id.as_str(), grant.name.as_str()),
        None => ("person", "", identity.and_then(Identity::actor).unwrap_or_default()),
    };
    let counted = async {
        sqlx::query(
            "INSERT INTO deprecated_calls
                    (caller_kind, caller_id, caller_name, method, path, calls, last_called_at)
             VALUES (?, ?, ?, ?, ?, 1, datetime('now'))
             ON CONFLICT (caller_kind, caller_id, method, path) DO UPDATE
                SET calls = calls + 1, last_called_at = excluded.last_called_at,
                    caller_name = excluded.caller_name",
        )
        .bind(kind)
        .bind(id)
        .bind(name)
        .bind(method)
        .bind(route)
        .execute(pool)
        .await?;
        // One statement decides, so two calls at once write one line.
        sqlx::query_scalar::<_, i64>(
            "UPDATE deprecated_calls SET logged_on = date('now')
              WHERE caller_kind = ? AND caller_id = ? AND method = ? AND path = ?
                AND logged_on <> date('now')
             RETURNING 1",
        )
        .bind(kind)
        .bind(id)
        .bind(method)
        .bind(route)
        .fetch_optional(pool)
        .await
    };
    match counted.await {
        Ok(Some(_)) => tracing::warn!(
            caller = name,
            method,
            path = route,
            replacement = new,
            "A deprecated API path was called. It answers until a release before 1.0 removes it"
        ),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "A call to a deprecated API path was not counted"),
    }
}

/// Who called an old path in the last 30 days: an application key's name, or `None` for the
/// owner's key or a session, with each `METHOD /path` called and the day of the last call.
pub struct Caller {
    pub application: Option<String>,
    pub routes: Vec<String>,
    pub last_day: String,
}

pub async fn recent_callers(pool: &SqlitePool) -> sqlx::Result<Vec<Caller>> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT caller_kind, caller_id, caller_name, method, path, date(last_called_at)
           FROM deprecated_calls
          WHERE last_called_at >= datetime('now', '-30 days')
          ORDER BY caller_kind, caller_name, caller_id, path, method",
    )
    .fetch_all(pool)
    .await?;
    let mut callers: Vec<(String, String, Caller)> = Vec::new();
    for (kind, id, name, method, path, day) in rows {
        let route = format!("{method} /api/v1{path}");
        match callers.last_mut() {
            Some((k, i, caller)) if *k == kind && *i == id => {
                caller.routes.push(route);
                caller.last_day = caller.last_day.clone().max(day);
            }
            _ => {
                let application = (kind == "application").then_some(name);
                let caller = Caller { application, routes: vec![route], last_day: day };
                callers.push((kind, id, caller));
            }
        }
    }
    Ok(callers.into_iter().map(|(_, _, caller)| caller).collect())
}

/// `location` with a resource renamed back to its old name, as `/api/v1/tasks/x` to
/// `/api/v1/jobs/x`. `None` when it names no renamed resource.
fn under_old_names(location: &str) -> Option<String> {
    let (base, path) = location.split_once("/api/v1")?;
    DEPRECATED_PATHS.iter().find_map(|(old, new)| {
        let (old, new) = (old.strip_suffix("{id}")?, new.strip_suffix("{id}")?);
        let id = path.strip_prefix(new)?;
        Some(format!("{base}/api/v1{old}{id}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_under_a_new_name_is_written_back_under_the_old_one() {
        assert_eq!(
            under_old_names("/routarr/api/v1/tasks/abc").as_deref(),
            Some("/routarr/api/v1/jobs/abc")
        );
        assert_eq!(
            under_old_names("/api/v1/exceptions/o-1").as_deref(),
            Some("/api/v1/overrides/o-1")
        );
        assert_eq!(under_old_names("/api/v1/rules/r-1"), None);
    }

    /// One row per old path, every new path among the current ones, none deprecated twice.
    #[test]
    fn the_table_names_each_old_path_once_and_never_a_new_one() {
        let olds: std::collections::HashSet<&str> =
            DEPRECATED_PATHS.iter().map(|(o, _)| *o).collect();
        assert_eq!(olds.len(), DEPRECATED_PATHS.len());
        assert!(DEPRECATED_PATHS.iter().all(|(_, new)| replacement(new).is_none()));
    }
}
