//! Authentication, one mode at a time.
//!
//! Every mode ends in an [`Identity`] in the request's extensions, all the rest of the
//! application reads. A person has full access, or reads alone when an OIDC read list let them
//! in, and then reads no one's name but their own. An application key reaches only what its
//! scopes grant, whatever the mode (`api::applications`).

use std::net::IpAddr;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{body::Body, http::Request};
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::config::AuthMode;
use crate::error::{AppError, AppResult};
use crate::jobs::Attribution;
use crate::services::accounts::{OpenedUnder, Reach};
use crate::services::applications::{self, Grant, Resolved, Scope, TOKEN_PREFIX};
use crate::services::executor::Confirmed;
use crate::services::{accounts, audit};

/// The cookie a session travels in. Named for the application, since a browser
/// pointed at several homelab services holds all of their cookies at once.
const SESSION_COOKIE: &str = "routarr_session";
/// The same over TLS at the root of the host: a browser takes a `__Host-` cookie only over
/// TLS, from this host and for every path of it, so no other subdomain and no plain `http://`
/// page can set one or write over it (RFC 6265bis §4.1.3.2).
const HOST_SESSION_COOKIE: &str = "__Host-routarr_session";
/// Prefix of the cookie carrying one OIDC attempt. The callback takes a `code` only from the
/// browser holding it, so a link with someone else's `code` cannot sign the reader in as them
/// (login CSRF).
pub const OIDC_COOKIE: &str = "routarr_oidc";

/// The cookie one attempt travels in, named after its `state`, so two
/// attempts started in one browser, in two tabs, both finish.
fn attempt_cookie(state: &str) -> String {
    let tag: String = state.chars().take(8).collect();
    format!("{OIDC_COOKIE}_{tag}")
}

/// How many earlier attempts a browser keeps beside the one it starts: each cookie is close to
/// half a kilobyte, and a page of another site sending the browser to the start again and
/// again would otherwise make its cookies larger than the proxy in front takes.
const ATTEMPTS_KEPT: usize = 2;

/// The attempt cookies a start clears: all but the [`ATTEMPTS_KEPT`] most recent, and any
/// that is no attempt. A browser sends cookies of one path oldest first, which orders
/// attempts started within one second.
fn attempts_to_clear(state: &AppState, headers: &HeaderMap) -> Vec<String> {
    let prefix = format!("{OIDC_COOKIE}_");
    let sent = headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| name.starts_with(&prefix));
    let mut attempts: Vec<(Option<i64>, usize, String)> = sent
        .enumerate()
        .map(|(at, (name, value))| {
            let expiry = crate::services::oidc::Attempt::expiry(&state.secrets, value.trim());
            (expiry, at, name.to_string())
        })
        .collect();
    attempts.sort();
    let kept = attempts.iter().filter(|(expiry, ..)| expiry.is_some()).count().min(ATTEMPTS_KEPT);
    attempts.truncate(attempts.len() - kept);
    attempts.into_iter().map(|(.., name)| name).collect()
}

/// Who is making a request, once a mode has decided.
///
/// A person reaches everything or reads alone ([`Reach`]), an application carries its [`Grant`].
/// The subject fills the `subject` column of `jobs`, `decisions`, `execution_logs` and
/// `overrides` when anybody is named ([`Identity::actor`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub source: AuthMode,
    /// What an application key allows. `None` for a person, whose `reach` says.
    pub application: Option<Grant>,
    pub reach: Reach,
    /// When the session this request carries was opened, in Unix seconds, for
    /// a person signed in through one. `None` for a key sent in a header.
    pub signed_in_at: Option<i64>,
}

impl Identity {
    /// The caller nobody had to name: `none` lets everyone through under it.
    fn anonymous(source: AuthMode) -> Self {
        Self::person("anonymous".to_string(), source)
    }

    fn person(subject: String, source: AuthMode) -> Self {
        Self { subject, source, application: None, reach: Reach::Full, signed_in_at: None }
    }

    fn application(grant: Grant, source: AuthMode) -> Self {
        let subject = grant.name.clone();
        Self { subject, source, application: Some(grant), reach: Reach::Full, signed_in_at: None }
    }

    /// A person with full access: the one caller who reads an instance's webhook address and
    /// who makes a category the fallback.
    pub fn is_owner(&self) -> bool {
        self.application.is_none() && self.reach == Reach::Full
    }

    /// The name worth recording on a write, if anybody vouched for one.
    ///
    /// `None` under `none` and `external`, whose one shared subject names nobody and would read
    /// like an attribution on the History screen.
    pub fn actor(&self) -> Option<&str> {
        match (&self.application, self.source) {
            (Some(grant), _) => Some(&grant.name),
            (None, AuthMode::None | AuthMode::External) => None,
            (None, _) => Some(&self.subject),
        }
    }

    /// What a write this caller asked for is recorded as.
    pub fn attribution(&self) -> Attribution {
        match &self.application {
            Some(grant) => Attribution::application(&grant.name, &grant.id),
            None => Attribution::manual(self.actor()),
        }
    }

    /// The guardrail answers this caller may give, out of those it sent.
    pub fn answerable(&self, sent: &Confirmed) -> Confirmed {
        match &self.application {
            Some(grant) => sent.only(|name| grant.may_answer(name)),
            None => sent.clone(),
        }
    }

    /// A guardrail's question, marked for a person when this caller may not
    /// answer it.
    pub fn refer(&self, error: AppError) -> AppError {
        match (error, &self.application) {
            (AppError::ConfirmationRequired { kind, includes, message }, Some(grant))
                if !grant.may_answer(kind)
                    || !includes.iter().all(|name| grant.may_answer(name)) =>
            {
                AppError::ConfirmationWithheld { kind, includes, message }
            }
            (error, _) => error,
        }
    }

    /// Whether this caller holds `scope`: an application its grant's, a reader the read scope,
    /// anyone else every scope.
    pub fn holds(&self, scope: Scope) -> bool {
        match &self.application {
            Some(grant) => grant.allows(scope),
            None => self.reach == Reach::Full || scope == Scope::Read,
        }
    }

    /// Who asked, as this caller may read it. An application and a reader read their own name
    /// and no one else's, often an e-mail address in `oidc`. An application's own is what its
    /// key wrote (`key`): a revoked key's records are not the next one's, under its name.
    pub fn shown_subject(&self, subject: Option<String>, key: Option<&str>) -> Option<String> {
        match &self.application {
            Some(grant) => subject.filter(|_| key == Some(grant.id.as_str())),
            None if self.reach == Reach::Read => {
                subject.filter(|named| key.is_none() && *named == self.subject)
            }
            None => subject,
        }
    }

    /// Refuse a move of the files a key was not allowed to make.
    pub fn may_move_files(&self, move_files: bool) -> AppResult<()> {
        match &self.application {
            Some(grant) if move_files && !grant.may_move_files => Err(AppError::Forbidden(
                "This application key may not move files. Send move_files: false, or ask the \
                 owner for a key that may."
                    .into(),
            )),
            _ => Ok(()),
        }
    }
}

/// A change to a credential or a setting, or a sign-in, as the security log
/// keeps it: who asked and from where.
pub(crate) fn audited(
    state: &AppState,
    identity: &Identity,
    client: Option<IpAddr>,
    event: audit::Event<'_>,
) {
    state.audit.record(event.by(Some(&identity.subject), client));
}

/// A refusal anyone can send as fast as they like, summed per address.
pub(crate) fn refused(state: &AppState, client: Option<IpAddr>, event: audit::Event<'_>) {
    let subject = event.subject;
    state.audit.record(event.by(subject, client).summed());
}

/// An event `audited` records: a change someone was allowed to make.
pub(crate) fn allowed(kind: audit::Kind, message: &'static str) -> audit::Event<'static> {
    audit::Event::new(kind, audit::Outcome::Allowed, message)
}

/// An event `refused` records.
pub(crate) fn refusal(kind: audit::Kind, message: &'static str) -> audit::Event<'static> {
    audit::Event::new(kind, audit::Outcome::Refused, message)
}

/// Where a request came from, as the sign-in throttle reads it.
fn client_of(state: &AppState, request: &Request<Body>) -> Option<IpAddr> {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|axum::extract::ConnectInfo(address)| address.ip());
    client_address(peer, request.headers(), &state.config.trusted_proxies)
}

/// Resolve an identity for the request, or refuse it. The one place a mode is consulted.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let client = client_of(&state, &request);
    // An application key, in every mode, refused outright when it names no live key: a revoked
    // key falling through to `none` would gain reach. The master key is checked first, in case
    // it shares the prefix.
    if api_key_identity(&state, request.headers()).is_none()
        && let Some(token) =
            extract_key(request.headers()).filter(|key| key.starts_with(TOKEN_PREFIX))
    {
        let grant = match applications::resolve(&state.pool, &token).await {
            Ok(Resolved::Live(grant)) => grant,
            Ok(Resolved::Expired { id, name, expires_at }) => {
                let event = refusal(audit::Kind::ApplicationKey, "AuditApplicationKeyExpired")
                    .with("name", &name)
                    .with("id", &id);
                refused(&state, client, audit::Event { subject: Some(&name), ..event });
                return expired_application_key(&expires_at);
            }
            Ok(Resolved::Unknown) => {
                // The id names the row and is no secret: the token's secret
                // follows it.
                let id = token.strip_prefix(TOKEN_PREFIX).and_then(|rest| rest.split_once('_'));
                let id = id.map_or("", |(id, _)| id);
                let event = refusal(audit::Kind::ApplicationKey, "AuditApplicationKeyUnknown");
                refused(&state, client, event.with("id", id));
                return unknown_application_key();
            }
            Err(e) => return e.into_response(),
        };
        if let Err(wait) = state.key_rates.take(&grant.id) {
            let message = state.localizer().await.translate(
                "ErrorApplicationTooFast",
                &[("rate", &applications::PER_SECOND.to_string())],
            );
            return AppError::TooManyRequests { message, retry_after: wait.as_secs() + 1 }
                .into_response();
        }
        if let Err(refused_route) = super::applications::admit(&grant, &request) {
            let (route, scope) = super::applications::asked(&request);
            let event = match scope {
                Some(scope) => {
                    refusal(audit::Kind::Scope, "AuditScopeMissing").with("scope", scope.as_str())
                }
                None => refusal(audit::Kind::Scope, "AuditOwnerRoute"),
            };
            let event = event.with("name", &grant.name).with("id", &grant.id).with("route", route);
            refused(&state, client, audit::Event { subject: Some(&grant.name), ..event });
            return refused_route.into_response();
        }
        request.extensions_mut().insert(Identity::application(grant, state.config.auth_mode));
        return next.run(request).await;
    }

    let mut renewal = None;
    let identity = match state.config.auth_mode {
        // No credential and no proxy-sent name is read. Another site's page can post here as
        // easily, so a keyless write whose Origin is another site is refused.
        mode @ (AuthMode::None | AuthMode::External) => {
            let keyless = api_key_identity(&state, request.headers()).is_none();
            if keyless
                && !same_origin(request.method(), request.headers(), &state.config.cors_origins)
            {
                refused(&state, client, refused_origin(request.headers()));
                return foreign_origin();
            }
            // DNS rebinding: another site can make its own name resolve to this address. Under
            // `external` the proxy in front decides which names reach here.
            if keyless
                && mode == AuthMode::None
                && let Some(host) = foreign_host(&state, &request)
            {
                let event = refusal(audit::Kind::Origin, "AuditUnknownHost").with("host", &host);
                refused(&state, client, event);
                return forbidden(&format!(
                    "This Routarr runs with ROUTARR_AUTH=none and answers only to an address, \
                     localhost or a name listed in ROUTARR_ALLOWED_HOSTS. Add {host} to \
                     ROUTARR_ALLOWED_HOSTS to reach it by that name."
                ));
            }
            Some(Identity::anonymous(mode))
        }
        // The session, or the API key beside it for a machine client, as Servarr does.
        AuthMode::Forms | AuthMode::Oidc => match api_key_identity(&state, request.headers()) {
            Some(identity) => Some(identity),
            None => match session_identity(&state, request.headers()).await {
                // A cookie travels on a cross-site request. After `SameSite=Lax` and the JSON
                // extractor's content type, a foreign Origin is the third guard.
                Some(_)
                    if !same_origin(
                        request.method(),
                        request.headers(),
                        &state.config.cors_origins,
                    ) =>
                {
                    refused(&state, client, refused_origin(request.headers()));
                    return foreign_origin();
                }
                Some((identity, renewed)) => {
                    renewal = renewed;
                    Some(identity)
                }
                None => None,
            },
        },
        // The key in a header, or the session a browser exchanged it for
        // (`key_session`), which keeps the key out of the browser's storage.
        // A key-less ApiKey mode cannot happen: `main` generates one at startup
        // and `DELETE /auth/api-key` refuses in this mode.
        AuthMode::ApiKey => match api_key_identity(&state, request.headers()) {
            Some(identity) => Some(identity),
            None => match session_identity(&state, request.headers()).await {
                Some(_)
                    if !same_origin(
                        request.method(),
                        request.headers(),
                        &state.config.cors_origins,
                    ) =>
                {
                    refused(&state, client, refused_origin(request.headers()));
                    return foreign_origin();
                }
                Some((identity, renewed)) => {
                    renewal = renewed;
                    Some(identity)
                }
                None => None,
            },
        },
    };

    match identity {
        // A reader reaches what the read scope grants a key, and nothing the owner keeps.
        Some(identity)
            if identity.reach == Reach::Read
                && super::applications::asked(&request).1 != Some(Scope::Read) =>
        {
            let (route, _) = super::applications::asked(&request);
            let event = refusal(audit::Kind::Scope, "AuditReaderRefused").with("route", route);
            refused(&state, client, audit::Event { subject: Some(&identity.subject), ..event });
            let message = state.localizer().await.translate("ErrorReadOnly", &[]);
            read_only(&message)
        }
        Some(identity) => {
            let name = session_cookie_name(&state, request.headers());
            request.extensions_mut().insert(identity);
            let mut response = next.run(request).await;
            // Never over a session cookie the handler set itself, as `change_password` does.
            let sets_its_own = response
                .headers()
                .get_all(axum::http::header::SET_COOKIE)
                .iter()
                .any(|value| value.as_bytes().starts_with(format!("{name}=").as_bytes()));
            if let Some(cookie) = renewal.filter(|_| !sets_its_own) {
                response.headers_mut().append(axum::http::header::SET_COOKIE, cookie);
            }
            response
        }
        None => {
            // A request with no key at all is a browser before its sign-in,
            // not an attempt worth a line.
            if extract_key(request.headers()).is_some() {
                refused(&state, client, refusal(audit::Kind::ApiKey, "AuditApiKeyRefused"));
            }
            unauthorized_because(
                "Missing or invalid API key. Send it as X-Api-Key or Authorization: Bearer <key>.",
            )
        }
    }
}

/// What the security log says of a write another site's page sent.
fn refused_origin(headers: &HeaderMap) -> audit::Event<'static> {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    refusal(audit::Kind::Origin, "AuditForeignOrigin").with("origin", origin)
}

/// The refusal of a token shaped like an application key that names no live one.
fn unknown_application_key() -> Response {
    unauthorized_because("This application key does not exist or was revoked.")
}

/// Refused until revoked, with a `reason` a script branches on: renewing means a new key.
fn expired_application_key(expires_at: &str) -> Response {
    let message = format!(
        "This application key expired on {}. Make a new one on the Applications screen.",
        crate::timestamp::written(expires_at)
    );
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, r#"Bearer realm="Routarr""#)],
        axum::Json(serde_json::json!({
            "error": "unauthorized", "message": message, "reason": "key_expired",
        })),
    )
        .into_response()
}

/// A 401 in the envelope, naming the scheme a key is sent with: RFC 9110
/// requires the challenge on every 401, and a Bearer one opens no browser
/// dialog, where a Basic one would.
fn unauthorized_because(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, r#"Bearer realm="Routarr""#)],
        axum::Json(serde_json::json!({ "error": "unauthorized", "message": message })),
    )
        .into_response()
}

/// The name a request was sent to, when it is neither an address, `localhost`
/// nor a name the operator listed.
fn foreign_host(state: &AppState, request: &Request<Body>) -> Option<String> {
    let host = request.headers().get(axum::http::header::HOST)?.to_str().ok()?;
    let name = crate::config::host_name(host);
    let own = name.parse::<std::net::IpAddr>().is_ok()
        || name == "localhost"
        || state.config.allowed_hosts.contains(&name);
    (!own).then_some(name)
}

/// The refusal of a write whose Origin is another site.
fn foreign_origin() -> Response {
    forbidden(
        "This request did not come from Routarr: its Origin is not the host it was sent to. \
         Behind a reverse proxy, forward the public host as X-Forwarded-Host.",
    )
}

/// A valid API key, whatever the mode.
fn api_key_identity(state: &AppState, headers: &HeaderMap) -> Option<Identity> {
    let expected = state.api_key()?;
    extract_key(headers)
        .filter(|provided| constant_time_eq(provided, &expected))
        .map(|_| Identity::person("apikey".to_string(), AuthMode::ApiKey))
}

/// What a session opened now is bound to under the mode in force, `None` in a mode that opens
/// none or with no API key to open one.
pub(crate) fn opened_under(state: &AppState) -> Option<OpenedUnder> {
    match state.config.auth_mode {
        AuthMode::Forms => Some(OpenedUnder::Account),
        AuthMode::Oidc => Some(OpenedUnder::Lists(state.config.oidc_access_digest())),
        AuthMode::ApiKey => state.api_key().map(OpenedUnder::Key),
        AuthMode::None | AuthMode::External => None,
    }
}

/// The session a request carries, when the mode in force opened it under what is in force now.
async fn live_session(state: &AppState, headers: &HeaderMap) -> Option<accounts::Session> {
    let id = cookie(headers, session_cookie_name(state, headers))?;
    let (mode, under) = (state.config.auth_mode.as_str(), opened_under(state)?);
    accounts::live_session(&state.pool, &id, mode, &under).await.ok().flatten()
}

/// The identity a live session cookie names, when the mode in force opened it,
/// and the cookie to give again when answering it moved its expiry: the
/// browser keeps a cookie only as long as the `Max-Age` it was last given.
async fn session_identity(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(Identity, Option<axum::http::HeaderValue>)> {
    let id = cookie(headers, session_cookie_name(state, headers))?;
    let mode = state.config.auth_mode;
    let session = live_session(state, headers).await?;
    let renewal = session
        .renewed
        .map(|seconds| session_cookie(state, headers, &id, seconds))
        .and_then(|cookie| axum::http::HeaderValue::from_str(&cookie).ok());
    let identity = Identity {
        signed_in_at: Some(session.opened_at),
        reach: session.reach,
        ..Identity::person(session.subject, mode)
    };
    Some((identity, renewal))
}

/// One named cookie out of the header, without a crate for it.
///
/// The header is `name=value; name=value`, and a value here is hex, so nothing
/// needs unquoting or percent-decoding.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim().to_string())
}

/// Whether a mutating request states an origin, and states this one.
///
/// An absent `Origin` (a same-origin GET, a non-browser client) passes. "This one" is
/// `X-Forwarded-Host` behind a proxy, since nginx sends the upstream's own name as `Host`.
fn same_origin(method: &Method, headers: &HeaderMap, listed: &[String]) -> bool {
    if !matches!(*method, Method::POST | Method::PUT | Method::DELETE | Method::PATCH) {
        return true;
    }
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return true;
    };
    // An origin listed in `ROUTARR_CORS_ORIGINS` writes as this application's own pages do.
    if origin.to_str().is_ok_and(|origin| listed.iter().any(|allowed| allowed == origin)) {
        return true;
    }
    let forwarded = headers.get("x-forwarded-host").is_some();
    let Some(host) = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(',').next())
        .map(str::trim)
    else {
        return false;
    };
    // `X-Forwarded-Host $host` in nginx drops the port, which the proxy states
    // apart in `X-Forwarded-Port`.
    let port = headers
        .get("x-forwarded-port")
        .and_then(|p| p.to_str().ok())
        .and_then(|p| p.split(',').next())
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|_| forwarded && split_port(host).1.is_none());
    let host = match port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    origin.to_str().ok().is_some_and(|origin| origin_names(origin, &host))
}

/// Whether an `Origin` (`scheme://authority`) names `host`, port included.
///
/// The scheme's default port is dropped on both sides: a browser omits it and
/// a proxy may write it out. `null` and anything else without a scheme names
/// nobody and matches nothing.
fn origin_names(origin: &str, host: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return false,
    };
    let (origin_host, origin_port) = split_port(authority);
    let (host, port) = split_port(host);
    !origin_host.is_empty()
        && origin_host.eq_ignore_ascii_case(host)
        && origin_port.unwrap_or(default_port) == port.unwrap_or(default_port)
}

/// `host:port` in two, leaving a bracketed IPv6 literal its colons. An
/// unparseable port stays part of the host, so it matches nothing.
fn split_port(authority: &str) -> (&str, Option<u16>) {
    let end = authority.rfind(']').map_or(0, |i| i + 1);
    match authority[end..].rsplit_once(':') {
        Some((before, port)) => match port.parse::<u16>() {
            Ok(port) => (&authority[..end + before.len()], Some(port)),
            Err(_) => (authority, None),
        },
        None => (authority, None),
    }
}

/// Refused to a reader, with the `reason` the interface keys on.
fn read_only(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "error": "forbidden", "message": message, "reason": "read_only",
        })),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "forbidden", "message": message })),
    )
        .into_response()
}

fn extract_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(value.trim().to_string());
    }
    // The scheme is a token compared without regard to case (RFC 9110 §11.1).
    let (scheme, token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?
        .trim()
        .split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
}

/// Compare without leaking the position of the first differing byte.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.ct_eq(b).into()
}

/// What the shell needs before it can render anything, including a refusal.
///
/// Public: a browser not signed in yet learns the mode and whether a key exists, which
/// shortens no 256-bit key.
pub async fn mode(State(state): State<AppState>) -> super::Json<serde_json::Value> {
    super::Json(serde_json::json!({
        "mode": state.config.auth_mode.as_str(),
        "api_key_configured": state.api_key().is_some(),
        // A key from the environment cannot be rotated: the variable replaces it at restart.
        "api_key_pinned": state.config.api_key.is_some(),
    }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// One answer for a wrong name, length or password: naming which tells an attacker the
/// others were right.
fn unauthorized() -> Response {
    unauthorized_because("Wrong username or password.")
}

/// Where a request comes from: the peer, or the client a trusted proxy
/// forwarded. `None` on a connection that carries no peer address, as a
/// test's does.
pub struct Client(pub Option<IpAddr>);

impl axum::extract::FromRequestParts<AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|axum::extract::ConnectInfo(address)| address.ip());
        Ok(Self(client_address(peer, &parts.headers, &state.config.trusted_proxies)))
    }
}

/// The client behind `peer`: `X-Forwarded-For` read from its last entry while a listed proxy
/// wrote it. Trusting anyone else's header lets them pick whose share of the sign-in queue
/// they fill.
fn client_address(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &[crate::config::Network],
) -> Option<IpAddr> {
    let listed = |ip: IpAddr| trusted.iter().any(|network| network.contains(ip));
    let mut client = peer?.to_canonical();
    let hops: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .collect();
    for hop in hops.into_iter().rev() {
        if !listed(client) {
            break;
        }
        // An entry that is not an address ends the chain at the proxy that
        // wrote it.
        let Ok(forwarded) = hop.trim().parse::<IpAddr>() else { break };
        client = forwarded.to_canonical();
    }
    Some(client)
}

/// Exchange a username and a password for a session cookie.
pub async fn login(
    State(state): State<AppState>,
    Client(client): Client,
    headers: HeaderMap,
    super::Json(credentials): super::Json<Credentials>,
) -> Response {
    // Before anything is read or hashed: a guess sent while waiting is checked
    // against nothing, the right password included.
    if let Some(left) = state.sign_in.held_back(client) {
        let seconds = left.as_secs() + 1;
        refused(&state, client, refusal(audit::Kind::SignIn, "AuditSignInHeldBack"));
        let message = state
            .localizer()
            .await
            .translate("ErrorSignInHeldBack", &[("seconds", &seconds.to_string())]);
        return AppError::TooManyRequests { message, retry_after: seconds }.into_response();
    }

    // Every refusal leaves one line naming where it came from, the line a
    // fail2ban filter reads, and never what was typed: a password typed into
    // the name field is a password.
    let refuse = || {
        // Every one counts toward the wait and toward a fail2ban jail, so
        // none is summed.
        state.audit.record(refusal(audit::Kind::SignIn, "AuditSignInRefused").by(None, client));
        state.sign_in.failed(client);
        unauthorized()
    };

    let stored = accounts::account(&state.pool).await.ok().flatten();
    let Some((username, hash)) = stored else {
        return refuse();
    };

    // Shorter than `change_password` accepts, so not the stored password: no argon2 check
    // needed. The minimum is public, the refusal above states it.
    if credentials.password.chars().count() < MIN_PASSWORD_LENGTH {
        return refuse();
    }
    // The hash runs whatever the name, so the timing does not tell which name exists.
    let name_matches = username == credentials.username.trim();

    // Bounded and off the runtime (`SignInThrottle`): argon2id makes this endpoint costly to serve.
    let matched = match state.sign_in.verify(&credentials.password, &hash, client).await {
        Ok(matched) => matched,
        Err(accounts::Busy) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::RETRY_AFTER, "1")],
                axum::Json(serde_json::json!({
                    "error": "busy",
                    "message": "Too many sign-ins are being checked at once. Try again in a moment.",
                })),
            )
                .into_response();
        }
    };

    if !(matched && name_matches) {
        return refuse();
    }
    state.sign_in.succeeded(client);

    match accounts::open_session(
        &state.pool,
        credentials.username.trim(),
        AuthMode::Forms.as_str(),
        Reach::Full,
        &OpenedUnder::Account,
    )
    .await
    {
        Ok(id) => {
            let name = credentials.username.trim();
            let person = Identity::person(name.to_string(), AuthMode::Forms);
            audited(&state, &person, client, allowed(audit::Kind::SignIn, "AuditSignedIn"));
            (
                StatusCode::OK,
                [(
                    axum::http::header::SET_COOKIE,
                    session_cookie(
                        &state,
                        &headers,
                        &id,
                        accounts::opening_seconds(AuthMode::Forms.as_str()),
                    ),
                )],
                axum::Json(serde_json::json!({ "username": credentials.username.trim() })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySession {
    pub key: String,
}

/// Exchange the API key for a session cookie, in `apikey` mode.
///
/// What a browser keeps instead of the key: `HttpOnly`, so no script on the
/// page can read it, ending with its lifetime or a sign-out. A wrong key counts
/// toward the wait of the address it came from, as a wrong password does.
pub async fn key_session(
    State(state): State<AppState>,
    Client(client): Client,
    headers: HeaderMap,
    super::Json(sent): super::Json<KeySession>,
) -> Response {
    if let Some(left) = state.sign_in.held_back(client) {
        let seconds = left.as_secs() + 1;
        refused(&state, client, refusal(audit::Kind::SignIn, "AuditSignInHeldBack"));
        let message = state
            .localizer()
            .await
            .translate("ErrorSignInHeldBack", &[("seconds", &seconds.to_string())]);
        return AppError::TooManyRequests { message, retry_after: seconds }.into_response();
    }
    let Some(key) = state.api_key().filter(|expected| constant_time_eq(sent.key.trim(), expected))
    else {
        let event = refusal(audit::Kind::SignIn, "AuditKeySessionRefused");
        state.audit.record(event.by(None, client));
        state.sign_in.failed(client);
        return unauthorized_because("This key opens nothing.");
    };
    state.sign_in.succeeded(client);

    let source = AuthMode::ApiKey.as_str();
    let under = OpenedUnder::Key(key);
    match accounts::open_session(&state.pool, "apikey", source, Reach::Full, &under).await {
        Ok(id) => {
            let person = Identity::person("apikey".to_string(), AuthMode::ApiKey);
            let event = allowed(audit::Kind::SignIn, "AuditKeySessionOpened");
            audited(&state, &person, client, event);
            (
                StatusCode::OK,
                [(
                    axum::http::header::SET_COOKIE,
                    session_cookie(&state, &headers, &id, accounts::opening_seconds(source)),
                )],
                axum::Json(serde_json::json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// End the session this request carries, and clear the cookie either way.
pub async fn logout(
    State(state): State<AppState>,
    Client(client): Client,
    headers: HeaderMap,
) -> Response {
    // The route is public, and a page of another site could end a session the
    // browser carries for it.
    if !same_origin(&Method::POST, &headers, &state.config.cors_origins) {
        refused(&state, client, refused_origin(&headers));
        return foreign_origin();
    }
    if let Some(id) = cookie(&headers, session_cookie_name(&state, &headers)) {
        if let Some(session) = live_session(&state, &headers).await {
            let person = Identity::person(session.subject, state.config.auth_mode);
            audited(&state, &person, client, allowed(audit::Kind::SignOut, "AuditSignedOut"));
        }
        let _ = accounts::close_session(&state.pool, &id).await;
    }
    (
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, session_cookie(&state, &headers, "", 0))],
        axum::Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

/// Who a request is.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Me {
    /// An application's name, the signed-in person, `apikey` for the owner's
    /// key, or `anonymous` where the mode asks for nothing.
    pub subject: String,
    /// When the application key this request carries stops being accepted. Null for a key that
    /// never expires, and for anyone else.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub expires_at: Option<String>,
    /// A person signed in to read alone, whose every other call answers 403 `read_only`.
    /// False for everyone else, an application key included, which its scopes hold.
    pub read_only: bool,
}

/// Who this request is, for a shell that shows a name and hides what a reader cannot do.
pub async fn me(axum::Extension(identity): axum::Extension<Identity>) -> super::Json<Me> {
    let expires_at = identity.application.as_ref().and_then(|grant| grant.expires_at.clone());
    let read_only = identity.reach == Reach::Read;
    super::Json(Me { subject: identity.subject, expires_at, read_only })
}

#[derive(serde::Deserialize)]
pub struct StartQuery {
    /// `0` to have the provider sign the person in again (`api::account::prove`).
    #[serde(default)]
    pub max_age: Option<i64>,
    /// The screen to come back to, a path under the mount point.
    #[serde(default, rename = "return")]
    pub return_to: Option<String>,
}

/// Send the browser to the provider. A failure redirects too, back to the sign-in screen,
/// its reason logged and never shown to an anonymous caller.
pub async fn oidc_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    super::Query(query): super::Query<StartQuery>,
) -> Response {
    use crate::services::oidc::{FLOW_MINUTES, screen};
    let return_to = query.return_to.as_deref().and_then(screen);
    let started = crate::services::oidc::start(&state, query.max_age.is_some(), return_to)
        .await
        .and_then(|start| Ok((start.attempt.sealed(&state.secrets)?, start)));
    match started {
        Ok((sealed, start)) => {
            let name = attempt_cookie(&start.attempt.state);
            let opened = cookie_header(&state, &headers, &name, &sealed, FLOW_MINUTES * 60);
            let cleared = attempts_to_clear(&state, &headers)
                .into_iter()
                .map(|old| cookie_header(&state, &headers, &old, "", 0));
            let cookies = std::iter::once(opened)
                .chain(cleared)
                .map(|cookie| (axum::http::header::SET_COOKIE, cookie))
                .collect::<Vec<_>>();
            (
                axum::response::AppendHeaders(cookies),
                axum::response::Redirect::to(&start.redirect_to),
            )
                .into_response()
        }
        // Logged where the provider is read, once while its failure is kept.
        Err(e) => {
            tracing::debug!("An OpenID Connect sign-in could not start: {e}");
            axum::response::Redirect::to(&format!("{}?signin=failed", home(&state))).into_response()
        }
    }
}

/// Where a browser comes back to the application.
fn home(state: &AppState) -> String {
    if state.config.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", state.config.base_path)
    }
}

#[derive(serde::Deserialize)]
pub struct Callback {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// What the provider says when the person refused, or when it did.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Where the provider sends the browser back. Every outcome is a redirect, a failure with
/// the marker the shell shows.
pub async fn oidc_callback(
    State(state): State<AppState>,
    Client(client): Client,
    headers: HeaderMap,
    super::Query(callback): super::Query<Callback>,
) -> Response {
    use crate::services::oidc::Attempt;
    let home = home(&state);

    // The attempt is over either way, so the cookie that carried it goes.
    let cookie_name = callback.state.as_deref().map(attempt_cookie);
    let cleared = cookie_name.as_deref().map(|name| cookie_header(&state, &headers, name, "", 0));
    let failed = || {
        let redirect = axum::response::Redirect::to(&format!("{home}?signin=failed"));
        match &cleared {
            Some(cleared) => {
                ([(axum::http::header::SET_COOKIE, cleared.clone())], redirect).into_response()
            }
            None => redirect.into_response(),
        }
    };

    // A refusal and an unknown client look alike on screen, so the reason goes to the log,
    // summed per address since anyone can send one. The code never does.
    if let Some(error) = callback.error.as_deref() {
        // RFC 6749 §4.1.2.1 codes are short words: anything longer is not the provider's.
        let error: String = error.chars().take(64).collect();
        let described: String =
            callback.error_description.as_deref().unwrap_or_default().chars().take(200).collect();
        let reason = format!("{error} {described}");
        let event = refusal(audit::Kind::SignIn, "AuditOidcProviderRefused");
        refused(&state, client, event.with("reason", reason.trim_end()));
        return failed();
    }
    let (Some(code), Some(flow_state), Some(cookie_name)) =
        (callback.code, callback.state, cookie_name)
    else {
        return failed();
    };
    // Before the exchange each attempt sets off at the provider's token endpoint.
    if state.sign_in.held_back(client).is_some() {
        refused(&state, client, refusal(audit::Kind::SignIn, "AuditSignInHeldBack"));
        return failed();
    }

    // Only the browser that started the attempt may finish it.
    let Some(attempt) = cookie(&headers, &cookie_name)
        .and_then(|sealed| Attempt::opened(&state.secrets, &sealed, &flow_state))
    else {
        refused(&state, client, refusal(audit::Kind::SignIn, "AuditOidcAttemptUnknown"));
        return failed();
    };

    // The screen that sent the person to sign in again, under the mount point.
    let landing = match attempt.return_to.as_deref() {
        Some(screen) => format!("{home}{}", screen.trim_start_matches('/')),
        None => home.clone(),
    };
    let (subject, reach) = match crate::services::oidc::finish(&state, &code, attempt).await {
        Ok(signed_in) => signed_in,
        Err(e) => {
            let event = refusal(audit::Kind::SignIn, "AuditOidcRefused").with("reason", e);
            state.audit.record(event.by(None, client));
            state.sign_in.failed(client);
            return failed();
        }
    };

    let under = OpenedUnder::Lists(state.config.oidc_access_digest());
    match accounts::open_session(&state.pool, &subject, AuthMode::Oidc.as_str(), reach, &under)
        .await
    {
        // Appended: an array of headers inserts each one, so the second cookie
        // would replace the session the browser is being handed.
        Ok(id) => {
            state.sign_in.succeeded(client);
            let person = Identity { reach, ..Identity::person(subject.clone(), AuthMode::Oidc) };
            let signed_in = match reach {
                Reach::Full => "AuditOidcSignedIn",
                Reach::Read => "AuditOidcSignedInToRead",
            };
            audited(&state, &person, client, allowed(audit::Kind::SignIn, signed_in));
            (
                axum::response::AppendHeaders([
                    (
                        axum::http::header::SET_COOKIE,
                        session_cookie(
                            &state,
                            &headers,
                            &id,
                            accounts::opening_seconds(AuthMode::Oidc.as_str()),
                        ),
                    ),
                    (axum::http::header::SET_COOKIE, cleared.unwrap_or_default()),
                ]),
                axum::response::Redirect::to(&landing),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("An OpenID Connect sign-in could not open its session: {e}");
            failed()
        }
    }
}

/// The shortest password accepted, the minimum OWASP ASVS 2.1.1 sets.
pub(crate) const MIN_PASSWORD_LENGTH: usize = 12;

/// The `Set-Cookie` value, scoped to the mount point.
///
/// `Path` follows `base_path`, so an application a proxy serves under another path does not
/// receive it. A service on another port of the same host name does (RFC 6265 §8.5).
/// `Secure` follows the scheme the browser used, so a cookie issued over TLS never travels
/// over `http://`.
pub(crate) fn session_cookie(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    max_age: i64,
) -> String {
    cookie_header(state, headers, session_cookie_name(state, headers), id, max_age)
}

/// The name the session cookie of this request goes by, set and read under it alone.
pub(crate) fn session_cookie_name(state: &AppState, headers: &HeaderMap) -> &'static str {
    if state.config.base_path.is_empty() && came_over_tls(state, headers) {
        HOST_SESSION_COOKIE
    } else {
        SESSION_COOKIE
    }
}

/// Every cookie this application sets, with the same scope and the same
/// guards. A `max_age` of 0 clears it.
fn cookie_header(
    state: &AppState,
    headers: &HeaderMap,
    name: &str,
    value: &str,
    max_age: i64,
) -> String {
    let path = home(state);
    let secure = if came_over_tls(state, headers) { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// Whether the browser reached this request over TLS: `X-Forwarded-Proto`, or an OIDC
/// redirect registered as `https://` for a proxy that forwards nothing.
fn came_over_tls(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .is_some_and(|scheme| scheme.trim().eq_ignore_ascii_case("https"))
        || state.config.oidc_redirect_url.as_deref().is_some_and(|url| url.starts_with("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn only_a_mode_that_names_somebody_produces_an_actor() {
        let named = Identity::person("alice".to_string(), AuthMode::Forms);
        assert_eq!(named.actor(), Some("alice"));

        for source in [AuthMode::None, AuthMode::External] {
            assert_eq!(Identity::anonymous(source).actor(), None);
        }
    }

    #[test]
    fn every_writing_method_is_held_to_the_origin_and_no_read_is() {
        let from = |origin: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(axum::http::header::ORIGIN, HeaderValue::from_static(origin));
            headers.insert(axum::http::header::HOST, HeaderValue::from_static("routarr.local"));
            headers
        };
        let (foreign, own) = (from("https://evil.example"), from("http://routarr.local"));
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            assert!(
                !same_origin(&method, &foreign, &[]),
                "{method} from another site went through"
            );
            assert!(same_origin(&method, &own, &[]), "{method} from this application was refused");
        }
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(same_origin(&method, &foreign, &[]), "{method} was held to the origin");
        }
    }

    #[test]
    fn a_forwarded_port_completes_a_forwarded_host_without_one() {
        let write = |port: Option<&str>| {
            let mut request = Request::post("/api/v1/simulate")
                .header("origin", "https://nas.lan:8443")
                .header("host", "routarr:9876")
                .header("x-forwarded-host", "nas.lan");
            if let Some(port) = port {
                request = request.header("x-forwarded-port", port);
            }
            request.body(Body::empty()).unwrap()
        };
        let same = |request: Request<Body>| same_origin(request.method(), request.headers(), &[]);
        assert!(same(write(Some("8443"))));
        assert!(!same(write(Some("443"))));
        assert!(!same(write(None)));
    }

    /// A range holds a proxy recreated with a new address, and behind two listed proxies the
    /// client is the address the first of them saw.
    #[test]
    fn only_a_trusted_proxy_names_the_client_it_forwards() {
        let network = |entry: &str| crate::config::Network::parse(entry).unwrap();
        let forwarded = |chain: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", HeaderValue::from_static(chain));
            headers
        };
        let headers = forwarded("192.0.2.1, 203.0.113.9");
        let proxy: Option<IpAddr> = "172.18.0.2".parse().ok();
        let neighbour: Option<IpAddr> = "192.168.1.40".parse().ok();
        let stranger: Option<IpAddr> = "198.51.100.7".parse().ok();
        let trusted = [network("172.18.0.2")];

        assert_eq!(client_address(proxy, &headers, &trusted), "203.0.113.9".parse().ok());
        assert_eq!(client_address(neighbour, &headers, &trusted), neighbour);
        assert_eq!(client_address(stranger, &headers, &trusted), stranger);
        assert_eq!(client_address(proxy, &headers, &[]), proxy);
        assert_eq!(client_address(proxy, &HeaderMap::new(), &trusted), proxy);
        assert_eq!(client_address(None, &headers, &trusted), None);

        let recreated: Option<IpAddr> = "172.18.0.9".parse().ok();
        let range = [network("172.18.0.0/16")];
        assert_eq!(client_address(recreated, &headers, &range), "203.0.113.9".parse().ok());

        let two = [network("172.18.0.0/16"), network("10.0.0.0/8")];
        let chain = forwarded("198.51.100.66, 203.0.113.9, 10.1.2.3");
        assert_eq!(client_address(recreated, &chain, &two), "203.0.113.9".parse().ok());
        let ended = forwarded("203.0.113.9, unknown, 10.1.2.3");
        assert_eq!(client_address(recreated, &ended, &two), "10.1.2.3".parse().ok());
    }

    #[test]
    fn comparison_rejects_length_mismatch() {
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("abc", "abc"));
    }
}
