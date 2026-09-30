//! Arr instance management.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::api::auth::Identity;
use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::detached;
use crate::localization::Localizer;
use crate::models::*;
use crate::services::connection::{self, Cause};
use crate::services::sync;
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
    // Stored encrypted: the plaintext never touches the database.
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
    let localizer = state.localizer().await;
    let base_url = validate(&req, &localizer)?;
    let existing = state.instance(&id).await?;
    let base_url = keep_saved_credentials(base_url, &existing.base_url, &localizer)?;

    // An empty api_key means "keep the current one": the UI only ever shows a
    // masked value, so re-submitting the form must not wipe the secret. Kept
    // for the address it was saved with only, or the next sync carries it to
    // whatever address was typed.
    let api_key = if req.api_key.trim().is_empty() {
        saved_for_this_address(
            &existing.base_url,
            &base_url,
            "InstanceKeyForNewAddress",
            &localizer,
        )?;
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
    // One transaction: the proposals go with the instance or not at all.
    let mut tx = state.pool.begin().await?;
    crate::services::routing::supersede_instance_decisions(&mut tx, &id).await?;
    let result =
        sqlx::query("DELETE FROM instances WHERE id = ?").bind(&id).execute(&mut *tx).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Instance {id} not found")));
    }
    tx.commit().await?;

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
    let saved = match &req.id {
        Some(id) => Some(state.instance(id).await?),
        None => None,
    };
    let base_url = match &saved {
        Some(saved) => keep_saved_credentials(base_url, &saved.base_url, &localizer)?,
        None => base_url,
    };
    let api_key = match (req.api_key.trim(), &saved) {
        ("", Some(saved)) => {
            saved_for_this_address(
                &saved.base_url,
                &base_url,
                "InstanceKeyForNewAddress",
                &localizer,
            )?;
            state.secrets.open(&saved.api_key)?
        }
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
        let sentence = connection::explain(&cause, kind, base_url, localizer);
        return Err(connection::refusal(&cause, sentence));
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
pub async fn sync_all(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> AppResult<Json<Vec<sync::SyncReport>>> {
    let by = identity.attribution();
    let reports = detached(async move { sync::sync_all_instances(&state, &by).await }).await?;
    Ok(Json(reports))
}

pub async fn sync_now(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<sync::SyncReport>> {
    let (task_state, task_id, by) = (state.clone(), id.clone(), identity.attribution());
    let synced = detached(async move { sync::sync_instance(&task_state, &task_id, &by).await });
    let error = match synced.await {
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
    let kind = instance.instance_type.as_str();
    check(&adapter, kind, &instance.base_url, &localizer).await?;
    // The probe passed, so the address answers as the Arr, and blaming it
    // sends the operator to edit an address that is right. What failed is the
    // library listing, the one call large enough to outlast the timeout.
    let service = [("service", connection::service_name(kind))];
    Err(match cause {
        Cause::TimedOut => {
            AppError::UpstreamDown(localizer.translate("InstanceSyncTimedOut", &service))
        }
        Cause::NotTheApi => {
            AppError::UpstreamDown(localizer.translate("InstanceSyncUnreadable", &service))
        }
        other => {
            let sentence = connection::explain(&other, kind, &instance.base_url, &localizer);
            connection::refusal(&other, sentence)
        }
    })
}

/// Refuse to send a stored secret anywhere but the origin it was saved with.
///
/// The API key is the Arr's write credential and a proxy's credentials guard
/// it, and the form shows neither back: left as shown on another address, one
/// would leave for that address unseen. `refusal` names the one to type again.
fn saved_for_this_address(
    saved: &str,
    typed: &str,
    refusal: &str,
    localizer: &Localizer,
) -> AppResult<()> {
    let same = match (reqwest::Url::parse(saved), reqwest::Url::parse(typed)) {
        (Ok(saved), Ok(typed)) => crate::http::stays_on_origin(&saved, &typed),
        _ => false,
    };
    if same {
        return Ok(());
    }
    Err(AppError::BadRequest(
        localizer.translate(refusal, &[("address", &crate::http::masked(typed))]),
    ))
}

/// The address as typed, or with the saved credentials in place of the mask
/// the form was shown. Kept for the address they were saved with only, like
/// the API key.
fn keep_saved_credentials(typed: String, saved: &str, localizer: &Localizer) -> AppResult<String> {
    let Some(restored) = crate::http::with_saved_credentials(&typed, saved) else {
        return Ok(typed);
    };
    saved_for_this_address(saved, &typed, "InstanceCredentialsForNewAddress", localizer)?;
    Ok(restored)
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
