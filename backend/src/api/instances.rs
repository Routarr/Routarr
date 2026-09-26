//! Arr instance management.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::localization::Localizer;
use crate::models::*;
use crate::services::{connection, sync};
use crate::state::AppState;
use serde::{Deserialize, Serialize};

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<InstanceResponse>>> {
    let instances = state.instances(false).await?;
    Ok(Json(
        instances
            .into_iter()
            .map(|i| InstanceResponse::from_instance(i, &state.config.base_path))
            .collect(),
    ))
}

/// Fetch one instance, with its API key masked like the list endpoint.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<Json<InstanceResponse>> {
    let base_url = validate(&req, &state.localizer().await)?;

    let id = Uuid::new_v4().to_string();
    let webhook_token = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
         sync_interval_minutes, webhook_token)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(req.instance_type.to_lowercase())
    .bind(&base_url)
    // Stored encrypted; the plaintext never touches the database.
    .bind(state.secrets.seal(req.api_key.trim())?)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&webhook_token)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<Json<InstanceResponse>> {
    let base_url = validate(&req, &state.localizer().await)?;
    let existing = state.instance(&id).await?;

    // An empty api_key means "keep the current one" — the UI only ever shows a
    // masked value, so re-submitting the form must not wipe the secret.
    let api_key = if req.api_key.trim().is_empty() {
        existing.api_key.clone()
    } else {
        state.secrets.seal(req.api_key.trim())?
    };

    sqlx::query(
        "UPDATE instances SET name = ?, instance_type = ?, base_url = ?, api_key = ?,
         enabled = ?, sync_interval_minutes = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(req.name.trim())
    .bind(req.instance_type.to_lowercase())
    .bind(&base_url)
    .bind(api_key)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&id)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let result =
        sqlx::query("DELETE FROM instances WHERE id = ?").bind(&id).execute(&state.pool).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Instance {id} not found")));
    }

    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// What a connection probe reports. A named struct rather than a `json!` so
/// `check-api-types` can hold it against its TypeScript mirror.
#[derive(Debug, Serialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub version: String,
    pub app_name: Option<String>,
    pub root_folders: usize,
    pub inaccessible_root_folders: usize,
}

pub async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<TestConnectionResponse>> {
    let instance = state.instance(&id).await?;
    let adapter = state.adapter(&instance)?;
    let localizer = state.localizer().await;
    Ok(Json(check(&adapter, &instance.instance_type, &instance.base_url, &localizer).await?))
}

/// Values typed in the instance form, tried before anything is saved.
#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    pub instance_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// The instance being edited, whose stored key a blank one stands for:
    /// the form never shows a stored key back.
    #[serde(default)]
    pub id: Option<String>,
}

pub async fn probe(
    State(state): State<AppState>,
    Json(req): Json<ProbeRequest>,
) -> AppResult<Json<TestConnectionResponse>> {
    let kind = req.instance_type.parse::<InstanceType>().map_err(AppError::BadRequest)?.to_string();
    let localizer = state.localizer().await;
    let base_url = normalize_base_url(&req.base_url, &localizer)?;
    let api_key = match (req.api_key.trim(), &req.id) {
        ("", Some(id)) => state.secrets.open(&state.instance(id).await?.api_key)?,
        (typed, _) => typed.to_string(),
    };
    let adapter = ArrAdapter::new(state.http.clone(), &kind, &base_url, &api_key)?;
    Ok(Json(check(&adapter, &kind, &base_url, &localizer).await?))
}

/// Reach the Arr as a sync would, and say what to change when it does not answer
/// as the type declared.
async fn check(
    adapter: &ArrAdapter,
    kind: &str,
    base_url: &str,
    localizer: &Localizer,
) -> AppResult<TestConnectionResponse> {
    let explain = |error| connection::explained(error, kind, base_url, localizer);
    let status = adapter.test_connection().await.map_err(explain)?;
    if let Some(cause) = connection::wrong_app(kind, status.app_name.as_deref()) {
        return Err(AppError::BadRequest(connection::explain(&cause, kind, base_url, localizer)));
    }
    let root_folders = adapter.get_root_folders().await.map_err(explain)?;

    Ok(TestConnectionResponse {
        success: true,
        version: status.version,
        app_name: status.app_name,
        root_folders: root_folders.len(),
        inaccessible_root_folders: root_folders.iter().filter(|rf| !rf.accessible).count(),
    })
}

/// Sync every enabled instance.
pub async fn sync_all(State(state): State<AppState>) -> AppResult<Json<Vec<sync::SyncReport>>> {
    Ok(Json(sync::sync_all_instances(&state, "manual").await?))
}

pub async fn sync_now(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<sync::SyncReport>> {
    let error = match sync::sync_instance(&state, &id, "manual").await {
        Ok(report) => return Ok(Json(report)),
        Err(error) => error,
    };
    let Some(cause) = connection::cause_of(&error) else {
        return Err(error);
    };
    // Explained by what answers at the address: a probe tells a wrong type from
    // a wrong port, which the sync's own failure, a 404 on the library, cannot.
    // This is the first failure most people read, since a new instance syncs as
    // its form closes.
    let instance = state.instance(&id).await?;
    let localizer = state.localizer().await;
    let adapter = state.adapter(&instance)?;
    check(&adapter, &instance.instance_type, &instance.base_url, &localizer).await?;
    Err(AppError::BadRequest(connection::explain(
        &cause,
        &instance.instance_type,
        &instance.base_url,
        &localizer,
    )))
}

/// Issue a fresh webhook token, invalidating the previous URL.
pub async fn rotate_webhook_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    state.instance(&id).await?;

    sqlx::query(
        "UPDATE instances SET webhook_token = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&id)
    .execute(&state.pool)
    .await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

/// Shared validation for create and update.
///
/// The type comes from a list the interface offers, so its refusal stays in
/// English. The name and the address are typed, and read under their field.
fn validate(req: &CreateInstanceRequest, localizer: &Localizer) -> AppResult<String> {
    req.instance_type.parse::<InstanceType>().map_err(AppError::BadRequest)?;

    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest(localizer.translate("InstanceNameRequired", &[])));
    }

    normalize_base_url(&req.base_url, localizer)
}

fn normalize_base_url(raw: &str, localizer: &Localizer) -> AppResult<String> {
    let base_url = raw.trim().trim_end_matches('/').to_string();
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(AppError::BadRequest(localizer.translate("InstanceUrlScheme", &[])));
    }
    Ok(base_url)
}
