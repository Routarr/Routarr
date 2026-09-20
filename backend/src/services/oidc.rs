//! OpenID Connect, authorization code flow with PKCE.
//!
//! One level of access: whoever the provider lets through gets in. Routarr
//! reads no group claim and keeps no user table — the subject is stored beside
//! every decision and every write it causes, and that is the whole of what an
//! identity buys here.
//!
//! **On not verifying the ID token's signature.** This is a confidential client
//! running the authorization code flow, so the token arrives in the body of a
//! response to a request *this server* made to the token endpoint over TLS,
//! authenticated with the client secret. OpenID Connect Core §3.1.3.7 says that
//! in exactly this case "the TLS server validation MAY be used to validate the
//! issuer in place of checking the token signature". The claims are still
//! checked — issuer, audience, expiry and the nonce this flow generated — and
//! what is skipped is JWKS fetching, key rotation and a JWT crypto dependency,
//! none of which would add anything a compromised TLS channel had not already
//! taken away.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How long a sign-in attempt may sit unfinished.
///
/// Long enough to type a password and answer a second factor, short enough that
/// an abandoned attempt is not a row waiting to be replayed.
pub const FLOW_MINUTES: i64 = 10;

/// How many sign-in attempts may sit unfinished at once.
///
/// `/auth/oidc/start` is public, so anyone who reaches the port can insert
/// rows, and nothing but the hourly maintenance pass removed them. Bounding it
/// here rather than refusing past a threshold is deliberate: a cap that turns
/// callers away hands an attacker a way to deny sign-in to the one account
/// there is — the objection `SignInThrottle` already raises against lockouts.
/// The *oldest* attempts are dropped instead, and a browser that has just
/// started one is always among the newest.
///
/// Wide, because the rows are the only thing between an anonymous flood and
/// the operator's own attempt: at sixty-four, sixty-five requests in ten
/// minutes evicted the flow of somebody answering their provider, and the
/// rows cost a few hundred bytes each. Four thousand is a megabyte of disk
/// for ten minutes against a flood nobody ever sees on a homelab port.
const MAX_PENDING_FLOWS: i64 = 4096;

/// The two endpoints a sign-in needs, as the provider states them.
#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
}

/// Read the provider's own description of itself.
///
/// Discovery rather than four configured endpoints: a provider states its own
/// addresses, and copying them by hand is four more values to keep in step with
/// somebody else's deployment.
/// How long the provider's description is trusted without asking again.
///
/// A static document by specification, and the endpoint that reads it is
/// public: refetched per call, one cheap inbound request becomes one outbound
/// request against the operator's own identity provider, which is an amplifier
/// pointed at the thing they log in with. Fifteen minutes is short enough that
/// a provider moving an endpoint is picked up within a deploy window.
const DISCOVERY_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub async fn discover(state: &AppState) -> AppResult<Provider> {
    let issuer = state
        .config
        .oidc_issuer
        .as_deref()
        .ok_or_else(|| AppError::Config("ROUTARR_OIDC_ISSUER is not set".into()))?;

    // Read under a shared lock, so concurrent sign-ins do not queue behind each
    // other for a document they all already have.
    if let Some((provider, read_at)) = state.oidc_provider.read().await.as_ref()
        && read_at.elapsed() < DISCOVERY_TTL
    {
        return Ok(provider.clone());
    }

    let url = format!("{issuer}/.well-known/openid-configuration");
    let provider: Provider = crate::integrations::send_json("oidc", state.http.get(&url)).await?;

    // The document has to describe the issuer it was fetched from, or a
    // redirect has quietly moved the whole conversation somewhere else.
    if provider.issuer.trim_end_matches('/') != issuer {
        return Err(AppError::Config(format!(
            "the provider at {url} calls itself '{}', not '{issuer}'",
            provider.issuer
        )));
    }
    // The flow trusts the channel in place of a token signature (see the
    // module doc), and the endpoints are only known once the provider has
    // described itself: one served in the clear is refused here, as the
    // issuer is at startup.
    for (name, endpoint) in [
        ("token_endpoint", &provider.token_endpoint),
        ("authorization_endpoint", &provider.authorization_endpoint),
    ] {
        if !crate::config::reaches_over_tls(endpoint) {
            return Err(AppError::Config(format!(
                "the provider's {name} is '{endpoint}', which is not https"
            )));
        }
    }
    *state.oidc_provider.write().await = Some((provider.clone(), std::time::Instant::now()));
    Ok(provider)
}

/// Everything the browser has to be sent to, for one attempt.
pub struct Start {
    pub redirect_to: String,
    /// The attempt's `state`, which the browser that left carries back in a
    /// cookie: without it, any browser presenting a known `code` and `state`
    /// pair is signed in as whoever started the attempt.
    pub state: String,
}

/// Begin a sign-in: record the attempt and build the provider's URL.
pub async fn start(state: &AppState) -> AppResult<Start> {
    let provider = discover(state).await?;
    let client_id = require(&state.config.oidc_client_id, "ROUTARR_OIDC_CLIENT_ID")?;
    let redirect_uri = require(&state.config.oidc_redirect_url, "ROUTARR_OIDC_REDIRECT_URL")?;

    let flow_state = crate::crypto::generate_secret()?;
    let nonce = crate::crypto::generate_secret()?;
    let verifier = crate::crypto::generate_secret()?;

    sqlx::query(
        "INSERT INTO oidc_flows (state, nonce, verifier, expires_at)
         VALUES (?, ?, ?, datetime('now', ?))",
    )
    .bind(&flow_state)
    .bind(&nonce)
    .bind(&verifier)
    .bind(format!("+{FLOW_MINUTES} minutes"))
    .execute(&state.pool)
    .await?;

    // After the insert, so the attempt just started is never the one dropped.
    // `expires_at` is stamped from `now` and indexed, so ordering by it orders
    // by when the attempt began.
    sqlx::query(
        "DELETE FROM oidc_flows
          WHERE expires_at <= datetime('now')
             OR state NOT IN (SELECT state FROM oidc_flows ORDER BY expires_at DESC LIMIT ?)",
    )
    .bind(MAX_PENDING_FLOWS)
    .execute(&state.pool)
    .await?;

    // S256, never `plain`: the challenge is what travels through the browser,
    // and a plain one is the verifier itself.
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));

    let query = [
        ("response_type", "code"),
        ("scope", "openid profile"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("state", &flow_state),
        ("nonce", &nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    let url =
        reqwest::Url::parse_with_params(&provider.authorization_endpoint, query).map_err(|e| {
            AppError::Config(format!("the provider's authorization URL is unusable: {e}"))
        })?;

    Ok(Start { redirect_to: url.to_string(), state: flow_state })
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// The claims this flow checks. Everything else the provider sends is ignored.
#[derive(Debug, Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    #[serde(default)]
    aud: Audience,
    exp: i64,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
}

/// `aud` is a string or an array of them, and providers use both.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    Absent,
}

impl Audience {
    fn contains(&self, client_id: &str) -> bool {
        match self {
            Self::One(value) => value == client_id,
            Self::Many(values) => values.iter().any(|v| v == client_id),
            Self::Absent => false,
        }
    }
}

/// Finish a sign-in and return the subject the provider vouched for.
pub async fn finish(state: &AppState, code: &str, flow_state: &str) -> AppResult<String> {
    // Taken, not read: a row that survives its use is an authorisation code
    // that can be presented twice.
    let flow: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM oidc_flows WHERE state = ? AND expires_at > datetime('now')
         RETURNING nonce, verifier",
    )
    .bind(flow_state)
    .fetch_optional(&state.pool)
    .await?;

    let Some((nonce, verifier)) = flow else {
        return Err(AppError::BadRequest(
            "This sign-in attempt is unknown or has expired. Start again.".into(),
        ));
    };

    let provider = discover(state).await?;
    let client_id = require(&state.config.oidc_client_id, "ROUTARR_OIDC_CLIENT_ID")?;
    let client_secret = require(&state.config.oidc_client_secret, "ROUTARR_OIDC_CLIENT_SECRET")?;
    let redirect_uri = require(&state.config.oidc_redirect_url, "ROUTARR_OIDC_REDIRECT_URL")?;

    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", &verifier),
    ];
    let tokens: TokenResponse = crate::integrations::send_json(
        "oidc",
        state.http.post(&provider.token_endpoint).form(&form),
    )
    .await?;

    let claims = decode_claims(&tokens.id_token)?;

    if claims.iss.trim_end_matches('/') != provider.issuer.trim_end_matches('/') {
        return Err(AppError::BadRequest("The token names another issuer.".into()));
    }
    if !claims.aud.contains(client_id) {
        return Err(AppError::BadRequest("The token was issued for another client.".into()));
    }
    if claims.exp <= chrono::Utc::now().timestamp() {
        return Err(AppError::BadRequest("The token has expired.".into()));
    }
    // The one claim that ties this token to the attempt this browser started.
    if claims.nonce.as_deref() != Some(nonce.as_str()) {
        return Err(AppError::BadRequest("The token belongs to another sign-in.".into()));
    }

    Ok(claims.preferred_username.unwrap_or(claims.sub))
}

/// The payload of a JWT, without verifying its signature.
///
/// See the module note: the token came from the token endpoint over TLS, which
/// §3.1.3.7 accepts in place of the signature. What this must still do is
/// refuse anything that is not a JWT rather than reading a fragment of one.
fn decode_claims(token: &str) -> AppResult<Claims> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| AppError::BadRequest("The provider did not return a JWT.".into()))?;
    let bytes = B64URL
        .decode(payload)
        .map_err(|_| AppError::BadRequest("The token's payload is not base64url.".into()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::BadRequest(format!("The token's claims are unreadable: {e}")))
}

fn require<'a>(value: &'a Option<String>, name: &str) -> AppResult<&'a str> {
    value.as_deref().ok_or_else(|| AppError::Config(format!("{name} is not set")))
}

/// Drop the attempts nobody came back from.
pub async fn purge_expired_flows(pool: &sqlx::SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM oidc_flows WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: serde_json::Value) -> String {
        format!("header.{}.signature", B64URL.encode(payload.to_string()))
    }

    #[test]
    fn an_audience_is_a_string_or_a_list_and_both_are_read() {
        let one = decode_claims(&jwt(serde_json::json!({
            "iss": "https://idp", "sub": "u1", "aud": "routarr", "exp": 1
        })))
        .unwrap();
        assert!(one.aud.contains("routarr"));
        assert!(!one.aud.contains("other"));

        let many = decode_claims(&jwt(serde_json::json!({
            "iss": "https://idp", "sub": "u1", "aud": ["a", "routarr"], "exp": 1
        })))
        .unwrap();
        assert!(many.aud.contains("routarr"));

        // Absent must not read as "matches everything".
        let none =
            decode_claims(&jwt(serde_json::json!({ "iss": "https://idp", "sub": "u1", "exp": 1 })))
                .unwrap();
        assert!(!none.aud.contains("routarr"));
    }

    /// Anything that is not a JWT must be refused rather than read in part.
    #[test]
    fn a_token_that_is_not_one_is_refused() {
        assert!(decode_claims("not a jwt").is_err());
        assert!(decode_claims("header.!!!not base64!!!.sig").is_err());
        assert!(decode_claims(&format!("header.{}.sig", B64URL.encode("not json"))).is_err());
    }

    #[test]
    fn the_subject_falls_back_to_sub_when_there_is_no_username() {
        let claims = decode_claims(&jwt(serde_json::json!({
            "iss": "https://idp", "sub": "abc-123", "aud": "routarr", "exp": 1
        })))
        .unwrap();
        assert!(claims.preferred_username.is_none());
        assert_eq!(claims.sub, "abc-123");
    }

    #[tokio::test]
    async fn a_flow_is_taken_once_and_expires() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO oidc_flows (state, nonce, verifier, expires_at)
             VALUES ('s1', 'n', 'v', datetime('now', '+10 minutes')),
                    ('stale', 'n', 'v', datetime('now', '-1 minute'))",
        )
        .execute(&pool)
        .await
        .unwrap();

        let taken: Option<(String, String)> = sqlx::query_as(
            "DELETE FROM oidc_flows WHERE state = ? AND expires_at > datetime('now')
             RETURNING nonce, verifier",
        )
        .bind("s1")
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(taken.is_some(), "a live flow must be taken");

        // The same state a second time finds nothing: a code cannot be replayed.
        let again: Option<(String, String)> = sqlx::query_as(
            "DELETE FROM oidc_flows WHERE state = ? AND expires_at > datetime('now')
             RETURNING nonce, verifier",
        )
        .bind("s1")
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(again.is_none(), "a flow survived its own use");

        assert_eq!(purge_expired_flows(&pool).await.unwrap(), 1);
    }
}
