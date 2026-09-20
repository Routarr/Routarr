//! The metadata sources this build knows about.
//!
//! The Settings screen needs the whole catalogue, not just the enabled part:
//! a source can only be added back to the priority list if the interface knows
//! it exists. The order itself is an ordinary setting (`metadata_providers`),
//! saved through `PUT /settings` like everything else.

use super::Json;
use axum::extract::State;
use serde::Serialize;

use crate::error::AppResult;
use crate::services::metadata;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ProviderDescription {
    pub id: String,
    /// A proper noun, never translated — as with "Radarr" and "Sonarr".
    pub display_name: String,
    /// False for a source whose data arrives with the library sync.
    pub fetched: bool,
    pub needs_key: bool,
    /// The environment variable its credential is read from, so the interface
    /// can name it instead of saying "a key is missing".
    pub key_env: Option<&'static str>,
    /// Whether it can answer today: it needs no key, or its key is set.
    pub configured: bool,
    /// The fields it can supply, so the interface can say what enabling it buys.
    pub fields: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderDescription>,
    /// Enabled sources, highest priority first. Everything not listed is off.
    pub order: Vec<String>,
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<ProvidersResponse>> {
    let settings = state.settings().await;
    let order = AppState::metadata_order_from(&settings).iter().map(|p| p.id.to_string()).collect();
    let keys = state.provider_keys_from(&settings);
    let providers = metadata::PROVIDERS
        .iter()
        .map(|provider| ProviderDescription {
            id: provider.id.to_string(),
            display_name: provider.display_name.to_string(),
            fetched: provider.fetched,
            needs_key: provider.needs_key,
            key_env: provider.key_env,
            configured: metadata::is_usable(provider, &keys),
            fields: provider.fields.iter().map(|f| f.as_str()).collect(),
        })
        .collect();

    Ok(Json(ProvidersResponse { providers, order }))
}
