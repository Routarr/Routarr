//! Authentication, one mode at a time.
//!
//! Routarr sits in a homelab, often behind a reverse proxy, but its API can
//! move files on disk — so a protected call carries a credential unless the
//! operator asked for none. Every mode ends at the same place: an [`Identity`]
//! in the request's extensions, which is what the rest of the application
//! reads. Nothing downstream knows which mode produced it, and that is the
//! point — adding a mode is one arm here, not a change everywhere.
//!
//! There are no roles. The Servarr applications have none either: their `User`
//! model carries a name and a password hash and nothing else, and a single
//! account. Access is all or nothing, and who gets in is the mode's business.

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{body::Body, http::Request};
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::config::AuthMode;
use crate::error::{AppError, AppResult};
use crate::services::accounts;

/// The cookie the `forms` mode sets. Named for the application, since a browser
/// pointed at several homelab services holds all of their cookies at once.
pub const SESSION_COOKIE: &str = "routarr_session";
/// Carries an OIDC attempt's `state` from the browser that left to the
/// browser that comes back. The callback accepts a `code` for the attempt it
/// names only from that browser: without it, a link carrying someone else's
/// `code` and `state` signed the reader in as that someone (login CSRF).
pub const OIDC_COOKIE: &str = "routarr_oidc";

/// Who is making a request, once a mode has decided.
///
/// It carries no role on purpose: there is one level of access, and the mode
/// decides who reaches it. The subject is what the `subject` column of
/// `decisions` and `execution_logs` records, when the mode names anybody —
/// see [`Identity::actor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub source: AuthMode,
}

impl Identity {
    /// The caller nobody had to name: `none` lets everyone through under it.
    fn anonymous(source: AuthMode) -> Self {
        Self { subject: "anonymous".to_string(), source }
    }

    /// The name worth recording on a write, if the mode vouched for one.
    ///
    /// `none` and `external` let everybody through under one shared subject, so
    /// storing it would fill an audit column with a word that names nobody —
    /// and reads, on the History screen, exactly like an attribution.
    pub fn actor(&self) -> Option<&str> {
        match self.source {
            AuthMode::None | AuthMode::External => None,
            _ => Some(&self.subject),
        }
    }
}

/// Resolve an identity for the request, or refuse it.
///
/// The one place a mode is consulted. A handler that needs to know who called
/// reads `Extension<Identity>`; nothing needs to ask how they proved it.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let identity = match state.config.auth_mode {
        AuthMode::None => Some(Identity::anonymous(AuthMode::None)),
        // Nothing is asked for and no header is read. The proxy in front has
        // already decided, and reading a name it sends would only invent a
        // trust boundary where none is enforceable.
        AuthMode::External => Some(Identity::anonymous(AuthMode::External)),
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
                Some(_) if !same_origin(&request) => {
                    return forbidden(
                        "This request did not come from Routarr: its Origin is not the host it \
                         was sent to. Behind a reverse proxy, forward the public host as \
                         X-Forwarded-Host.",
                    );
                }
                other => other,
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
            next.run(request).await
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

/// A valid API key, whatever the mode.
fn api_key_identity(state: &AppState, headers: &HeaderMap) -> Option<Identity> {
    let expected = state.api_key()?;
    extract_key(headers)
        .filter(|provided| constant_time_eq(provided, &expected))
        .map(|_| Identity { subject: "apikey".to_string(), source: AuthMode::ApiKey })
}

/// The identity a live session cookie names, when the mode in force opened it.
async fn session_identity(state: &AppState, headers: &HeaderMap) -> Option<Identity> {
    let id = cookie(headers, SESSION_COOKIE)?;
    let mode = state.config.auth_mode;
    let subject =
        accounts::session_subject(&state.pool, &id, mode.as_str()).await.ok().flatten()?;
    Some(Identity { subject, source: mode })
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
/// write of every session behind it was refused.
fn same_origin(request: &Request<Body>) -> bool {
    if !matches!(*request.method(), Method::POST | Method::PUT | Method::DELETE | Method::PATCH) {
        return true;
    }
    let headers = request.headers();
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return true;
    };
    let Some(host) = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(',').next())
        .map(str::trim)
    else {
        return false;
    };
    origin.to_str().ok().is_some_and(|origin| origin_names(origin, host))
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
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_string())
}

/// Compare without leaking the position of the first differing byte.
fn constant_time_eq(a: &str, b: &str) -> bool {
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
    // refuses every request once it is gone — including the one that would put
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

/// Exchange a username and a password for a session cookie.
///
/// One message for a wrong name and a wrong password alike: saying which was
/// wrong tells an attacker that the other was right.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    super::Json(credentials): super::Json<Credentials>,
) -> Response {
    let stored = accounts::account(&state.pool).await.ok().flatten();
    let Some((username, hash)) = stored else {
        return unauthorized();
    };

    // One cheap refusal before the expensive one. A password shorter than the
    // minimum cannot be the stored one — `change_password` refuses to set one —
    // so hashing it would spend 355 ms proving what its length already says.
    // The minimum is public: the refusal above states it.
    if credentials.password.chars().count() < MIN_PASSWORD_LENGTH {
        return unauthorized();
    }
    // The username is *not* checked first: answering at once on a wrong name
    // and 355 ms later on the right one tells a caller which name exists. The
    // hash is verified either way and the name is required to match too.
    let name_matches = username == credentials.username.trim();

    // Bounded, and off the runtime. See `SignInThrottle`: argon2id is what
    // makes this endpoint expensive to serve as well as hard to guess, and it
    // is the machine that needs defending rather than the password.
    let matched = match state.sign_in.verify(&credentials.password, &hash).await {
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
        return unauthorized();
    }

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

/// Who this request is, for a shell that wants to show a name.
pub async fn me(
    axum::Extension(identity): axum::Extension<Identity>,
) -> super::Json<serde_json::Value> {
    super::Json(serde_json::json!({ "subject": identity.subject }))
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
    match crate::services::oidc::start(&state).await {
        Ok(start) => (
            [(
                axum::http::header::SET_COOKIE,
                cookie_header(
                    &state,
                    &headers,
                    OIDC_COOKIE,
                    &start.state,
                    crate::services::oidc::FLOW_MINUTES * 60,
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
}

/// Where the provider sends the browser back.
///
/// It answers with a redirect in every case, because the caller is a browser
/// following a link and not a client reading JSON. A failure lands on the
/// application with a marker the shell can show.
pub async fn oidc_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(callback): axum::extract::Query<Callback>,
) -> Response {
    let home = home(&state);

    // The attempt is over either way, so the cookie that carried it goes.
    let cleared = cookie_header(&state, &headers, OIDC_COOKIE, "", 0);
    let failed = || {
        (
            [(axum::http::header::SET_COOKIE, cleared.clone())],
            axum::response::Redirect::to(&format!("{home}?signin=failed")),
        )
            .into_response()
    };

    let (Some(code), Some(flow_state), None) =
        (callback.code, callback.state, callback.error.as_deref())
    else {
        return failed();
    };

    // Only the browser that started the attempt may finish it. Refused before
    // the row is taken: a pair presented from elsewhere must not cost the
    // browser that is answering its provider the attempt it started.
    if cookie(&headers, OIDC_COOKIE).as_deref() != Some(flow_state.as_str()) {
        tracing::warn!(
            "An OpenID Connect callback arrived from a browser that did not start the attempt"
        );
        return failed();
    }

    let subject = match crate::services::oidc::finish(&state, &code, &flow_state).await {
        Ok(subject) => subject,
        Err(e) => {
            tracing::warn!("An OpenID Connect sign-in failed: {e}");
            return failed();
        }
    };

    match accounts::open_session(&state.pool, &subject, AuthMode::Oidc.as_str()).await {
        Ok(id) => (
            [
                (
                    axum::http::header::SET_COOKIE,
                    session_cookie(&state, &headers, &id, accounts::SESSION_DAYS),
                ),
                (axum::http::header::SET_COOKIE, cleared),
            ],
            axum::response::Redirect::to(&home),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct PasswordChange {
    pub current: String,
    pub new_password: String,
}

/// Replace the password, having proved the old one.
///
/// Behind the middleware, so a session already opened it — and still asking for
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
    // so the queue is not the point — keeping argon2 off the runtime is. A
    // check left there holds a worker for 355 ms, and on a small machine that
    // is every other request waiting.
    // Busy reads as "not this password": the caller retries, and a change that
    // proceeded on a check that never ran would be the one bug here worth
    // fearing.
    let current_matches: bool =
        state.sign_in.verify(&change.current, &hash).await.unwrap_or_default();
    if !current_matches {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(
                serde_json::json!({ "error": "unauthorized", "message": "Wrong username or password." }),
            ),
        )
            .into_response();
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

    match accounts::set_password(&state.pool, &change.new_password).await {
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
/// guards; `max_age` 0 clears it.
fn cookie_header(
    state: &AppState,
    headers: &HeaderMap,
    name: &str,
    value: &str,
    max_age: i64,
) -> String {
    let path = if state.config.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", state.config.base_path)
    };
    let secure = if came_over_tls(state, headers) { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// Whether the browser reached this request over TLS.
///
/// Routarr itself serves plain HTTP; the proxy that terminates TLS says so
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
        let named = Identity { subject: "alice".to_string(), source: AuthMode::Forms };
        assert_eq!(named.actor(), Some("alice"));

        // `none` and `external` share one subject that names nobody; storing it
        // would read on the History screen exactly like an attribution.
        for source in [AuthMode::None, AuthMode::External] {
            assert_eq!(Identity::anonymous(source).actor(), None);
        }
    }

    #[test]
    fn reads_x_api_key() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_static("abc"));
        assert_eq!(extract_key(&h).as_deref(), Some("abc"));
    }

    #[test]
    fn reads_bearer_token() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
        assert_eq!(extract_key(&h).as_deref(), Some("abc"));
    }

    #[test]
    fn ignores_other_schemes() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(extract_key(&h), None);
    }

    #[test]
    fn comparison_rejects_length_mismatch() {
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("abc", "abc"));
    }
}
