//! What the owner does to their own ways in: the API key, the password and
//! the sessions, and the proof a session gives before it touches a key.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::auth::{
    Client, Identity, SESSION_COOKIE, allowed, audited, constant_time_eq, cookie, refusal,
};
use crate::config::AuthMode;
use crate::error::{AppError, AppResult};
use crate::services::accounts::{self, RECENT_SIGN_IN_SECONDS};
use crate::services::{applications, audit};
use crate::state::AppState;

/// What a person signed in through a session sends before a key is made or
/// withdrawn: the password in `forms`, the API key in `apikey`. A session left
/// open on a shared machine would otherwise make keys that outlive it.
#[derive(Debug, Default, serde::Deserialize)]
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
            if let Some(left) = state.sign_in.held_back(client) {
                let seconds = left.as_secs() + 1;
                let message = localizer
                    .translate("ErrorSignInHeldBack", &[("seconds", &seconds.to_string())]);
                return Err(AppError::TooManyRequests { message, retry_after: seconds });
            }
            let Some((_, hash)) = accounts::account(&state.pool).await? else {
                return Err(AppError::Forbidden(
                    localizer.translate("ErrorProofPasswordWrong", &[]),
                ));
            };
            let password = proof.current_password.as_deref().unwrap_or_default();
            match state.sign_in.verify(password, &hash, client).await {
                Ok(true) => true,
                Ok(false) => {
                    state.sign_in.failed(client);
                    false
                }
                Err(accounts::Busy) => {
                    return Err(AppError::Busy(localizer.translate("ErrorSignInBusy", &[])));
                }
            }
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

/// End the sessions a browser opened with the API key, but `keep`: they were
/// opened with a key that no longer opens anything.
async fn end_key_sessions(state: &AppState, keep: Option<&str>) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE source = ? AND id != ?")
        .bind(AuthMode::ApiKey.as_str())
        .bind(keep.map(accounts::stored).unwrap_or_default())
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Mint a new API key, replacing whatever was there.
///
/// The value is returned exactly once. Storing it to show again later would
/// make every subsequent read of this screen a second chance to copy it, which
/// is the property a credential should not have.
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
    end_key_sessions(&state, cookie(&headers, SESSION_COOKIE).as_deref()).await?;
    audited(&state, &identity, client, allowed(audit::Kind::ApiKey, "AuditApiKeyReplaced"));
    Ok(super::Json(serde_json::json!({ "api_key": key })))
}

/// Withdraw the key, leaving the session as the only way in.
pub async fn delete_api_key(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    body: axum::body::Bytes,
) -> AppResult<StatusCode> {
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
    prove(&state, &identity, &proof_in(&body)?, client).await?;
    state.clear_api_key().await?;
    audited(&state, &identity, client, allowed(audit::Kind::ApiKey, "AuditApiKeyWithdrawn"));
    Ok(StatusCode::NO_CONTENT)
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
pub struct PasswordChange {
    pub current: String,
    pub new_password: String,
    /// Also revoke every application key and replace the API key.
    #[serde(default)]
    pub revoke_keys: bool,
}

/// Replace the password, having proved the old one.
///
/// Behind the middleware, so a session already opened it, and still asking for
/// the current password, because a session left open on a shared machine is
/// exactly the case a password change must not be free in.
pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Client(client): Client,
    headers: HeaderMap,
    super::Json(change): super::Json<PasswordChange>,
) -> AppResult<Response> {
    let localizer = state.localizer().await;
    let Some((_, hash)) = accounts::account(&state.pool).await? else {
        return Err(AppError::Forbidden("This installation has no account to change".into()));
    };
    // Through the throttle like a sign-in: this route is behind the middleware,
    // so the queue is not the point, and keeping argon2 off the runtime is. A
    // check left there holds a worker for the whole hash, and on a small
    // machine that is every other request waiting.
    let matched = match state.sign_in.verify(&change.current, &hash, client).await {
        Ok(matched) => matched,
        Err(accounts::Busy) => {
            return Err(AppError::Busy(localizer.translate("ErrorSignInBusy", &[])));
        }
    };
    if !matched {
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
    // Every session the old password opened is gone, this one included: the
    // point of changing a password is that what the old one reached is closed.
    // The browser that changed it proved the old one a moment ago, and gets a
    // new session, so a key shown once in this answer is not lost to the
    // sign-in screen.
    let source = AuthMode::Forms.as_str();
    let id = accounts::open_session(&state.pool, &identity.subject, source).await?;
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
    let current = cookie(&headers, SESSION_COOKIE);
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
    let current = cookie(&headers, SESSION_COOKIE)
        .is_some_and(|id| accounts::stored(&id).starts_with(&handle));
    if !accounts::end_session(&state.pool, &handle).await? {
        return Err(AppError::NotFound("No live session has that handle".into()));
    }
    let event = allowed(audit::Kind::SignOut, "AuditSessionEnded").with("handle", &handle);
    audited(&state, &identity, client, event);
    if current {
        let cleared = super::auth::session_cookie(&state, &headers, "", 0);
        return Ok(
            ([(axum::http::header::SET_COOKIE, cleared)], StatusCode::NO_CONTENT).into_response()
        );
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
