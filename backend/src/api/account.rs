//! What the owner does to their own ways in: the API key, the password and
//! the sessions, and the proof a session gives before it touches a key.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::auth::{
    Client, Identity, allowed, audited, constant_time_eq, cookie, refusal, session_cookie_name,
};
use crate::config::AuthMode;
use crate::error::{AppError, AppResult};
use crate::services::accounts::{self, OpenedUnder, RECENT_SIGN_IN_SECONDS};
use crate::services::{applications, audit};
use crate::state::AppState;

/// What a person signed in through a session sends before a key is made or
/// withdrawn: the password in `forms`, the API key in `apikey`. A session left
/// open on a shared machine would otherwise make keys that outlive it.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    #[serde(default)]
    pub current_password: Option<String>,
    #[serde(default)]
    pub current_key: Option<String>,
}

/// The proof a request body carries. A script sending the key in a header
/// has nothing to prove, and may send no body at all.
pub fn proof_in(body: &[u8]) -> AppResult<Proof> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Proof::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("The body is not a proof this route reads: {e}")))
}

/// Refuse what `identity` asks unless it proved itself again.
///
/// A key sent in a header is its own proof. A session in `oidc` needs a sign-in
/// made within [`RECENT_SIGN_IN_SECONDS`], which the provider is asked for with
/// `max_age=0`. A wrong password counts toward the wait of the address it came
/// from, as a sign-in does.
pub async fn prove(
    state: &AppState,
    identity: &Identity,
    proof: &Proof,
    client: Option<std::net::IpAddr>,
) -> AppResult<()> {
    let Some(signed_in_at) = identity.signed_in_at else {
        return Ok(());
    };
    let localizer = state.localizer().await;
    let proven = match identity.source {
        AuthMode::Forms => {
            let password = proof.current_password.as_deref().unwrap_or_default();
            current_password_matches(state, password, client, &localizer).await?
        }
        AuthMode::ApiKey => match (state.api_key(), proof.current_key.as_deref()) {
            (Some(expected), Some(sent)) => constant_time_eq(sent.trim(), &expected),
            _ => false,
        },
        AuthMode::Oidc => {
            if chrono::Utc::now().timestamp() - signed_in_at > RECENT_SIGN_IN_SECONDS {
                return Err(AppError::Reauthenticate(localizer.translate("ErrorSignInAgain", &[])));
            }
            true
        }
        AuthMode::None | AuthMode::External => true,
    };
    if proven {
        return Ok(());
    }
    let event = refusal(audit::Kind::Proof, "AuditProofRefused");
    state.audit.record(event.by(Some(&identity.subject), client));
    let key = if identity.source == AuthMode::ApiKey {
        "ErrorProofKeyWrong"
    } else {
        "ErrorProofPasswordWrong"
    };
    Err(AppError::Forbidden(localizer.translate(key, &[])))
}

/// Whether `password` is the account's, asked as a sign-in asks it: refused unread while the
/// address waits after its failures, and a wrong one counted toward that wait.
async fn current_password_matches(
    state: &AppState,
    password: &str,
    client: Option<std::net::IpAddr>,
    localizer: &crate::localization::Localizer,
) -> AppResult<bool> {
    if let Some(left) = state.sign_in.held_back(client) {
        let seconds = left.as_secs() + 1;
        let message =
            localizer.translate("ErrorSignInHeldBack", &[("seconds", &seconds.to_string())]);
        return Err(AppError::TooManyRequests { message, retry_after: seconds });
    }
    let Some((_, hash)) = accounts::account(&state.pool).await? else {
        return Ok(false);
    };
    match state.sign_in.verify(password, &hash, client).await {
        Ok(true) => Ok(true),
        Ok(false) => {
            state.sign_in.failed(client);
            Ok(false)
        }
        Err(accounts::Busy) => Err(AppError::Busy(localizer.translate("ErrorSignInBusy", &[]))),
    }
}

fn refuse_if_pinned(state: &AppState) -> AppResult<()> {
    if state.config.api_key.is_some() {
        return Err(AppError::Conflict {
            reason: "key_pinned",
            message:
                "ROUTARR_API_KEY sets this key, so it cannot be changed here. Change the variable \
             and restart."
                    .into(),
        });
    }
    Ok(())
}

/// End the sessions a browser opened with the API key, but `keep`: they were
/// opened with a key that no longer opens anything, which also refuses them.
async fn end_key_sessions(state: &AppState, keep: Option<&str>) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE source = ? AND id != ?")
        .bind(AuthMode::ApiKey.as_str())
        .bind(keep.map(accounts::stored).unwrap_or_default())
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Mint a new API key, replacing whatever was there. This answer is the only time the value is
/// shown.
pub async fn rotate_api_key(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> AppResult<super::Json<serde_json::Value>> {
    refuse_if_pinned(&state)?;
    prove(&state, &identity, &proof_in(&body)?, client).await?;
    let key = state.rotate_api_key().await?;
    let asking = cookie(&headers, session_cookie_name(&state, &headers));
    if let Some(id) = asking.as_deref() {
        let source = AuthMode::ApiKey.as_str();
        accounts::rebind(&state.pool, id, source, &OpenedUnder::Key(key.clone())).await?;
    }
    end_key_sessions(&state, asking.as_deref()).await?;
    audited(&state, &identity, client, allowed(audit::Kind::ApiKey, "AuditApiKeyReplaced"));
    Ok(super::Json(serde_json::json!({ "api_key": key })))
}

/// Withdraw the key, leaving the session as the only way in.
pub async fn delete_api_key(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    body: axum::body::Bytes,
) -> AppResult<super::Json<super::Deleted>> {
    refuse_if_pinned(&state)?;
    // Gone, the key would lock out every request, the one restoring it included.
    if state.config.auth_mode == AuthMode::ApiKey {
        return Err(AppError::Conflict { reason: "only_credential", message: "The API key is the only way in while ROUTARR_AUTH=apikey. Switch to a session mode \
             before removing it."
                .into() });
    }
    prove(&state, &identity, &proof_in(&body)?, client).await?;
    state.clear_api_key().await?;
    audited(&state, &identity, client, allowed(audit::Kind::ApiKey, "AuditApiKeyWithdrawn"));
    Ok(super::Json(super::Deleted { deleted: true }))
}

/// Revoke every application key and replace the API key, unless the
/// environment pins it or there is none, for an owner who believes a key
/// leaked with the password. Returns the new API key, shown once.
pub(crate) async fn revoke_every_key(state: &AppState) -> AppResult<Option<String>> {
    let revoked = applications::revoke_all(&state.pool).await?;
    let key = if state.config.api_key.is_none() && state.api_key().is_some() {
        Some(state.rotate_api_key().await?)
    } else {
        None
    };
    end_key_sessions(state, None).await?;
    tracing::info!("Every key was revoked: {revoked} application key(s), the API key replaced");
    Ok(key)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordChange {
    pub current: String,
    pub new_password: String,
    /// Also revoke every application key and replace the API key.
    #[serde(default)]
    pub revoke_keys: bool,
}

/// Replace the password, having proved the old one.
///
/// The current password is asked even behind a session, since a shared machine may have left
/// one open. Outside `forms` no sign-in reads the account, so there is none to change.
pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    headers: HeaderMap,
    super::Json(change): super::Json<PasswordChange>,
) -> AppResult<Response> {
    if state.config.auth_mode != AuthMode::Forms {
        return Err(AppError::NotFound("No such API route.".into()));
    }
    let localizer = state.localizer().await;
    if !current_password_matches(&state, &change.current, client, &localizer).await? {
        let event = refusal(audit::Kind::Password, "AuditPasswordRefused");
        state.audit.record(event.by(Some(&identity.subject), client));
        return Err(AppError::Forbidden(localizer.translate("ErrorProofPasswordWrong", &[])));
    }
    let shortest = super::auth::MIN_PASSWORD_LENGTH;
    if change.new_password.chars().count() < shortest {
        let refusal =
            localizer.translate("ErrorPasswordTooShort", &[("min", &shortest.to_string())]);
        return Err(AppError::BadRequest(refusal));
    }

    accounts::set_password(&state.pool, &state.config.password_path(), &change.new_password)
        .await?;
    audited(&state, &identity, client, allowed(audit::Kind::Password, "AuditPasswordChanged"));
    let api_key = if change.revoke_keys {
        let key = revoke_every_key(&state).await?;
        let event = allowed(audit::Kind::ApiKey, "AuditEveryKeyRevoked");
        audited(&state, &identity, client, event);
        key
    } else {
        None
    };
    // `set_password` ended every session, this one included. This browser gets a new one, so a
    // key shown once here is not lost to the sign-in screen.
    let source = AuthMode::Forms.as_str();
    let (full, account) = (accounts::Reach::Full, &OpenedUnder::Account);
    let id = accounts::open_session(&state.pool, &identity.subject, source, full, account).await?;
    let renewed =
        super::auth::session_cookie(&state, &headers, &id, accounts::opening_seconds(source));
    Ok((
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, renewed)],
        axum::Json(serde_json::json!({ "ok": true, "api_key": api_key })),
    )
        .into_response())
}

/// The live sessions, the one this request holds marked.
pub async fn sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<super::Json<Vec<accounts::Listed>>> {
    let current = cookie(&headers, session_cookie_name(&state, &headers));
    Ok(super::Json(accounts::list_sessions(&state.pool, current.as_deref()).await?))
}

/// Sign out everywhere: every session ends, this one included.
pub async fn end_sessions(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    headers: HeaderMap,
) -> AppResult<Response> {
    let ended = accounts::end_every_session(&state.pool).await?;
    let event = allowed(audit::Kind::SignOut, "AuditEverySessionEnded").with("count", ended);
    audited(&state, &identity, client, event);
    Ok((
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, super::auth::session_cookie(&state, &headers, "", 0))],
        axum::Json(serde_json::json!({ "ended": ended })),
    )
        .into_response())
}

/// End one session by the handle the list gives it.
pub async fn end_session(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    headers: HeaderMap,
    super::Path(handle): super::Path<String>,
) -> AppResult<Response> {
    let current = cookie(&headers, session_cookie_name(&state, &headers))
        .is_some_and(|id| accounts::stored(&id).starts_with(&handle));
    if !accounts::end_session(&state.pool, &handle).await? {
        return Err(AppError::NotFound("No live session has that handle".into()));
    }
    let event = allowed(audit::Kind::SignOut, "AuditSessionEnded").with("handle", &handle);
    audited(&state, &identity, client, event);
    let ended = super::Json(super::Deleted { deleted: true });
    if current {
        let cleared = super::auth::session_cookie(&state, &headers, "", 0);
        return Ok(([(axum::http::header::SET_COOKIE, cleared)], ended).into_response());
    }
    Ok(ended.into_response())
}
