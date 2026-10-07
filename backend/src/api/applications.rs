//! Application keys: what the owner hands out, and what each one reaches.

use super::Json;
use axum::extract::State;

use super::Path;
use axum::http::{Method, StatusCode};

use crate::api::auth::Identity;
use crate::error::{AppError, AppResult};
use crate::services::applications::{self, Application, Grant, Minted, NewApplication, Scope};
use crate::state::AppState;

/// What an application key may call, by method and route under `/api/v1`.
///
/// A route missing here is the owner's alone: no key reaches the settings,
/// an instance's credentials, a backup archive or another key, whatever its
/// scopes. Adding a route to the router adds nothing here, which is the
/// point: an application gains a route only by a line written for it.
pub const GRANTS: &[(&str, &str, Scope)] = &[
    ("GET", "/auth/me", Scope::Read),
    ("GET", "/status", Scope::Read),
    ("GET", "/metrics", Scope::Read),
    ("GET", "/categories", Scope::Read),
    ("GET", "/media", Scope::Read),
    ("GET", "/media/{id}", Scope::Read),
    ("GET", "/media/{id}/explain", Scope::Read),
    ("GET", "/route", Scope::Read),
    ("GET", "/decisions", Scope::Read),
    ("GET", "/overrides", Scope::Read),
    ("GET", "/jobs", Scope::Read),
    ("GET", "/jobs/{id}", Scope::Read),
    ("GET", "/logs", Scope::Read),
    ("GET", "/logs/export", Scope::Read),
    // Their address and state. Neither their key nor the webhook's address,
    // whose token lets anyone post events as the Arr (`api::instances`).
    ("GET", "/instances", Scope::Read),
    ("GET", "/instances/{id}", Scope::Read),
    ("GET", "/root-folders", Scope::Read),
    ("GET", "/root-folders/conflicts", Scope::Read),
    ("GET", "/rules", Scope::Read),
    ("GET", "/rules/{id}", Scope::Read),
    ("GET", "/rules/conditions", Scope::Read),
    ("GET", "/rules/health", Scope::Read),
    ("GET", "/rules/export", Scope::Read),
    ("GET", "/rule-tests", Scope::Read),
    ("GET", "/media/facets", Scope::Read),
    ("GET", "/metadata/providers", Scope::Read),
    // The archives' names, sizes and dates. Downloading one stays the owner's:
    // an archive holds the master key every stored credential is sealed with.
    ("GET", "/backups", Scope::Read),
    // It probes every Arr and every source, and records what it found.
    ("GET", "/health", Scope::Operate),
    ("POST", "/rule-tests/run", Scope::Operate),
    ("POST", "/backups", Scope::Operate),
    ("POST", "/instances/sync", Scope::Operate),
    ("POST", "/instances/{id}/sync", Scope::Operate),
    ("POST", "/simulate", Scope::Operate),
    ("POST", "/decisions/apply", Scope::Operate),
    ("POST", "/decisions/apply-all", Scope::Operate),
    ("POST", "/decisions/revert", Scope::Operate),
    ("POST", "/overrides", Scope::Write),
    ("DELETE", "/overrides/{id}", Scope::Write),
    ("PUT", "/overrides/external", Scope::Write),
    ("DELETE", "/overrides/external", Scope::Write),
    ("POST", "/rules", Scope::Configure),
    ("PUT", "/rules/{id}", Scope::Configure),
    ("DELETE", "/rules/{id}", Scope::Configure),
    ("POST", "/rules/{id}/duplicate", Scope::Configure),
    ("POST", "/rules/reorder", Scope::Configure),
    ("POST", "/rules/import", Scope::Configure),
    // Neither writes, both serve whoever edits a rule: what the server makes
    // of a draft, and what the draft would move.
    ("POST", "/rules/validate", Scope::Configure),
    ("POST", "/rules/preview", Scope::Configure),
    ("POST", "/rule-tests", Scope::Configure),
    ("DELETE", "/rule-tests/{id}", Scope::Configure),
    ("POST", "/categories", Scope::Configure),
    ("PUT", "/categories/{id}", Scope::Configure),
    ("DELETE", "/categories/{id}", Scope::Configure),
    ("POST", "/root-folders", Scope::Configure),
    ("DELETE", "/root-folders/{id}", Scope::Configure),
    ("PUT", "/root-folders/{id}/category", Scope::Configure),
];

/// The scope a route asks of an application key, or `None` when no key may
/// call it. HEAD asks what GET does, since it answers the same headers.
pub fn scope_for(method: &Method, route: &str) -> Option<Scope> {
    let method = if *method == Method::HEAD { &Method::GET } else { method };
    GRANTS
        .iter()
        .find(|(granted_method, granted_route, _)| {
            *granted_method == method.as_str() && *granted_route == route
        })
        .map(|(_, _, scope)| *scope)
}

/// Let an application key through to the route it asked for, or say why not.
pub fn admit(grant: &Grant, request: &axum::extract::Request) -> AppResult<()> {
    // The template, never the path sent: `/media/42` is `/media/{id}`, and a
    // grant keyed by the path would match nothing but the path it named.
    let matched = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|matched| matched.as_str())
        .unwrap_or_default();
    let route = matched.rsplit_once("/api/v1").map_or(matched, |(_, route)| route);
    let method = request.method();
    match scope_for(method, route) {
        Some(scope) if grant.allows(scope) => Ok(()),
        Some(scope) => Err(AppError::Forbidden(format!(
            "{method} {route} needs the {} scope, which this application key does not hold.",
            scope.as_str()
        ))),
        None => Err(AppError::Forbidden(format!(
            "{method} {route} is reserved to the owner: no application key may call it."
        ))),
    }
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<Application>>> {
    Ok(Json(applications::list(&state.pool).await?))
}

/// Make a key. Its token is in this answer and nowhere else, ever.
/// A key asked for, with the proof a session gives (`api::account::prove`).
#[derive(serde::Deserialize)]
pub struct Requested {
    #[serde(flatten)]
    pub application: NewApplication,
    #[serde(flatten)]
    pub proof: crate::api::account::Proof,
}

pub async fn create(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Json(requested): Json<Requested>,
) -> AppResult<Json<Minted>> {
    crate::api::account::prove(&state, &identity, &requested.proof, client).await?;
    let minted = applications::create(&state, requested.application, identity.actor()).await?;
    let made = &minted.application;
    let scopes: Vec<&str> = made.scopes.iter().map(|scope| scope.as_str()).collect();
    let detail = format!(
        "The application key {} ({}) was made, with the scopes [{}], may move files: {}",
        made.name,
        made.id,
        scopes.join(", "),
        made.may_move_files
    );
    crate::api::auth::audited(&state, &identity, client, "application_key", detail);
    Ok(Json(minted))
}

pub async fn revoke(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    applications::revoke(&state.pool, &id).await?;
    let detail = format!("The application key {id} was revoked");
    crate::api::auth::audited(&state, &identity, client, "application_key", detail);
    Ok(StatusCode::NO_CONTENT)
}
