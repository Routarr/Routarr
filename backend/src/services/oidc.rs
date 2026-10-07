//! OpenID Connect, authorization code flow with PKCE.
//!
//! One level of access, for the people the operator names: by their `sub`, or
//! by a group the provider says they belong to (`ROUTARR_OIDC_ALLOWED_*`).
//! Routarr keeps no user table. The subject is stored beside every decision
//! and every write it causes, and that is the whole of what an identity buys
//! here.
//!
//! An attempt keeps nothing on the server. Its `state`, nonce, PKCE verifier
//! and expiry travel in a cookie sealed with the master key, so the public
//! route that starts one writes nothing, and a flood of starts evicts nobody's
//! attempt. A code presented twice is refused by the provider, which spends it
//! at the first exchange (RFC 6749 §4.1.2), and the callback clears the cookie.
//!
//! **On not verifying the ID token's signature.** This is a confidential client
//! running the authorization code flow, so the token arrives in the body of a
//! response to a request *this server* made to the token endpoint over TLS,
//! authenticated with the client secret. OpenID Connect Core §3.1.3.7 says that
//! in exactly this case "the TLS server validation MAY be used to validate the
//! issuer in place of checking the token signature". The claims are still
//! checked (issuer, audience, expiry and the nonce this flow generated). What
//! is skipped is JWKS fetching, key rotation and a JWT crypto dependency, none
//! of which would add anything a compromised TLS channel had not already taken
//! away.

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

/// What sets this cookie apart from any other value sealed with the master
/// key, an Arr key among them: one presented as an attempt is refused.
const PURPOSE: &str = "oidc-attempt";

/// One attempt, as its cookie carries it.
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct Attempt {
    purpose: String,
    pub state: String,
    nonce: String,
    verifier: String,
    /// Unix seconds.
    expires: i64,
    /// Set when the person has to sign in again at the provider, whatever
    /// session it holds: the token must then say they just did (`auth_time`).
    #[serde(default)]
    pub again: bool,
    /// The screen to land on afterwards, a path under the mount point.
    #[serde(default)]
    pub return_to: Option<String>,
}

/// A screen a sign-in may land on: a path under the mount point, and nothing
/// that leaves it.
pub fn screen(path: &str) -> Option<String> {
    let path = path.trim();
    let inside = path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.contains("://")
        && path.chars().all(|c| c.is_ascii_graphic());
    inside.then(|| path.to_string())
}

impl Attempt {
    /// The attempt sealed for its cookie.
    pub fn sealed(&self, secrets: &crate::crypto::SecretBox) -> AppResult<String> {
        secrets.seal(&serde_json::to_string(self)?)
    }

    /// The attempt a cookie carries, when it is one, it is still live and it
    /// is the one `state` names.
    pub fn opened(secrets: &crate::crypto::SecretBox, sealed: &str, state: &str) -> Option<Self> {
        use subtle::ConstantTimeEq;
        if !crate::crypto::SecretBox::is_sealed(sealed) {
            return None;
        }
        let attempt: Self = serde_json::from_str(&secrets.open(sealed).ok()?).ok()?;
        let named: bool = attempt.state.as_bytes().ct_eq(state.as_bytes()).into();
        (attempt.purpose == PURPOSE && named && attempt.expires > chrono::Utc::now().timestamp())
            .then_some(attempt)
    }
}

/// The two endpoints a sign-in needs, as the provider states them, and how
/// its token endpoint takes the client's credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

impl Provider {
    /// Whether the credentials go in the body. HTTP Basic is what OpenID
    /// Connect Discovery reads when a provider lists nothing, and the one a
    /// provider left at its defaults accepts (Authelia refuses a secret in the
    /// body), so the body is used only where Basic is not offered.
    fn takes_the_secret_in_the_body(&self) -> bool {
        let offers =
            |method: &str| self.token_endpoint_auth_methods_supported.iter().any(|m| m == method);
        offers("client_secret_post") && !offers("client_secret_basic")
    }
}

/// How long the provider's description is trusted without asking again.
///
/// A static document by specification, and the endpoint that reads it is
/// public: refetched per call, one cheap inbound request becomes one outbound
/// request against the operator's own identity provider, which is an amplifier
/// pointed at the thing they log in with. Fifteen minutes is short enough that
/// a provider moving an endpoint is picked up within a deploy window.
const DISCOVERY_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Read the provider's own description of itself.
///
/// Discovery rather than four configured endpoints: a provider states its own
/// addresses, and copying them by hand is four more values to keep in step with
/// somebody else's deployment.
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
    /// The attempt, which the browser that left carries back in a cookie:
    /// without it, any browser presenting a known `code` and `state` pair is
    /// signed in as whoever started the attempt.
    pub attempt: Attempt,
}

/// Begin a sign-in: make the attempt and build the provider's URL. `again`
/// asks the provider to sign the person in again whatever session it holds,
/// for what a session older than a few minutes may not do.
pub async fn start(state: &AppState, again: bool, return_to: Option<String>) -> AppResult<Start> {
    let provider = discover(state).await?;
    let client_id = require(&state.config.oidc_client_id, "ROUTARR_OIDC_CLIENT_ID")?;
    let redirect_uri = require(&state.config.oidc_redirect_url, "ROUTARR_OIDC_REDIRECT_URL")?;

    let attempt = Attempt {
        purpose: PURPOSE.to_string(),
        state: crate::crypto::generate_secret()?,
        nonce: crate::crypto::generate_secret()?,
        verifier: crate::crypto::generate_secret()?,
        expires: chrono::Utc::now().timestamp() + FLOW_MINUTES * 60,
        again,
        return_to,
    };

    // S256, never `plain`: the challenge is what travels through the browser,
    // and a plain one is the verifier itself.
    let challenge = B64URL.encode(Sha256::digest(attempt.verifier.as_bytes()));
    // A group is read from a claim most providers send only for this scope.
    let scope = if state.config.oidc_allowed_groups.is_empty() {
        "openid profile"
    } else {
        "openid profile groups"
    };

    let mut query = vec![
        ("response_type", "code"),
        ("scope", scope),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("state", &attempt.state),
        ("nonce", &attempt.nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    // Both: OpenID Connect Core gives `max_age=0` the meaning, and a provider
    // that reads only one reads `prompt`.
    if again {
        query.extend([("max_age", "0"), ("prompt", "login")]);
    }
    let url =
        reqwest::Url::parse_with_params(&provider.authorization_endpoint, query).map_err(|e| {
            AppError::Config(format!("the provider's authorization URL is unusable: {e}"))
        })?;

    Ok(Start { redirect_to: url.to_string(), attempt })
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
    /// The client the token was issued at the request of, when it names one.
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    /// When the person last signed in at the provider, in Unix seconds.
    #[serde(default)]
    auth_time: Option<i64>,
    /// The groups claim is named by the operator, so every other claim is kept.
    #[serde(flatten)]
    others: serde_json::Map<String, serde_json::Value>,
}

impl Claims {
    /// The groups the token names under `claim`: a list of names, or one.
    fn groups(&self, claim: &str) -> Vec<&str> {
        match self.others.get(claim) {
            Some(serde_json::Value::Array(names)) => {
                names.iter().filter_map(serde_json::Value::as_str).collect()
            }
            Some(serde_json::Value::String(name)) => vec![name.as_str()],
            _ => Vec::new(),
        }
    }
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

/// Finish a sign-in and return the subject to record for the person the
/// provider vouched for, if the operator lets them in.
pub async fn finish(state: &AppState, code: &str, attempt: Attempt) -> AppResult<String> {
    let Attempt { nonce, verifier, again, .. } = attempt;
    let provider = discover(state).await?;
    let client_id = require(&state.config.oidc_client_id, "ROUTARR_OIDC_CLIENT_ID")?;
    let client_secret = require(&state.config.oidc_client_secret, "ROUTARR_OIDC_CLIENT_SECRET")?;
    let redirect_uri = require(&state.config.oidc_redirect_url, "ROUTARR_OIDC_REDIRECT_URL")?;

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", &verifier),
    ];
    let request = state.http.post(&provider.token_endpoint);
    let request = if provider.takes_the_secret_in_the_body() {
        form.extend([("client_id", client_id), ("client_secret", client_secret)]);
        request
    } else {
        // Each form-encoded before the two are joined (RFC 6749 §2.3.1): a
        // secret holding a colon would otherwise split in the wrong place.
        let encoded =
            |value: &str| form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
        request.basic_auth(encoded(client_id), Some(encoded(client_secret)))
    };
    let tokens: TokenResponse = crate::integrations::send_json("oidc", request.form(&form)).await?;

    let claims = decode_claims(&tokens.id_token)?;

    if claims.iss.trim_end_matches('/') != provider.issuer.trim_end_matches('/') {
        return Err(AppError::BadRequest("The token names another issuer.".into()));
    }
    // An audience this client shares with others is accepted only when the
    // client that asked for the token, if the token names one, is this one
    // (OpenID Connect Core, section 3.1.3.7).
    if !claims.aud.contains(client_id) || claims.azp.as_deref().is_some_and(|azp| azp != client_id)
    {
        return Err(AppError::BadRequest("The token was issued for another client.".into()));
    }
    if claims.exp <= chrono::Utc::now().timestamp() {
        return Err(AppError::BadRequest("The token has expired.".into()));
    }
    // The one claim that ties this token to the attempt this browser started.
    if claims.nonce.as_deref() != Some(nonce.as_str()) {
        return Err(AppError::BadRequest("The token belongs to another sign-in.".into()));
    }
    // Asked to sign the person in again, the provider has to say it did, and
    // just now: a session it kept would otherwise pass for a fresh sign-in.
    let recent = super::accounts::RECENT_SIGN_IN_SECONDS;
    if again && !claims.auth_time.is_some_and(|at| chrono::Utc::now().timestamp() - at <= recent) {
        return Err(AppError::Forbidden(
            "the provider did not sign the person in again, or did not say when".into(),
        ));
    }

    let config = &state.config;
    let allowed = config.oidc_allow_anyone
        || config.oidc_allowed_subjects.contains(&claims.sub)
        || claims
            .groups(&config.oidc_groups_claim)
            .iter()
            .any(|group| config.oidc_allowed_groups.iter().any(|allowed| allowed == group));
    if !allowed {
        return Err(AppError::Forbidden(format!(
            "the provider signed in '{}', who is neither an allowed subject nor in an allowed \
             group",
            claims.sub
        )));
    }

    Ok(recorded_subject(claims.preferred_username, claims.sub))
}

/// The name a session and every write it makes record. The `sub` is what the
/// provider guarantees unique and constant, and a `preferred_username` is
/// neither (OpenID Connect Core §5.7): many providers let a person change it.
/// So a name shown beside the `sub`, never in place of it.
fn recorded_subject(name: Option<String>, sub: String) -> String {
    match name.filter(|name| !name.is_empty() && *name != sub) {
        Some(name) => format!("{name} ({sub})"),
        None => sub,
    }
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

    /// A name a person can change is shown beside the `sub`, which they
    /// cannot, and never in its place.
    #[test]
    fn the_recorded_subject_carries_the_sub_beside_any_name() {
        assert_eq!(recorded_subject(Some("alice".into()), "u-42".into()), "alice (u-42)");
        assert_eq!(recorded_subject(None, "u-42".into()), "u-42");
        assert_eq!(recorded_subject(Some(String::new()), "u-42".into()), "u-42");
        assert_eq!(recorded_subject(Some("u-42".into()), "u-42".into()), "u-42");
    }

    /// An attempt opens within its ten minutes, for the state it names, from
    /// a value sealed as one and nothing else: a cookie written in clear
    /// would let its writer choose the nonce and the verifier.
    #[test]
    fn an_attempt_opens_only_while_live_for_its_state_and_sealed_as_one() {
        use crate::crypto::SecretBox;
        let secrets = SecretBox::load(
            Some("dGVzdC1rZXktMzItYnl0ZXMtZm9yLXVuaXQtdGVzdCE="),
            None,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .unwrap();
        let now = chrono::Utc::now().timestamp();
        let state = "s".repeat(64);
        let attempt = |expires: i64, purpose: &str| Attempt {
            purpose: purpose.into(),
            state: state.clone(),
            nonce: "n".into(),
            verifier: "v".into(),
            expires,
            again: false,
            return_to: None,
        };

        let live = attempt(now + 600, PURPOSE).sealed(&secrets).unwrap();
        assert!(Attempt::opened(&secrets, &live, &state).is_some());
        assert!(Attempt::opened(&secrets, &live, &"t".repeat(64)).is_none(), "another state");
        let stale = attempt(now - 1, PURPOSE).sealed(&secrets).unwrap();
        assert!(Attempt::opened(&secrets, &stale, &state).is_none(), "past its ten minutes");
        let other = attempt(now + 600, "notification").sealed(&secrets).unwrap();
        assert!(Attempt::opened(&secrets, &other, &state).is_none(), "sealed for another use");
        let clear = serde_json::to_string(&attempt(now + 600, PURPOSE)).unwrap();
        assert!(Attempt::opened(&secrets, &clear, &state).is_none(), "written in clear");
    }

    /// A list of groups, or one written as a string, under the claim the
    /// operator named.
    #[test]
    fn groups_are_read_under_the_claim_named() {
        let claims = decode_claims(&jwt(serde_json::json!({
            "iss": "https://idp", "sub": "u1", "exp": 1,
            "groups": ["media", "admins"], "role": "family", "count": 3
        })))
        .unwrap();
        assert_eq!(claims.groups("groups"), ["media", "admins"]);
        assert_eq!(claims.groups("role"), ["family"]);
        assert!(claims.groups("count").is_empty());
        assert!(claims.groups("absent").is_empty());
    }
}
