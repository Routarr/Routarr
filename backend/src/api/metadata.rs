//! The metadata sources this build knows about, and a refresh of what they
//! answered.
//!
//! The Settings screen needs the whole catalogue, not just the enabled part:
//! a source can only be added back to the priority list if the interface knows
//! it exists. The order itself is an ordinary setting (`metadata_providers`),
//! saved through `PUT /settings` like everything else.

use super::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::api::auth::Identity;
use crate::api::jobs::{answer, prefers_async};
use crate::error::{AppError, AppResult};
use crate::services::{enrichment, metadata};
use crate::state::AppState;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderDescription {
    pub id: String,
    /// A proper noun, never translated, like "Radarr" and "Sonarr".
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
    /// The media types it answers for, `movie` and `series`: TheTVDB answers
    /// for series alone.
    pub media_types: Vec<&'static str>,
    /// The site its data comes from, which the interface credits and links
    /// to. Null for Radarr and Sonarr.
    pub website: Option<&'static str>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
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
            media_types: provider.media_types.to_vec(),
            website: provider.website,
        })
        .collect();

    Ok(Json(ProvidersResponse { providers, order }))
}

/// What to ask the sources again about: one source's answers, one title's,
/// both, or everything when neither is named.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    pub source: Option<String>,
    pub media_id: Option<String>,
}

/// Ask the sources again, as a task. A title is asked at once, its search
/// matches forgotten; a source or everything is marked due and read by a
/// pass, refused while one runs.
pub async fn refresh(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> AppResult<Response> {
    let source = match request.source.as_deref() {
        None => None,
        Some(id) => Some(
            metadata::info(id)
                .filter(|provider| provider.fetched)
                .ok_or_else(|| {
                    AppError::BadRequest(format!("'{id}' is not a source Routarr asks"))
                })?
                .id,
        ),
    };
    let by = identity.attribution();
    let task_state = state.clone();
    match request.media_id {
        Some(id) => {
            let media = crate::api::media::load_media(&state, &id).await?;
            let work =
                async move { enrichment::refresh_title(&task_state, &by, &media, source).await };
            answer(&state, prefers_async(&headers), work).await
        }
        None => {
            let work = async move { enrichment::refresh(&task_state, &by, source).await };
            answer(&state, prefers_async(&headers), work).await
        }
    }
}
