//! Arr instance management.

use super::Json;
use axum::extract::State;

use super::Path;
use axum::http::HeaderMap;
use axum::response::Response;
use uuid::Uuid;

use crate::api::auth::Identity;
use crate::api::jobs::{answer, prefers_async};
use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::{Detail, JobKind};
use crate::localization::Localizer;
use crate::models::*;
use crate::services::connection::{self, Cause};
use crate::services::sync;
use crate::state::AppState;
use serde::{Deserialize, Serialize};

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> AppResult<Json<Vec<InstanceResponse>>> {
    let instances = state.instances(false).await?;
    Ok(Json(instances.into_iter().map(|i| shown(&state, &identity, i)).collect()))
}

/// Fetch one instance, with its API key masked like the list endpoint.
pub async fn get_one(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    Ok(Json(shown(&state, &identity, state.instance(&id).await?)))
}

/// An instance as `identity` may read it. An application key reads neither
/// the webhook's address, whose token lets anyone post events as the Arr, nor
/// any part of the Arr's key.
fn shown(state: &AppState, identity: &Identity, instance: Instance) -> InstanceResponse {
    let response = InstanceResponse::from_instance(instance, &state.config.base_path);
    match identity.application {
        Some(_) => {
            InstanceResponse { webhook_url: None, api_key_masked: String::new(), ..response }
        }
        None => response,
    }
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

    let instance_type = req.instance_type.to_lowercase();
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    sqlx::query(
        "UPDATE instances SET name = ?, instance_type = ?, base_url = ?, api_key = ?,
         enabled = ?, sync_interval_minutes = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(req.name.trim())
    .bind(&instance_type)
    .bind(&base_url)
    .bind(api_key)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    // A pending proposal names the ids of the Arr the instance pointed at, and
    // another Arr gives those ids to other titles. The next sync tells which
    // titles are still the same (`sync::reassigned`), and the next simulation
    // proposes again.
    if base_url != existing.base_url || instance_type != existing.instance_type {
        sqlx::query(
            "UPDATE decisions SET superseded = 1
              WHERE instance_id = ? AND status = 'pending' AND superseded = 0",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(Json(InstanceResponse::from_instance(state.instance(&id).await?, &state.config.base_path)))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // Not under a sync of it: its writes would fail on rows gone, and the
    // failure would be notified for an instance that no longer exists.
    let Some(_sync) = state.jobs.try_lock(&format!("sync:{id}")) else {
        return Err(AppError::Conflict(
            "This instance is being synced. Delete it once the sync has finished.".into(),
        ));
    };
    // One transaction: the proposals go with the instance or not at all.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    crate::services::routing::supersede_instance_decisions(&mut tx, &id).await?;
    let result =
        sqlx::query("DELETE FROM instances WHERE id = ?").bind(&id).execute(&mut *tx).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Instance {id} not found")));
    }

    // Out of every rule's scope too. A rule scoped to this instance alone is
    // switched off: left with no instance its scope would read as every one,
    // and the rule would start routing libraries it was never written for.
    let scoped: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT id, instance_ids, enabled FROM rules
          WHERE instance_ids IS NOT NULL AND EXISTS (SELECT 1 FROM json_each(rules.instance_ids)
                                                      WHERE json_each.value = ?)",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await?;
    let mut switched_off = Vec::new();
    for (rule, ids, enabled) in scoped {
        let left: Vec<String> = serde_json::from_str::<Vec<String>>(&ids)?
            .into_iter()
            .filter(|kept| *kept != id)
            .collect();
        if left.is_empty() {
            sqlx::query("UPDATE rules SET instance_ids = NULL, enabled = 0 WHERE id = ?")
                .bind(&rule)
                .execute(&mut *tx)
                .await?;
            if enabled {
                switched_off.push(rule);
            }
        } else {
            sqlx::query("UPDATE rules SET instance_ids = ? WHERE id = ?")
                .bind(serde_json::to_string(&left)?)
                .bind(&rule)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;

    Ok(Json(serde_json::json!({ "deleted": true, "rules_switched_off": switched_off })))
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

/// Sync every enabled instance, under a task of its own a caller can follow:
/// each instance's sync is a task too, and following the first would answer
/// one report where the call answers one per instance.
pub async fn sync_all(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let (task_state, by) = (state.clone(), identity.attribution());
    let work = async move {
        let state = task_state;
        let instances = state.instances(true).await?.len();
        let mut job = state
            .jobs
            .start(
                JobKind::SyncAll,
                &by,
                None,
                Detail::new("JobDetailSyncingAll").with("count", instances),
            )
            .await?;
        let reports = match sync::sync_all_instances(&state, &by).await {
            Ok(reports) => reports,
            Err(error) => {
                job.fail(&error).await;
                return Err(error);
            }
        };
        let failed = reports.iter().filter(|report| report.error.is_some()).count();
        if failed < reports.len() {
            crate::jobs::scheduler::follow_sync(&state, &by).await;
        }
        job.report(&reports);
        let detail = Detail::new("JobDetailSyncedAll")
            .with("synced", reports.len() - failed)
            .with("failed", failed);
        if failed > 0 && failed == reports.len() {
            job.fail_with(detail).await;
        } else {
            job.succeed(detail).await;
        }
        Ok(reports)
    };
    answer(&state, prefers_async(&headers), work).await
}

pub async fn sync_now(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let (task_state, by) = (state.clone(), identity.attribution());
    let work = async move {
        match sync::sync_instance(&task_state, &id, &by).await {
            Ok(report) => {
                crate::jobs::scheduler::follow_sync(&task_state, &by).await;
                Ok(report)
            }
            Err(error) => Err(explained(&task_state, &id, error).await),
        }
    };
    answer(&state, prefers_async(&headers), work).await
}

/// A failed sync, in the words of what answers at the instance's address,
/// written on its task too: a caller that did not wait reads it there.
async fn explained(state: &AppState, id: &str, error: AppError) -> AppError {
    let Some(cause) = connection::cause_of(&error) else {
        return error;
    };
    let explained = match explain_sync_failure(state, id, cause).await {
        Ok(explained) | Err(explained) => explained,
    };
    if let Some(task) = crate::jobs::registry::announced() {
        let _ = sqlx::query("UPDATE jobs SET error_message = ? WHERE id = ? AND status = 'failed'")
            .bind(explained.public_message())
            .bind(task)
            .execute(&state.pool)
            .await;
    }
    explained
}

async fn explain_sync_failure(state: &AppState, id: &str, cause: Cause) -> AppResult<AppError> {
    // Explained by what answers at the address: a probe tells a wrong type from
    // a wrong port, which the sync's own failure, a 404 on the library, cannot.
    // This is the first failure most people read, since a new instance syncs as
    // its form closes.
    let instance = state.instance(id).await?;
    let localizer = state.localizer().await;
    let adapter = state.adapter(&instance)?;
    let kind = instance.instance_type.as_str();
    check(&adapter, kind, &instance.base_url, &localizer).await?;
    // The probe passed, so the address answers as the Arr, and blaming it
    // sends the operator to edit an address that is right. What failed is the
    // library listing, the one call large enough to outlast the timeout.
    let service = [("service", connection::service_name(kind))];
    Ok(match cause {
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
    // The API's path is appended to the address: after a `?` or a `#` it would
    // land in the query or the fragment, and the key would go to whatever path
    // the address names.
    if base_url.contains(['?', '#']) {
        return Err(AppError::BadRequest(localizer.translate("InstanceUrlPlain", &[])));
    }
    Ok(base_url)
}
