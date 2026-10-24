//! OpenID Connect, authorization code flow with PKCE.
//!
//! Full access for the people the operator names, by their `sub` or a group the provider puts
//! them in (`ROUTARR_OIDC_ALLOWED_*`), read access for those the read lists name
//! (`ROUTARR_OIDC_READ_*`). Routarr keeps no user table: the subject is stored beside every
//! write it causes.
//!
//! An attempt keeps nothing on the server. Its `state`, nonce, PKCE verifier
//! and expiry travel in a cookie sealed with the master key, so the public
//! route that starts one writes nothing, and a flood of starts evicts nobody's
//! attempt. A code presented twice is refused by the provider, which spends it
//! at the first exchange (RFC 6749 §4.1.2), and the callback clears the cookie.
//!
//! The ID token's signature is not verified. A confidential client receives the token from the
//! token endpoint over TLS, in answer to its own request authenticated with the client secret,
//! and OpenID Connect Core §3.1.3.7 lets "the TLS server validation" stand in for the signature
//! then. The issuer, audience, expiry and nonce are still checked.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How long a sign-in attempt may sit unfinished, long enough to type a password and answer a
/// second factor.
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

/// The longest screen a sign-in comes back to, far past what the interface sends: it is sealed
/// into the attempt's cookie, which a page of another site can otherwise make as large as a
/// browser keeps.
const SCREEN_LENGTH: usize = 512;

/// A screen a sign-in may land on: a path under the mount point, and nothing
/// that leaves it, which a dot segment would.
pub fn screen(path: &str) -> Option<String> {
    let path = path.trim();
    let dot = |segment: &str| {
        matches!(
            segment.to_ascii_lowercase().as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
    };
    let inside = path.starts_with('/')
        && path.len() <= SCREEN_LENGTH
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.contains("://")
        && path.chars().all(|c| c.is_ascii_graphic())
        && !path.split(['/', '?', '#']).any(dot);
    inside.then(|| path.to_string())
}

impl Attempt {
    /// The attempt sealed for its cookie.
    pub fn sealed(&self, secrets: &crate::crypto::SecretBox) -> AppResult<String> {
        secrets.seal(&serde_json::to_string(self)?)
    }

    /// Until when the attempt a cookie carries is live, in Unix seconds, when it is one.
    pub fn expiry(secrets: &crate::crypto::SecretBox, sealed: &str) -> Option<i64> {
        if !crate::crypto::SecretBox::is_sealed(sealed) {
            return None;
        }
        let attempt: Self = serde_json::from_str(&secrets.open(sealed).ok()?).ok()?;
        (attempt.purpose == PURPOSE).then_some(attempt.expires)
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

/// How long the provider's description is trusted without asking again. The route that reads it
/// is public: refetched per call, each anonymous request would become one against the identity
/// provider, which rate-limits by address and would lock the operator out.
const DISCOVERY_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How long a provider that could not be read is answered from that failure. A provider down
/// is when browsers retry, and each start asking it again would be the flood the cache bounds.
const FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// The provider's description as last read, or why it could not be, and until when either is
/// kept.
pub type Described = Option<(Result<Provider, String>, std::time::Instant)>;

/// Read the provider's own description of itself (OpenID Connect Discovery). Starts that find
/// nothing kept wait for the one reading it, and a failure is logged once per [`FAILURE_TTL`].
pub async fn discover(state: &AppState) -> AppResult<Provider> {
    let issuer = state
        .config
        .oidc_issuer
        .as_deref()
        .ok_or_else(|| AppError::Config("ROUTARR_OIDC_ISSUER is not set".into()))?;

    // Read under a shared lock, so concurrent sign-ins do not queue behind each
    // other for a document they all already have.
    if let Some(kept) = kept(&*state.oidc_provider.read().await) {
        return kept;
    }
    let mut described = state.oidc_provider.write().await;
    if let Some(kept) = kept(&described) {
        return kept;
    }
    let read = describe(state, issuer).await;
    let kept = match &read {
        Ok(provider) => (Ok(provider.clone()), DISCOVERY_TTL),
        Err(e) => {
            tracing::error!("The OpenID Connect provider could not be read: {e}");
            (Err(e.to_string()), FAILURE_TTL)
        }
    };
    *described = Some((kept.0, std::time::Instant::now() + kept.1));
    read
}

/// What `described` still answers, if it is fresh.
fn kept(described: &Described) -> Option<AppResult<Provider>> {
    let (answer, fresh_until) = described.as_ref()?;
    (std::time::Instant::now() < *fresh_until).then(|| answer.clone().map_err(AppError::Config))
}

async fn describe(state: &AppState, issuer: &str) -> AppResult<Provider> {
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
    // The flow trusts the channel in place of a token signature (see the module doc), so an
    // endpoint served in the clear is refused, as the issuer is at startup.
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
    Ok(provider)
}

/// Everything the browser has to be sent to, for one attempt.
pub struct Start {
    pub redirect_to: String,
    /// The attempt, which the browser that left carries back in its cookie.
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
    let groups_named =
        !state.config.oidc_allowed_groups.is_empty() || !state.config.oidc_read_groups.is_empty();
    let scope = if !groups_named { "openid profile" } else { "openid profile groups" };

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
    /// A NumericDate, which may carry a fraction (RFC 7519, section 2).
    exp: f64,
    #[serde(default)]
    nonce: Option<String>,
    /// The client the token was issued at the request of, when it names one.
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    /// When the person last signed in at the provider, in Unix seconds.
    #[serde(default)]
    auth_time: Option<f64>,
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
/// provider vouched for, and how far they reach, if the operator lets them in.
pub async fn finish(
    state: &AppState,
    code: &str,
    attempt: Attempt,
) -> AppResult<(String, super::accounts::Reach)> {
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
    let now = chrono::Utc::now().timestamp() as f64;
    if claims.exp <= now {
        return Err(AppError::BadRequest("The token has expired.".into()));
    }
    // The one claim that ties this token to the attempt this browser started.
    if claims.nonce.as_deref() != Some(nonce.as_str()) {
        return Err(AppError::BadRequest("The token belongs to another sign-in.".into()));
    }
    // Asked to sign the person in again, the provider has to say it did, and
    // just now: a session it kept would otherwise pass for a fresh sign-in.
    let recent = super::accounts::RECENT_SIGN_IN_SECONDS as f64;
    if again && !claims.auth_time.is_some_and(|at| now - at <= recent) {
        return Err(AppError::Forbidden(
            "the provider did not sign the person in again, or did not say when".into(),
        ));
    }

    let reach = reach_of(&state.config, &claims)?;
    Ok((recorded_subject(claims.preferred_username, claims.sub), reach))
}

/// Full access for a person the full lists name, whatever the read lists say, the read scope
/// for one only those name, and a refusal for anyone else.
fn reach_of(config: &crate::config::Config, claims: &Claims) -> AppResult<super::accounts::Reach> {
    use super::accounts::Reach;
    let groups = claims.groups(&config.oidc_groups_claim);
    let named = |subjects: &[String], named_groups: &[String]| {
        subjects.contains(&claims.sub)
            || groups.iter().any(|group| named_groups.iter().any(|named| named == group))
    };
    if config.oidc_allow_anyone || named(&config.oidc_allowed_subjects, &config.oidc_allowed_groups)
    {
        return Ok(Reach::Full);
    }
    if named(&config.oidc_read_subjects, &config.oidc_read_groups) {
        return Ok(Reach::Read);
    }
    let groups_named =
        !config.oidc_allowed_groups.is_empty() || !config.oidc_read_groups.is_empty();
    // The usual mistake: groups sent in userinfo alone, or by no mapper at all.
    if groups_named && !claims.others.contains_key(&config.oidc_groups_claim) {
        return Err(AppError::Forbidden(format!(
            "the provider signed in '{}', whose ID token carries no '{}' claim, so no group can \
             let them in. Have the provider put the groups in the ID token",
            claims.sub, config.oidc_groups_claim
        )));
    }
    Err(AppError::Forbidden(format!(
        "the provider signed in '{}', who is neither an allowed subject nor in an allowed group",
        claims.sub
    )))
}

/// The name a session and every write it makes record: the `sub`, which the provider keeps
/// unique and constant, with a `preferred_username` beside it, which a person may change
/// (OpenID Connect Core §5.7).
fn recorded_subject(name: Option<String>, sub: String) -> String {
    match name.filter(|name| !name.is_empty() && *name != sub) {
        Some(name) => format!("{name} ({sub})"),
        None => sub,
    }
}

/// The payload of a JWT, without verifying its signature (see the module doc). Anything that
/// is not a JWT is refused.
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
    fn a_date_carrying_a_fraction_is_read() {
        let claims = decode_claims(&jwt(serde_json::json!({
            "iss": "https://idp", "sub": "u1", "exp": 1_700_000_300.5, "auth_time": 1_700_000_000.25
        })))
        .unwrap();
        assert!(claims.exp > 1_700_000_300.0);
        assert!(claims.auth_time.is_some_and(|at| at > 1_700_000_000.0));
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

    #[test]
    fn a_token_that_is_not_one_is_refused() {
        assert!(decode_claims("not a jwt").is_err());
        assert!(decode_claims("header.!!!not base64!!!.sig").is_err());
        assert!(decode_claims(&format!("header.{}.sig", B64URL.encode("not json"))).is_err());
    }

    #[test]
    fn the_recorded_subject_carries_the_sub_beside_any_name() {
        assert_eq!(recorded_subject(Some("alice".into()), "u-42".into()), "alice (u-42)");
        assert_eq!(recorded_subject(None, "u-42".into()), "u-42");
        assert_eq!(recorded_subject(Some(String::new()), "u-42".into()), "u-42");
        assert_eq!(recorded_subject(Some("u-42".into()), "u-42".into()), "u-42");
    }

    /// A cookie written in clear would let its writer choose the nonce and the verifier.
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
