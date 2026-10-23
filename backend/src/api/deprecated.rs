//! The paths the contract renamed, still answered under their old names until their removal
//! before 1.0, and marked deprecated on every answer. One table feeds the router's aliases, the
//! application key grants and the contract, so an old path can never reach more than its new one.
//! Each call is counted against its caller, so the operator learns which script to move.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot};

use crate::api::auth::Identity;
use crate::state::AppState;

/// How many calls wait for the writer before one is dropped uncounted.
const QUEUED: usize = 4096;

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
        let method = request.method().as_str();
        state.deprecated_calls.count(&state.pool, identity, method, route, new).await;
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

/// A caller and the old path it called: kind, caller id, method, path.
type Key = (&'static str, String, String, String);

struct Call {
    key: Key,
    name: String,
    at: String,
    logged: bool,
}

enum Order {
    Count(Call),
    /// Answered once every call sent before it is stored.
    Flush(oneshot::Sender<()>),
}

/// The calls to old paths, stored by a task of its own: counting a call never waits behind a
/// writer, which a sync holds for seconds.
#[derive(Default)]
pub struct Calls {
    store: Option<mpsc::Sender<Order>>,
    /// The day each caller and path was last logged.
    logged: Mutex<HashMap<Key, String>>,
}

impl Calls {
    pub fn counting(pool: SqlitePool) -> Self {
        let (store, orders) = mpsc::channel(QUEUED);
        tokio::spawn(write(pool, orders));
        Self { store: Some(store), logged: Mutex::default() }
    }

    /// Counts a call to the old path `route` against its caller, and logs it once a day per
    /// caller and route. A count that fails is logged and never fails the call.
    async fn count(
        &self,
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
        let key: Key = (kind, id.to_owned(), method.to_owned(), route.to_owned());
        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let known = self.day_logged(&key).as_deref() == Some(today.as_str());
        let logged = !known && !logged_on(pool, &key, &today).await;
        self.logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), today.clone());
        if logged {
            tracing::warn!(
                caller = name,
                method,
                path = route,
                replacement = new,
                "A deprecated API path was called. It answers until a release before 1.0 removes it"
            );
        }
        let at = crate::services::routing::format_timestamp(now);
        let call = Call { key, name: name.to_owned(), at, logged };
        if let Some(store) = &self.store
            && store.try_send(Order::Count(call)).is_err()
        {
            tracing::warn!("A call to a deprecated API path was not counted");
        }
    }

    fn day_logged(&self, key: &Key) -> Option<String> {
        self.logged.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    /// Wait until every call counted so far is in the table: what a test reads after a request,
    /// and what a stop does before the database closes.
    pub async fn flush(&self) {
        let Some(store) = &self.store else { return };
        let (done, stored) = oneshot::channel();
        if store.send(Order::Flush(done)).await.is_ok() {
            stored.await.ok();
        }
    }
}

/// Whether a previous run logged `key` on `today`. A read, which never waits for a writer.
async fn logged_on(pool: &SqlitePool, key: &Key, today: &str) -> bool {
    let (kind, id, method, path) = key;
    sqlx::query_scalar::<_, bool>(
        "SELECT logged_on = ? FROM deprecated_calls
          WHERE caller_kind = ? AND caller_id = ? AND method = ? AND path = ?",
    )
    .bind(today)
    .bind(kind)
    .bind(id)
    .bind(method)
    .bind(path)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Store what arrives, a batch per transaction, until every sender is gone.
async fn write(pool: SqlitePool, mut orders: mpsc::Receiver<Order>) {
    while let Some(first) = orders.recv().await {
        let mut calls: HashMap<Key, (Call, i64)> = HashMap::new();
        let mut waiting = Vec::new();
        let mut next = Some(first);
        while let Some(order) = next {
            match order {
                Order::Count(call) => match calls.get_mut(&call.key) {
                    Some((kept, count)) => {
                        *count += 1;
                        kept.logged |= call.logged;
                        (kept.name, kept.at) = (call.name, call.at);
                    }
                    None => {
                        calls.insert(call.key.clone(), (call, 1));
                    }
                },
                Order::Flush(done) => waiting.push(done),
            }
            next = orders.try_recv().ok();
        }
        if let Err(e) = insert(&pool, calls.into_values()).await {
            tracing::warn!("Calls to deprecated API paths went uncounted: {e}");
        }
        for done in waiting {
            done.send(()).ok();
        }
    }
}

async fn insert(pool: &SqlitePool, calls: impl Iterator<Item = (Call, i64)>) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    for (call, count) in calls {
        let (kind, id, method, path) = &call.key;
        sqlx::query(
            "INSERT INTO deprecated_calls
                    (caller_kind, caller_id, caller_name, method, path, calls, last_called_at,
                     logged_on)
             VALUES (?, ?, ?, ?, ?, ?, ?, CASE WHEN ? THEN date(?) ELSE '' END)
             ON CONFLICT (caller_kind, caller_id, method, path) DO UPDATE
                SET calls = calls + excluded.calls, last_called_at = excluded.last_called_at,
                    caller_name = excluded.caller_name,
                    logged_on = CASE WHEN ? THEN excluded.logged_on ELSE logged_on END",
        )
        .bind(kind)
        .bind(id)
        .bind(&call.name)
        .bind(method)
        .bind(path)
        .bind(count)
        .bind(&call.at)
        .bind(call.logged)
        .bind(&call.at)
        .bind(call.logged)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
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
