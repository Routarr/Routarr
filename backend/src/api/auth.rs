//! Authentication, one mode at a time.
//!
//! Routarr sits in a homelab, often behind a reverse proxy, but its API can
//! move files on disk, so a protected call carries a credential unless the
//! operator asked for none. Every mode ends at the same place: an [`Identity`]
//! in the request's extensions, which is what the rest of the application
//! reads. Nothing downstream knows which mode produced it, and that is the
//! point: adding a mode is one arm here, not a change everywhere.
//!
//! There are no roles among people. The Servarr applications have none either:
//! their `User` model carries a name and a password hash and nothing else, and
//! a single account. A person's access is all or nothing, and who gets in is
//! the mode's business. An application key is the one exception: it reaches
//! only what its scopes grant, whatever the mode (`api::applications`).

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
use crate::services::accounts;
use crate::services::applications::{self, Grant, TOKEN_PREFIX};
use crate::services::executor::Confirmed;

/// The cookie the `forms` mode sets. Named for the application, since a browser
/// pointed at several homelab services holds all of their cookies at once.
pub const SESSION_COOKIE: &str = "routarr_session";
/// Carries an OIDC attempt from the browser that left to the browser that
/// comes back, one cookie per attempt under this prefix. The callback accepts
/// a `code` for the attempt it names only from that browser: without it, a
/// link carrying someone else's `code` and `state` would sign the reader in as
/// that someone (login CSRF).
pub const OIDC_COOKIE: &str = "routarr_oidc";

/// The cookie one attempt travels in, named after its `state`, so two
/// attempts started in one browser, in two tabs, both finish.
fn attempt_cookie(state: &str) -> String {
    let tag: String = state.chars().take(8).collect();
    format!("{OIDC_COOKIE}_{tag}")
}

/// Who is making a request, once a mode has decided.
///
/// A person carries no role: there is one level of access, and the mode
/// decides who reaches it. An application carries its [`Grant`]. The subject
/// is what the `subject` column of `jobs`, `decisions`, `execution_logs` and
/// `overrides` records, when anybody is named (see [`Identity::actor`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub source: AuthMode,
    /// What an application key allows. `None` for a person, who may do
    /// everything.
    pub application: Option<Grant>,
}

impl Identity {
    /// The caller nobody had to name: `none` lets everyone through under it.
    fn anonymous(source: AuthMode) -> Self {
        Self { subject: "anonymous".to_string(), source, application: None }
    }

    fn person(subject: String, source: AuthMode) -> Self {
        Self { subject, source, application: None }
    }

    fn application(grant: Grant, source: AuthMode) -> Self {
        Self { subject: grant.name.clone(), source, application: Some(grant) }
    }

    /// The name worth recording on a write, if anybody vouched for one.
    ///
    /// `none` and `external` let everybody through under one shared subject, so
    /// storing it would fill an audit column with a word that names nobody,
    /// and that reads, on the History screen, exactly like an attribution. An
    /// application key names its application in every mode.
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

    /// Whether this caller holds `scope`: an application its grant's, anyone
    /// else every scope.
    pub fn holds(&self, scope: crate::services::applications::Scope) -> bool {
        self.application.as_ref().is_none_or(|grant| grant.allows(scope))
    }

    /// Who asked, as this caller may read it. An application reads its own
    /// name and no one else's: `subject` holds a person's user name in `forms`
    /// and often an e-mail address in `oidc`, and a key handed to another
    /// application must not learn who runs Routarr. Its own is what its key
    /// wrote (`key`), never what carries its name: a revoked key's records
    /// are not the next one's, under the same name or a person's.
    pub fn shown_subject(&self, subject: Option<String>, key: Option<&str>) -> Option<String> {
        match &self.application {
            Some(grant) => subject.filter(|_| key == Some(grant.id.as_str())),
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

/// Resolve an identity for the request, or refuse it.
///
/// The one place a mode is consulted. A handler that needs to know who called
/// reads `Extension<Identity>`, and nothing needs to ask how they proved it.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // An application key, in every mode: held to its scopes, and refused
    // outright when it names no live key. Letting a revoked key fall through to
    // a mode that asks nothing would hand it more than it ever had. The master
    // key is looked at first, so one that happens to share the prefix is still
    // the master key.
    if api_key_identity(&state, request.headers()).is_none()
        && let Some(token) =
            extract_key(request.headers()).filter(|key| key.starts_with(TOKEN_PREFIX))
    {
        let grant = match applications::resolve(&state.pool, &token).await {
            Ok(Some(grant)) => grant,
            Ok(None) => return unknown_application_key(),
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
        if let Err(refusal) = super::applications::admit(&grant, &request) {
            return refusal.into_response();
        }
        request.extensions_mut().insert(Identity::application(grant, state.config.auth_mode));
        return next.run(request).await;
    }

    let mut renewal = None;
    let identity = match state.config.auth_mode {
        // Nothing is asked for, and no name a proxy sends is read: the proxy in
        // front has already decided, and reading one would only invent a trust
        // boundary where none is enforceable. A page of another site can then
        // post here as easily as this application, so a write that carries no
        // API key is refused when its Origin says another site asked.
        mode @ (AuthMode::None | AuthMode::External) => {
            let keyless = api_key_identity(&state, request.headers()).is_none();
            if keyless && !same_origin(&request, &state.config.cors_origins) {
                return foreign_origin();
            }
            // A page of another site can make its own name resolve to this
            // address, and the browser then calls it same-origin. Under
            // `external` the proxy in front decides which names reach here.
            if keyless
                && mode == AuthMode::None
                && let Some(host) = foreign_host(&state, &request)
            {
                return forbidden(&format!(
                    "This Routarr runs with ROUTARR_AUTH=none and answers only to an address, \
                     localhost or a name listed in ROUTARR_ALLOWED_HOSTS. Add {host} to \
                     ROUTARR_ALLOWED_HOSTS to reach it by that name."
                ));
            }
            Some(Identity::anonymous(mode))
        }
        // The session, or the API key beside it. A machine client cannot hold
        // a cookie, and Servarr keeps the two the same way. OIDC resolves the
        // same way once the provider has answered: the session is the session.
        AuthMode::Forms | AuthMode::Oidc => match api_key_identity(&state, request.headers()) {
            Some(identity) => Some(identity),
            None => match session_identity(&state, request.headers()).await {
                // A cookie travels on a cross-site request whether or not the
                // page meant to send it. `SameSite=Lax` already refuses the
                // dangerous shapes and the JSON extractor refuses a form's
                // content type, so this is the third of three: an Origin that
                // is present and foreign is not this application asking.
                Some(_) if !same_origin(&request, &state.config.cors_origins) => {
                    return foreign_origin();
                }
                Some((identity, renewed)) => {
                    renewal = renewed;
                    Some(identity)
                }
                None => None,
            },
        },
        // A key-less ApiKey mode cannot happen: `main` generates one at startup
        // and `DELETE /auth/api-key` refuses in this mode. Refusing rather than
        // passing through keeps the failure loud if that ever stops being true.
        AuthMode::ApiKey => api_key_identity(&state, request.headers()),
    };

    match identity {
        Some(identity) => {
            request.extensions_mut().insert(identity);
            let mut response = next.run(request).await;
            // Not over a cookie the handler set itself, as a password change
            // clears the session it ends.
            let sets_its_own = response
                .headers()
                .get_all(axum::http::header::SET_COOKIE)
                .iter()
                .any(|value| value.as_bytes().starts_with(SESSION_COOKIE.as_bytes()));
            if let Some(cookie) = renewal.filter(|_| !sets_its_own) {
                response.headers_mut().append(axum::http::header::SET_COOKIE, cookie);
            }
            response
        }
        None => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": "unauthorized",
                "message": "Missing or invalid API key. Send it as X-Api-Key or Authorization: Bearer <key>."
            })),
        )
            .into_response(),
    }
}

/// The refusal of a token shaped like an application key that names no live one.
fn unknown_application_key() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "error": "unauthorized",
            "message": "This application key does not exist or was revoked.",
        })),
    )
        .into_response()
}

/// The refusal of a write whose Origin is another site.
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

/// The identity a live session cookie names, when the mode in force opened it,
/// and the cookie to give again when answering it moved its expiry: the
/// browser keeps a cookie only as long as the `Max-Age` it was last given.
async fn session_identity(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(Identity, Option<axum::http::HeaderValue>)> {
    let id = cookie(headers, SESSION_COOKIE)?;
    let mode = state.config.auth_mode;
    let session = accounts::live_session(&state.pool, &id, mode.as_str()).await.ok().flatten()?;
    let renewal = session
        .renewed
        .then(|| session_cookie(state, headers, &id, accounts::SESSION_DAYS))
        .and_then(|cookie| axum::http::HeaderValue::from_str(&cookie).ok());
    Some((Identity::person(session.subject, mode), renewal))
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
/// A read cannot forge anything, and an absent `Origin` is what a same-origin
/// GET and every non-browser client look like. What this refuses is a browser
/// saying plainly that another site asked.
///
/// "This one" is the host the browser addressed, which behind a reverse proxy
/// is `X-Forwarded-Host` rather than `Host`: nginx sends the upstream's own
/// name as `Host` unless told otherwise, and compared against that, every
/// write of every session behind it would be refused.
fn same_origin(request: &Request<Body>, listed: &[String]) -> bool {
    if !matches!(*request.method(), Method::POST | Method::PUT | Method::DELETE | Method::PATCH) {
        return true;
    }
    let headers = request.headers();
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return true;
    };
    // An origin the operator listed in `ROUTARR_CORS_ORIGINS` calls as this
    // application's own pages do: that is what listing it means.
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
/// Public, and deliberately thin: the mode decides which gate to show, and a
/// browser that has not signed in yet must be able to learn that much.
///
/// It also says whether a key exists at all. In `forms` and `oidc` one is only
/// present when the operator set `ROUTARR_API_KEY` or generated one, so without
/// this the interface would offer a field that cannot work. Saying so tells an
/// unauthenticated caller nothing the mode had not already told them, and
/// nothing that shortens a 256-bit key.
pub async fn mode(State(state): State<AppState>) -> super::Json<serde_json::Value> {
    super::Json(serde_json::json!({
        "mode": state.config.auth_mode.as_str(),
        "api_key_configured": state.api_key().is_some(),
        // A key the environment supplies cannot be rotated: the new one would
        // live until the next restart and then be replaced by the variable
        // again. The interface says so rather than offering a button that
        // quietly expires.
        "api_key_pinned": state.config.api_key.is_some(),
    }))
}

/// Mint a new API key, replacing whatever was there.
///
/// The value is returned exactly once. Storing it to show again later would
/// make every subsequent read of this screen a second chance to copy it, which
/// is the property a credential should not have.
pub async fn rotate_api_key(
    State(state): State<AppState>,
) -> AppResult<super::Json<serde_json::Value>> {
    refuse_if_pinned(&state)?;
    let key = state.rotate_api_key()?;
    Ok(super::Json(serde_json::json!({ "api_key": key })))
}

/// Withdraw the key, leaving the session as the only way in.
pub async fn delete_api_key(State(state): State<AppState>) -> AppResult<StatusCode> {
    refuse_if_pinned(&state)?;
    // In `apikey` mode it is the only credential there is, and the middleware
    // refuses every request once it is gone, including the one that would put
    // it back.
    if state.config.auth_mode == AuthMode::ApiKey {
        return Err(AppError::Conflict(
            "The API key is the only way in while ROUTARR_AUTH=apikey. Switch to a session mode \
             before removing it."
                .into(),
        ));
    }
    state.clear_api_key()?;
    Ok(StatusCode::NO_CONTENT)
}

fn refuse_if_pinned(state: &AppState) -> AppResult<()> {
    if state.config.api_key.is_some() {
        return Err(AppError::Conflict(
            "ROUTARR_API_KEY sets this key, so it cannot be changed here. Change the variable \
             and restart."
                .into(),
        ));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// One answer for a wrong name, a wrong length and a wrong password alike.
///
/// Saying which was wrong tells an attacker that the others were right.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(
            serde_json::json!({ "error": "unauthorized", "message": "Wrong username or password." }),
        ),
    )
        .into_response()
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

/// The client behind `peer`. Each proxy appends the address it saw to
/// `X-Forwarded-For`, so the entries are read from the last while the one
/// that wrote each is a proxy the operator listed: the first address no
/// listed proxy stands behind is the client. Anyone else writing the header
/// would choose whose share of the sign-in queue they fill.
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
///
/// One message for a wrong name and a wrong password alike: saying which was
/// wrong tells an attacker that the other was right.
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
        match client {
            Some(address) => tracing::warn!("A sign-in was refused for {address}"),
            None => tracing::warn!("A sign-in was refused for an unknown address"),
        }
        state.sign_in.failed(client);
        unauthorized()
    };

    let stored = accounts::account(&state.pool).await.ok().flatten();
    let Some((username, hash)) = stored else {
        return refuse();
    };

    // One cheap refusal before the expensive one. A password shorter than the
    // minimum cannot be the stored one, since `change_password` refuses to set
    // one, so hashing it would spend a whole argon2 check proving what its
    // length already says. The minimum is public: the refusal above states it.
    if credentials.password.chars().count() < MIN_PASSWORD_LENGTH {
        return refuse();
    }
    // The username is *not* checked first: answering at once on a wrong name
    // and only after a hash on the right one tells a caller which name
    // exists. The hash is verified either way and the name is required to
    // match too.
    let name_matches = username == credentials.username.trim();

    // Bounded, and off the runtime. See `SignInThrottle`: argon2id is what
    // makes this endpoint expensive to serve as well as hard to guess, and it
    // is the machine that needs defending rather than the password.
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

    match accounts::open_session(&state.pool, credentials.username.trim(), AuthMode::Forms.as_str())
        .await
    {
        Ok(id) => (
            StatusCode::OK,
            [(
                axum::http::header::SET_COOKIE,
                session_cookie(&state, &headers, &id, accounts::SESSION_DAYS),
            )],
            axum::Json(serde_json::json!({ "username": credentials.username.trim() })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// End the session this request carries, and clear the cookie either way.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = cookie(&headers, SESSION_COOKIE) {
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
}

/// Who this request is, for a shell that wants to show a name.
pub async fn me(axum::Extension(identity): axum::Extension<Identity>) -> super::Json<Me> {
    super::Json(Me { subject: identity.subject })
}

/// Send the browser to the provider.
///
/// A redirect rather than a JSON URL the shell would follow: the browser has to
/// leave, and the one thing that cannot go wrong here is a fetch that stays.
/// A failure is a redirect too, back to the sign-in screen with the marker the
/// shell shows, since the caller is a browser following a link. The reason goes
/// to the log, where a configuration fault is fixed, and never to an anonymous
/// caller.
pub async fn oidc_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use crate::services::oidc::FLOW_MINUTES;
    let started = crate::services::oidc::start(&state)
        .await
        .and_then(|start| Ok((start.attempt.sealed(&state.secrets)?, start)));
    match started {
        Ok((sealed, start)) => (
            [(
                axum::http::header::SET_COOKIE,
                cookie_header(
                    &state,
                    &headers,
                    &attempt_cookie(&start.attempt.state),
                    &sealed,
                    FLOW_MINUTES * 60,
                ),
            )],
            axum::response::Redirect::to(&start.redirect_to),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("An OpenID Connect sign-in could not start: {e}");
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

/// Where the provider sends the browser back.
///
/// It answers with a redirect in every case, because the caller is a browser
/// following a link and not a client reading JSON. A failure lands on the
/// application with a marker the shell can show.
pub async fn oidc_callback(
    State(state): State<AppState>,
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

    // A person who refused and a client the provider does not know look alike
    // on screen, so the reason goes to the log. The code never does.
    if let Some(error) = callback.error.as_deref() {
        let described: String =
            callback.error_description.as_deref().unwrap_or_default().chars().take(200).collect();
        tracing::warn!("The OpenID Connect provider refused the sign-in: {error} {described}");
        return failed();
    }
    let (Some(code), Some(flow_state), Some(cookie_name)) =
        (callback.code, callback.state, cookie_name)
    else {
        return failed();
    };

    // Only the browser that started the attempt may finish it.
    let Some(attempt) = cookie(&headers, &cookie_name)
        .and_then(|sealed| Attempt::opened(&state.secrets, &sealed, &flow_state))
    else {
        tracing::warn!(
            "An OpenID Connect callback arrived for an attempt this browser did not start, or \
             one past its ten minutes"
        );
        return failed();
    };

    let subject = match crate::services::oidc::finish(&state, &code, attempt).await {
        Ok(subject) => subject,
        Err(e) => {
            tracing::warn!("An OpenID Connect sign-in failed: {e}");
            return failed();
        }
    };

    match accounts::open_session(&state.pool, &subject, AuthMode::Oidc.as_str()).await {
        // Appended: an array of headers inserts each one, so the second cookie
        // would replace the session the browser is being handed.
        Ok(id) => (
            axum::response::AppendHeaders([
                (
                    axum::http::header::SET_COOKIE,
                    session_cookie(&state, &headers, &id, accounts::SESSION_DAYS),
                ),
                (axum::http::header::SET_COOKIE, cleared.unwrap_or_default()),
            ]),
            axum::response::Redirect::to(&home),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("An OpenID Connect sign-in could not open its session: {e}");
            failed()
        }
    }
}

#[derive(serde::Deserialize)]
pub struct PasswordChange {
    pub current: String,
    pub new_password: String,
}

/// Replace the password, having proved the old one.
///
/// Behind the middleware, so a session already opened it, and still asking for
/// the current password, because a session left open on a shared machine is
/// exactly the case a password change must not be free in.
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    super::Json(change): super::Json<PasswordChange>,
) -> Response {
    let Ok(Some((_, hash))) = accounts::account(&state.pool).await else {
        return forbidden("This installation has no account to change");
    };
    // Through the throttle like a sign-in: this route is behind the middleware,
    // so the queue is not the point, and keeping argon2 off the runtime is. A
    // check left there holds a worker for the whole hash, and on a small
    // machine that is every other request waiting.
    // Busy reads as "not this password": the caller retries, and a change that
    // proceeded on a check that never ran would be the one bug here worth
    // fearing.
    let current_matches: bool =
        state.sign_in.verify(&change.current, &hash, None).await.unwrap_or_default();
    if !current_matches {
        return unauthorized();
    }
    if change.new_password.chars().count() < MIN_PASSWORD_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "bad_request",
                "message": format!("A password needs at least {MIN_PASSWORD_LENGTH} characters."),
            })),
        )
            .into_response();
    }

    let password_path = state.config.password_path();
    match accounts::set_password(&state.pool, &password_path, &change.new_password).await {
        // Every session it had opened is gone, including this one: the point of
        // changing a password is that what the old one reached is now closed.
        Ok(()) => (
            StatusCode::OK,
            [(axum::http::header::SET_COOKIE, session_cookie(&state, &headers, "", 0))],
            axum::Json(serde_json::json!({ "ok": true })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// Short enough not to argue with, long enough to be worth hashing.
const MIN_PASSWORD_LENGTH: usize = 12;

/// The `Set-Cookie` value, scoped to the mount point.
///
/// `Path` follows `base_path`: a cookie set at `/` would be sent to every other
/// application behind the same proxy. `SameSite=Lax` is what stops a
/// cross-site form from acting, and `HttpOnly` keeps it out of reach of any
/// script that ever slips past the CSP. `Secure` follows the scheme the browser
/// used: without it a cookie issued over TLS also travels on a plain `http://`
/// request to the same host, and a proxy that terminates TLS answers both.
fn session_cookie(state: &AppState, headers: &HeaderMap, id: &str, days: i64) -> String {
    let max_age = if days == 0 { 0 } else { days * 24 * 60 * 60 };
    cookie_header(state, headers, SESSION_COOKIE, id, max_age)
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

/// Whether the browser reached this request over TLS.
///
/// Routarr itself serves plain HTTP, and the proxy that terminates TLS says so
/// through `X-Forwarded-Proto`. An OIDC redirect registered as `https://` is
/// the same fact stated in the configuration, for a proxy that forwards
/// nothing.
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

    /// A subject worth storing, and the three cases where there is none.
    #[test]
    fn only_a_mode_that_names_somebody_produces_an_actor() {
        let named = Identity::person("alice".to_string(), AuthMode::Forms);
        assert_eq!(named.actor(), Some("alice"));

        // `none` and `external` share one subject that names nobody, and storing
        // it would read on the History screen exactly like an attribution.
        for source in [AuthMode::None, AuthMode::External] {
            assert_eq!(Identity::anonymous(source).actor(), None);
        }
    }

    /// nginx's usual `proxy_set_header X-Forwarded-Host $host` carries no port,
    /// and a site on a port of its own gets every write refused. The port the
    /// proxy states beside it, in `X-Forwarded-Port`, completes the host.
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
        assert!(same_origin(&write(Some("8443")), &[]));
        assert!(!same_origin(&write(Some("443")), &[]));
        assert!(!same_origin(&write(None), &[]));
    }

    /// Only a proxy the operator names in `ROUTARR_TRUSTED_PROXIES` names the
    /// client it forwards. Any other peer writing the header, a neighbour on
    /// the same network included, would choose a new address on every attempt
    /// and fill every place of the sign-in queue alone. A range holds a proxy
    /// recreated with a new address, and behind two listed proxies the client
    /// is the address the first of them saw.
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
