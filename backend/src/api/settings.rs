//! Global settings.

use super::Json;
use axum::extract::State;
use serde::Deserialize;
use std::collections::HashMap;

use crate::error::{AppError, AppResult};
use crate::services::settings::{check, is_secret};
use crate::state::AppState;

pub async fn get_all(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&state.pool)
            .await?;

    let mut settings = serde_json::Map::new();
    for (key, value) in rows {
        // A sealed value never leaves the process. The screen needs to know
        // whether one is set, not what it is, so it gets a boolean under a
        // separate key and the key itself is left out: an empty string written
        // back removes the credential, so a client saving what it read would
        // erase every one.
        if is_secret(&key) {
            let configured = !value.trim().is_empty();
            settings.insert(format!("{key}_configured"), serde_json::Value::Bool(configured));
            continue;
        }
        settings.insert(key, serde_json::Value::String(value));
    }

    // A key never stored is left out rather than answered as "": an empty
    // value is refused for several keys, `onboarding` among them, so a client
    // writing back what it read would be refused, and the screen, which fills
    // what is missing with its own fallback, would take "" for a choice.

    Ok(Json(serde_json::Value::Object(settings)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub settings: HashMap<String, String>,
}

pub async fn update(
    State(state): State<AppState>,
    Json(req): Json<UpdateSettingsRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let categories: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?;

    // Validate everything before writing anything: a half-applied settings save
    // is worse than a rejected one.
    let localizer = state.localizer().await;
    for (key, value) in &req.settings {
        check(key, value, &categories, &localizer)?;
    }
    if let Some(list) = req.settings.get("metadata_providers") {
        refuse_a_source_without_its_key(&state, list, &req.settings).await?;
    }

    let mut tx = state.pool.begin().await?;
    for (key, value) in &req.settings {
        // Sealed here rather than in the client, so a value reaching the table
        // in plaintext is impossible whatever the caller sent. An empty value
        // stays empty: sealing nothing would store an opaque blob meaning "unset".
        let stored = if is_secret(key) && !value.trim().is_empty() {
            state.secrets.seal(value.trim())?
        } else {
            value.trim().to_string()
        };
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(&stored)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // Pruning only after a backup is taken would leave every archive above the
    // new retention on disk, and on screen, until the next scheduled run.
    // The number takes effect when it is set.
    if req.settings.contains_key("backup_retention_count")
        && let Err(e) = crate::services::backup::prune(&state).await
    {
        tracing::warn!("Could not prune backups after the retention changed: {e}");
    }

    Ok(Json(serde_json::json!({ "updated": req.settings.len() })))
}

/// Refuse to add a source that needs a key and has none: not in this save, not
/// stored, not in the environment. It could answer nothing, and the health
/// page could only warn about it.
async fn refuse_a_source_without_its_key(
    state: &AppState,
    list: &str,
    saving: &HashMap<String, String>,
) -> AppResult<()> {
    match sources_without_their_key(state, list, saving).await.first() {
        Some(id) => Err(AppError::BadRequest(format!(
            "'metadata_providers': '{id}' needs an API key before it can be enabled"
        ))),
        None => Ok(()),
    }
}

/// The sources `list` adds that need a key and have none. A source already
/// listed stays: the Settings screen sends the whole list on every save, and
/// refusing the list as it stands would refuse every setting until the source
/// is removed.
pub(crate) async fn sources_without_their_key(
    state: &AppState,
    list: &str,
    saving: &HashMap<String, String>,
) -> Vec<String> {
    let settings = state.settings().await;
    let listed: Vec<&str> =
        AppState::metadata_order_from(&settings).iter().map(|source| source.id).collect();
    list.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty() && !listed.contains(id))
        .filter(|id| {
            crate::services::metadata::info(id).is_some_and(|source| {
                let typed =
                    saving.get(&format!("{id}_api_key")).is_some_and(|key| !key.trim().is_empty());
                source.needs_key
                    && !typed
                    && state.provider_key_from(&settings, source.id).is_none()
            })
        })
        .map(str::to_string)
        .collect()
}
