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
use crate::services::{sync, webhook_check};
use crate::state::AppState;
use serde::{Deserialize, Serialize};

pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> AppResult<Json<Vec<InstanceResponse>>> {
    let instances = state.instances(false).await?;
    let localizer = state.localizer().await;
    Ok(Json(instances.into_iter().map(|i| shown(&state, &identity, &localizer, i)).collect()))
}

/// Fetch one instance, with its API key masked like the list endpoint.
pub async fn get_one(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> AppResult<Json<InstanceResponse>> {
    let localizer = state.localizer().await;
    Ok(Json(shown(&state, &identity, &localizer, state.instance(&id).await?)))
}

/// An instance as `identity` may read it. An application key reads neither
/// the webhook's address, whose token lets anyone post events as the Arr, nor
/// any part of the Arr's key.
fn shown(
    state: &AppState,
    identity: &Identity,
    localizer: &Localizer,
    instance: Instance,
) -> InstanceResponse {
    let key_set = key_set(state, &instance);
    let response =
        InstanceResponse::from_instance(instance, &state.config.base_path, localizer, key_set);
    // The webhook address carries a token that posts as the Arr.
    match identity.is_owner() {
        false => InstanceResponse {
            webhook_url: None,
            webhook_path: None,
            webhook_token: None,
            api_key_masked: String::new(),
            ..response
        },
        true => response,
    }
}

/// Whether the Arr's key is stored. A value the master key cannot open is a
/// key all the same, one the health warnings name.
fn key_set(state: &AppState, instance: &Instance) -> bool {
    state.secrets.open(&instance.api_key).map_or(true, |key| !key.is_empty())
}

/// Another instance holds the name, case and spaces aside: a bundle names an
/// instance by it, and two would be one there.
fn name_taken(e: sqlx::Error, name: &str, localizer: &crate::localization::Localizer) -> AppError {
    match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Conflict {
            reason: "name_taken",
            message: localizer.translate("ErrorInstanceNameTaken", &[("name", name)]),
        },
        _ => e.into(),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<super::Created<InstanceResponse>> {
    let localizer = state.localizer().await;
    let base_url = validate(&req.name, &req.instance_type, &req.base_url, &localizer)?;

    let id = Uuid::new_v4().to_string();
    let webhook_token = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
         sync_interval_minutes, webhook_token, auto_apply)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(req.instance_type.to_lowercase())
    .bind(&base_url)
    .bind(state.secrets.seal(req.api_key.trim())?)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(&webhook_token)
    .bind(req.auto_apply)
    .execute(&state.pool)
    .await
    .map_err(|e| name_taken(e, req.name.trim(), &localizer))?;

    let instance = state.instance(&id).await?;
    let key_set = key_set(&state, &instance);
    let created =
        InstanceResponse::from_instance(instance, &state.config.base_path, &localizer, key_set);
    Ok(super::Created::at(&state, format!("/instances/{id}"), created))
}

pub async fn update(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Path(id): Path<String>,
    Json(req): Json<CreateInstanceRequest>,
) -> AppResult<Json<InstanceResponse>> {
    let localizer = state.localizer().await;
    let base_url = validate(&req.name, &req.instance_type, &req.base_url, &localizer)?;
    let existing = state.instance(&id).await?;
    let base_url = keep_saved_credentials(base_url, &existing.base_url, &localizer)?;

    // A blank key keeps the stored one, which the form never shows back, on the address it
    // was saved with only, or the next sync carries it to whatever address was typed.
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
         enabled = ?, sync_interval_minutes = ?, auto_apply = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(req.name.trim())
    .bind(&instance_type)
    .bind(&base_url)
    .bind(api_key)
    .bind(req.enabled)
    .bind(req.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
    .bind(req.auto_apply)
    .bind(&id)
    .execute(&mut *tx)
    .await
    .map_err(|e| name_taken(e, req.name.trim(), &localizer))?;
    // A pending proposal names ids another Arr gives to other titles. The next sync tells
    // which titles stay the same (`sync::reassigned`), and the next simulation proposes again.
    let elsewhere = base_url != existing.base_url || instance_type != existing.instance_type;
    // Switched off, it is neither synced nor routed, so no run would replace them.
    if elsewhere || (existing.enabled && !req.enabled) {
        sqlx::query(
            "UPDATE decisions SET superseded = 1
              WHERE instance_id = ? AND status = 'pending' AND superseded = 0",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    }
    if elsewhere {
        // The stored check describes the Arr at the old address.
        sqlx::query(
            "UPDATE instances SET webhook_check = NULL, webhook_checked_at = NULL WHERE id = ?",
        )
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // Audited as the global switch is: both decide where Routarr writes alone.
    if req.auto_apply != existing.auto_apply {
        let message =
            if req.auto_apply { "AuditInstanceAutoApplyOn" } else { "AuditInstanceAutoApplyOff" };
        let event = crate::api::auth::allowed(crate::services::audit::Kind::Settings, message);
        crate::api::auth::audited(
            &state,
            &identity,
            client,
            event.with("instance", req.name.trim()),
        );
    }

    let (instance, localizer) = (state.instance(&id).await?, state.localizer().await);
    let key_set = key_set(&state, &instance);
    Ok(Json(InstanceResponse::from_instance(
        instance,
        &state.config.base_path,
        &localizer,
        key_set,
    )))
}

/// How long a removal waits for a webhook delivery of its instance, which holds the lock for one
/// title's sync and simulation.
const DELIVERY_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // Not under a sync or a delivery, which would fail on rows gone and notify a deleted instance.
    let syncing = async || {
        let message = state.localizer().await.translate("ErrorInstanceSyncing", &[]);
        AppError::InProgress { reason: "sync_running", message }
    };
    let Some(_sync) = state.jobs.try_lock(&format!("sync:{id}")) else {
        return Err(syncing().await);
    };
    let Some(_delivery) = state.jobs.lock_within(&format!("webhook:{id}"), DELIVERY_WAIT).await
    else {
        return Err(syncing().await);
    };
    // One transaction: the proposals go with the instance or not at all.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let Some(name) = sqlx::query_scalar::<_, String>("SELECT name FROM instances WHERE id = ?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Err(AppError::NotFound(format!("Instance {id} not found")));
    };
    crate::services::routing::supersede_instance_decisions(&mut tx, &id).await?;
    // Settled by the instance's next sync, which never comes. A revert keeps its move's time.
    let removed = crate::localization::Told::new("ErrorMoveInstanceRemoved").with("arr", &name);
    sqlx::query(
        "UPDATE decisions
            SET status = 'failed', error_message = ?,
                applied_at = CASE WHEN reverted_at IS NULL THEN NULL ELSE applied_at END
          WHERE instance_id = ? AND status = 'requested'",
    )
    .bind(removed.stored())
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM instances WHERE id = ?").bind(&id).execute(&mut *tx).await?;

    // A rule scoped to this instance alone is switched off: left with no instance, its scope
    // reads as every one, and it would route libraries it was never written for.
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
    /// What this release of the Arr lacks, below the oldest Routarr supports.
    pub warning: Option<String>,
    /// The Arr's webhook connection to the instance tested. Null for values not saved yet.
    pub webhook: Option<WebhookCheckResponse>,
}

pub async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<TestConnectionResponse>> {
    let instance = state.instance(&id).await?;
    let adapter = state.adapter(&instance)?;
    let localizer = state.localizer().await;
    let mut answer =
        check(&adapter, &instance.instance_type, &instance.base_url, &localizer).await?;
    answer.webhook = webhook_check::checked(&state, &instance, &adapter, &localizer).await;
    Ok(Json(answer))
}

/// Values typed in the instance form, tried before anything is saved.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    let mut answer = check(&adapter, &kind, &base_url, &localizer).await?;
    // The saved instance's Arr: its webhook connection is read as a test of it would read it.
    if let Some(saved) = saved.filter(|s| s.instance_type == kind && s.base_url == base_url) {
        answer.webhook = webhook_check::checked(&state, &saved, &adapter, &localizer).await;
    }
    Ok(Json(answer))
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

    let warning = crate::api::health::below_version(localizer, kind, None, &status.version);
    Ok(TestConnectionResponse {
        success: true,
        version: status.version,
        warning,
        app_name: status.app_name,
        root_folders: root_folders.len(),
        inaccessible_root_folders: root_folders.iter().filter(|rf| !rf.accessible).count(),
        webhook: None,
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
    // Switched off, as the scheduler and the webhook take it: nothing is read.
    let instance = state.instance(&id).await?;
    if !instance.enabled {
        let localizer = state.localizer().await;
        return Err(AppError::Conflict {
            reason: "instance_disabled",
            message: localizer.translate("ErrorInstanceSyncDisabled", &[("name", &instance.name)]),
        });
    }
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

/// Sync the instance and remove every title its Arr no longer reports, the removal a sync holds
/// when it would take most of the library. The owner's alone.
pub async fn remove_missing(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let instance = state.instance(&id).await?;
    if !instance.enabled {
        let localizer = state.localizer().await;
        return Err(AppError::Conflict {
            reason: "instance_disabled",
            message: localizer.translate("ErrorInstanceSyncDisabled", &[("name", &instance.name)]),
        });
    }
    let (task_state, by) = (state.clone(), identity.attribution());
    let work = async move {
        match sync::sync_removing_missing(&task_state, &id, &by).await {
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
        let _ = sqlx::query(
            "UPDATE jobs SET error_message = ?, error_code = ? WHERE id = ? AND status = 'failed'",
        )
        .bind(explained.public_message())
        .bind(explained.code())
        .bind(task)
        .execute(&state.pool)
        .await;
    }
    explained
}

async fn explain_sync_failure(state: &AppState, id: &str, cause: Cause) -> AppResult<AppError> {
    // A probe of the address tells a wrong type from a wrong port, which the sync's own
    // failure, a 404 on the library, cannot.
    let instance = state.instance(id).await?;
    let localizer = state.localizer().await;
    let adapter = state.adapter(&instance)?;
    let kind = instance.instance_type.as_str();
    check(&adapter, kind, &instance.base_url, &localizer).await?;
    // The probe passed, so the address is right and blaming it misleads: what failed is the
    // library listing, the one call large enough to outlast its timeout or outgrow its cap.
    let service = [("service", connection::service_name(kind))];
    Ok(match cause {
        Cause::TimedOut => {
            let named = [service[0], ("variable", "ROUTARR_LIBRARY_TIMEOUT_SECS")];
            AppError::UpstreamDown(localizer.translate("InstanceSyncTimedOut", &named))
        }
        Cause::TooLarge => {
            let named = [service[0], ("variable", "ROUTARR_MAX_LIBRARY_MIB")];
            AppError::UpstreamDown(localizer.translate("InstanceSyncTooLarge", &named))
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

/// Refuse to send a stored secret anywhere but the origin it was saved with: the form shows
/// neither the key nor a proxy's credentials back, so one left as shown on another address
/// would leave for it unseen. `refusal` names the one to type again.
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
        "UPDATE instances SET webhook_token = ?, webhook_token_at = datetime('now'),
         last_webhook_at = NULL, last_webhook_event = NULL, webhook_check = NULL,
         webhook_checked_at = NULL, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&id)
    .execute(&state.pool)
    .await?;

    let (instance, localizer) = (state.instance(&id).await?, state.localizer().await);
    let key_set = key_set(&state, &instance);
    Ok(Json(InstanceResponse::from_instance(
        instance,
        &state.config.base_path,
        &localizer,
        key_set,
    )))
}

/// Whether an instance may be written as given, and its address as stored:
/// the one check of a create, an update and a configuration import.
pub(crate) fn validate(
    name: &str,
    instance_type: &str,
    base_url: &str,
    localizer: &Localizer,
) -> AppResult<String> {
    instance_type.parse::<InstanceType>().map_err(|_| {
        AppError::BadRequest(localizer.translate("InstanceTypeUnknown", &[("type", instance_type)]))
    })?;

    if name.trim().is_empty() {
        return Err(AppError::BadRequest(localizer.translate("InstanceNameRequired", &[])));
    }

    normalize_base_url(base_url, localizer)
}

fn normalize_base_url(raw: &str, localizer: &Localizer) -> AppResult<String> {
    let base_url = raw.trim().trim_end_matches('/').to_string();
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(AppError::BadRequest(localizer.translate("InstanceUrlScheme", &[])));
    }
    // The API's path is appended: after a `?` or a `#` it would land in the query or the
    // fragment, and the key would go to whatever path the address names.
    if base_url.contains(['?', '#']) {
        return Err(AppError::BadRequest(localizer.translate("InstanceUrlPlain", &[])));
    }
    Ok(base_url)
}
