//! An OpenID Connect provider on an ephemeral port.
//!
//! Enough of one to exercise the whole flow: discovery, then a token endpoint
//! that hands back an ID token built from what the request asked for. It
//! records the form the client sent, so a test can assert on the PKCE verifier
//! and the client secret that actually went over the wire rather than on what
//! the code meant to send.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{Form, State};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use tokio::net::TcpListener;

#[derive(Clone)]
struct FakeState {
    issuer: String,
    /// What the ID token will claim. A test bends these to check a refusal.
    claims: Arc<Mutex<serde_json::Value>>,
    /// The form of every token request, in order.
    exchanges: Arc<Mutex<Vec<HashMap<String, String>>>>,
    discoveries: Arc<AtomicUsize>,
    endpoints: Arc<Mutex<Option<String>>>,
}

pub struct FakeOidc {
    pub issuer: String,
    claims: Arc<Mutex<serde_json::Value>>,
    exchanges: Arc<Mutex<Vec<HashMap<String, String>>>>,
    /// How many times the discovery document was asked for.
    ///
    /// `/auth/oidc/start` is public and unauthenticated, so one request per
    /// call makes Routarr an amplifier pointed at the operator's own identity
    /// provider. Counting is the only way to see a cache working.
    discoveries: Arc<AtomicUsize>,
    /// Where the discovery document places the two endpoints, when not at the
    /// issuer itself.
    endpoints: Arc<Mutex<Option<String>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeOidc {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let issuer = format!("http://{addr}");

        let claims = Arc::new(Mutex::new(serde_json::json!({})));
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let discoveries = Arc::new(AtomicUsize::new(0));
        let endpoints = Arc::new(Mutex::new(None));
        let state = FakeState {
            issuer: issuer.clone(),
            claims: Arc::clone(&claims),
            exchanges: Arc::clone(&exchanges),
            discoveries: Arc::clone(&discoveries),
            endpoints: Arc::clone(&endpoints),
        };

        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/token", post(token))
            .with_state(state);

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });

        Self { issuer, claims, exchanges, discoveries, endpoints, shutdown: Some(tx) }
    }

    /// Describe the endpoints as living at `base` rather than at the issuer —
    /// what a provider whose token endpoint is served in the clear looks like.
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

    pub fn exchanges(&self) -> Vec<HashMap<String, String>> {
        self.exchanges.lock().expect("exchanges").clone()
    }
}

impl Drop for FakeOidc {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn discovery(State(state): State<FakeState>) -> Json<serde_json::Value> {
    state.discoveries.fetch_add(1, Ordering::SeqCst);
    let base = state.endpoints.lock().expect("endpoints").clone().unwrap_or(state.issuer.clone());
    Json(serde_json::json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
    }))
}

async fn token(
    State(state): State<FakeState>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    // The nonce travels in the authorization request, which a browser would
    // have carried and which never reaches this endpoint. A test reads it from
    // the flow row it just created and states it through `will_claim`.
    let mut claims = serde_json::json!({
        "iss": state.issuer,
        "sub": "user-42",
        "aud": form.get("client_id").cloned().unwrap_or_default(),
        "exp": chrono::Utc::now().timestamp() + 300,
    });
    if let (Some(base), Some(overrides)) =
        (claims.as_object_mut(), state.claims.lock().expect("claims").as_object())
    {
        for (key, value) in overrides {
            base.insert(key.clone(), value.clone());
        }
    }

    state.exchanges.lock().expect("exchanges").push(form);

    let payload = B64URL.encode(claims.to_string());
    Json(serde_json::json!({
        "access_token": "at",
        "token_type": "Bearer",
        "id_token": format!("header.{payload}.signature"),
    }))
}
