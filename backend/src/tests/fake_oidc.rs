//! An OpenID Connect provider on an ephemeral port.
//!
//! Enough of one to exercise the whole flow: discovery, an authorization
//! endpoint that issues a code for what the browser carried, then a token
//! endpoint that spends the code only for the client, the redirect address and
//! the PKCE verifier it was issued to, and hands back an ID token carrying the
//! nonce asked for. It records the form and the credentials the client sent,
//! so a test can assert on what actually went over the wire rather than on
//! what the code meant to send.
//!
//! Like a provider left at its defaults (Authelia's `client_secret_basic`), it
//! takes the client's credentials as HTTP Basic and refuses them in the body,
//! unless it was told to advertise `client_secret_post` alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct FakeState {
    issuer: String,
    /// What the ID token will claim. A test bends these to check a refusal.
    claims: Arc<Mutex<serde_json::Value>>,
    /// Every token request, in order.
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    discoveries: Arc<AtomicUsize>,
    endpoints: Arc<Mutex<Option<String>>>,
    /// Where the discovery document places the authorization endpoint alone.
    authorization_at: Arc<Mutex<Option<String>>>,
    /// The issuer the discovery document names, when not its own.
    named_issuer: Arc<Mutex<Option<String>>>,
    /// Whether the provider advertises `client_secret_post` alone.
    post_only: Arc<std::sync::atomic::AtomicBool>,
    /// The codes issued and not yet spent, each with what it was issued for.
    issued: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

/// One token request: its form, and the client id and secret of its HTTP
/// Basic header, decoded as RFC 6749 §2.3.1 encodes them.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub form: HashMap<String, String>,
    pub basic: Option<(String, String)>,
}

pub struct FakeOidc {
    pub issuer: String,
    claims: Arc<Mutex<serde_json::Value>>,
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    post_only: Arc<std::sync::atomic::AtomicBool>,
    /// How many times the discovery document was asked for.
    ///
    /// `/auth/oidc/start` is public and unauthenticated, so one request per
    /// call makes Routarr an amplifier pointed at the operator's own identity
    /// provider. Counting is the only way to see a cache working.
    discoveries: Arc<AtomicUsize>,
    /// Where the discovery document places the two endpoints, when not at the
    /// issuer itself.
    endpoints: Arc<Mutex<Option<String>>>,
    authorization_at: Arc<Mutex<Option<String>>>,
    named_issuer: Arc<Mutex<Option<String>>>,
    /// Serving until it is dropped.
    _server: super::Served,
}

impl FakeOidc {
    pub async fn start() -> Self {
        // Bound first: the provider names itself, its issuer, in what it serves.
        let (listener, issuer) = super::listen().await;

        let claims = Arc::new(Mutex::new(serde_json::json!({})));
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let discoveries = Arc::new(AtomicUsize::new(0));
        let endpoints = Arc::new(Mutex::new(None));
        let authorization_at = Arc::new(Mutex::new(None));
        let named_issuer = Arc::new(Mutex::new(None));
        let post_only = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state = FakeState {
            issuer: issuer.clone(),
            claims: Arc::clone(&claims),
            exchanges: Arc::clone(&exchanges),
            discoveries: Arc::clone(&discoveries),
            endpoints: Arc::clone(&endpoints),
            authorization_at: Arc::clone(&authorization_at),
            named_issuer: Arc::clone(&named_issuer),
            post_only: Arc::clone(&post_only),
            issued: Arc::default(),
        };

        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(state);

        let server = super::serve_on(listener, app);

        Self {
            issuer,
            claims,
            exchanges,
            post_only,
            discoveries,
            endpoints,
            authorization_at,
            named_issuer,
            _server: server,
        }
    }

    /// Describe the authorization endpoint alone as living at `base`.
    pub fn advertise_authorization_at(&self, base: &str) {
        *self.authorization_at.lock().expect("authorization") = Some(base.to_string());
    }

    /// Describe the provider as `issuer`, which is not where it answers.
    pub fn call_itself(&self, issuer: &str) {
        *self.named_issuer.lock().expect("issuer") = Some(issuer.to_string());
    }

    /// From now on the provider advertises `client_secret_post` alone, takes
    /// the credentials in the body and refuses them as HTTP Basic.
    pub fn accept_only_post(&self) {
        self.post_only.store(true, Ordering::SeqCst);
    }

    /// Describe the endpoints as living at `base` rather than at the issuer, as
    /// a provider whose token endpoint is served in the clear does.
    pub fn advertise_endpoints_at(&self, base: &str) {
        *self.endpoints.lock().expect("endpoints") = Some(base.to_string());
    }

    /// How many times the provider was asked to describe itself.
    pub fn discoveries(&self) -> usize {
        self.discoveries.load(Ordering::SeqCst)
    }

    /// What the next ID token will claim, merged over the sound defaults.
    pub fn will_claim(&self, overrides: serde_json::Value) {
        *self.claims.lock().expect("claims") = overrides;
    }

    pub fn exchanges(&self) -> Vec<Exchange> {
        self.exchanges.lock().expect("exchanges").clone()
    }
}

async fn discovery(State(state): State<FakeState>) -> Json<serde_json::Value> {
    state.discoveries.fetch_add(1, Ordering::SeqCst);
    let base = state.endpoints.lock().expect("endpoints").clone().unwrap_or(state.issuer.clone());
    let authorization =
        state.authorization_at.lock().expect("authorization").clone().unwrap_or(base.clone());
    let issuer = state.named_issuer.lock().expect("issuer").clone().unwrap_or(state.issuer.clone());
    let mut document = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{authorization}/authorize"),
        "token_endpoint": format!("{base}/token"),
    });
    // Left out otherwise, as many providers do: OpenID Connect Discovery then
    // reads `client_secret_basic`.
    if state.post_only.load(Ordering::SeqCst) {
        document["token_endpoint_auth_methods_supported"] =
            serde_json::json!(["client_secret_post"]);
    }
    Json(document)
}

/// Issue a code for what the browser carried, and send it back to the
/// redirect address, as a provider does once the person has signed in. It
/// refuses what Keycloak and Authelia refuse: another flow than the code
/// flow, a plain PKCE challenge, or no `openid` scope.
async fn authorize(
    State(state): State<FakeState>,
    Query(asked): Query<HashMap<String, String>>,
) -> Response {
    let wanted = |name: &str, value: &str| asked.get(name).map(String::as_str) == Some(value);
    let scoped = asked.get("scope").is_some_and(|scope| scope.split(' ').any(|s| s == "openid"));
    let (Some(redirect_uri), Some(sent_state)) = (asked.get("redirect_uri"), asked.get("state"))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !wanted("response_type", "code")
        || !wanted("code_challenge_method", "S256")
        || !scoped
        || !asked.contains_key("client_id")
        || !asked.contains_key("code_challenge")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let code = uuid::Uuid::new_v4().simple().to_string();
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &code)
        .append_pair("state", sent_state)
        .finish();
    let back = format!("{redirect_uri}?{query}");
    state.issued.lock().expect("issued").insert(code, asked);
    (StatusCode::FOUND, [(header::LOCATION, back)]).into_response()
}

/// The client id and secret of a `Basic` header, each form-decoded.
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    use base64::engine::general_purpose::STANDARD;

    let encoded = headers.get("authorization")?.to_str().ok()?.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    let form_decoded = |part: &str| {
        form_urlencoded::parse(format!("v={part}").as_bytes()).next().map(|(_, v)| v.into_owned())
    };
    Some((form_decoded(id)?, form_decoded(secret)?))
}

async fn token(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // The one grant this flow uses: a provider refuses any other.
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unsupported_grant_type" })),
        ));
    }
    let basic = basic_credentials(&headers);
    state
        .exchanges
        .lock()
        .expect("exchanges")
        .push(Exchange { form: form.clone(), basic: basic.clone() });
    let post_only = state.post_only.load(Ordering::SeqCst);
    let client = match (&basic, post_only) {
        (Some((id, _)), false) if !form.contains_key("client_secret") => id.clone(),
        (None, true) if form.contains_key("client_secret") => {
            form.get("client_id").cloned().unwrap_or_default()
        }
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid_client" })),
            ));
        }
    };

    // Single-use, as RFC 6749 §4.1.2 has every provider spend a code, and
    // only by the client, the redirect address and the PKCE verifier it was
    // issued to (RFC 7636 §4.6).
    let code = form.get("code").map(String::as_str).unwrap_or_default();
    let asked = state.issued.lock().expect("issued").remove(code);
    let verifier = form.get("code_verifier").map(String::as_str).unwrap_or_default();
    let paired = asked.as_ref().is_some_and(|asked| {
        asked.get("client_id") == Some(&client)
            && asked.get("redirect_uri") == form.get("redirect_uri")
            && asked.get("code_challenge")
                == Some(&B64URL.encode(Sha256::digest(verifier.as_bytes())))
    });
    let Some(asked) = asked.filter(|_| paired) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        ));
    };

    let mut claims = serde_json::json!({
        "iss": state.issuer,
        "sub": "user-42",
        "aud": client,
        "exp": chrono::Utc::now().timestamp() + 300,
        "nonce": asked.get("nonce"),
    });
    if let (Some(base), Some(overrides)) =
        (claims.as_object_mut(), state.claims.lock().expect("claims").as_object())
    {
        for (key, value) in overrides {
            base.insert(key.clone(), value.clone());
        }
    }

    let payload = B64URL.encode(claims.to_string());
    Ok(Json(serde_json::json!({
        "access_token": "at",
        "token_type": "Bearer",
        "id_token": format!("header.{payload}.signature"),
    })))
}
