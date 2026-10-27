//! Near-real-time routing through Radarr and Sonarr webhooks, authenticated by a per-instance
//! token: a Basic password from any release, `X-Routarr-Token` from Radarr 5.16 and Sonarr 4.0.11,
//! else the URL, where every proxy and the Arr's own log keep it.

use super::Json;
use axum::extract::{FromRequest, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::Path;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use tracing::{info, warn};

use crate::api::auth::{Client, refusal, refused};
use crate::error::{AppError, AppResult};
use crate::integrations::arr_webhook::{self, TOKEN_HEADER};
use crate::jobs::registry::WaitingPlace;
use crate::models::Instance;
use crate::services::audit::Kind;
use crate::services::auto_apply::{self, AutoApplyOutcome};
use crate::services::{enrichment, routing, sync};
use crate::state::AppState;

/// The subset of the Arr webhook payload Routarr acts on.
#[derive(Debug, Deserialize)]
pub struct ArrWebhook {
    #[serde(rename = "eventType")]
    pub event_type: Option<String>,
    pub movie: Option<ArrWebhookMedia>,
    pub series: Option<ArrWebhookMedia>,
}

#[derive(Debug, Deserialize)]
pub struct ArrWebhookMedia {
    pub id: Option<i64>,
    pub title: Option<String>,
}

/// The deliveries, and the automatic moves, that may wait per instance: an Arr sends one at a
/// time, and four leave room for a retrying proxy. Past it a delivery is acknowledged without
/// work and a move is left to the next sync, so an unauthenticated caller cannot pile up tasks.
pub const MAX_WAITING_DELIVERIES: usize = 4;

/// The events that report an item is gone rather than changed.
const DELETE_EVENTS: [&str; 2] = ["moviedelete", "seriesdelete"];

/// The events ignored so far, each logged the first time it arrives.
static IGNORED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// How many event types [`IGNORED`] keeps: the Arrs send about twenty, and a type sent by
/// whoever holds the token stays in memory until the process stops.
pub const IGNORED_KEPT: usize = 64;

/// The longest event type kept, as long as the longest an Arr sends with room to spare.
const EVENT_LENGTH: usize = 64;

/// How many event types are remembered as ignored, every test's included. Only a test asks.
#[cfg(test)]
pub fn ignored_kinds() -> usize {
    IGNORED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
}

/// The largest delivery read: a season import's is tens of KiB.
pub const BODY_MAX: usize = 1024 * 1024;

/// A delivery carrying its token in the URL, for an Arr that sends no header.
pub async fn receive(
    State(state): State<AppState>,
    Path((instance_id, token)): Path<(String, String)>,
    Client(client): Client,
    request: Request,
) -> Response {
    deliver(state, &instance_id, Some(token), client, request).await
}

/// A delivery carrying its token in a header.
pub async fn receive_with_header(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
    Client(client): Client,
    request: Request,
) -> Response {
    let token = header_token(request.headers());
    deliver(state, &instance_id, token, client, request).await
}

/// The delivery `request` carries, read once its token names `instance_id`: a stranger's body
/// is never read, and its refusal says nothing about its shape.
async fn deliver(
    state: AppState,
    instance_id: &str,
    token: Option<String>,
    client: Option<std::net::IpAddr>,
    request: Request,
) -> Response {
    let instance = match authenticated(&state, instance_id, token, client).await {
        Ok(instance) => instance,
        Err(refused) => return refused.into_response(),
    };
    match Json::<ArrWebhook>::from_request(request, &state).await {
        Ok(Json(payload)) => delivered(state, instance, payload).await.into_response(),
        Err(rejection) => rejection,
    }
}

/// The token of `X-Routarr-Token`, or the password of Basic credentials.
fn header_token(headers: &HeaderMap) -> Option<String> {
    use base64::Engine as _;
    if let Some(token) = headers.get(TOKEN_HEADER).and_then(|value| value.to_str().ok()) {
        return Some(token.trim().to_string());
    }
    let credentials = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = credentials.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()?;
    let (_, password) = String::from_utf8(decoded)
        .ok()?
        .split_once(':')
        .map(|(u, p)| (u.to_string(), p.to_string()))?;
    Some(password)
}

/// Record that `instance` heard from its Arr, at most once a minute: a season sends one delivery
/// per episode. The stamp leaves `updated_at` alone, which the probe reuse reads.
async fn stamp_delivery(state: &AppState, instance: &str, event: Option<&str>) {
    let stamped = sqlx::query(
        "UPDATE instances SET last_webhook_at = datetime('now'), last_webhook_event = ?
          WHERE id = ?
            AND (last_webhook_at IS NULL OR last_webhook_at <= datetime('now', '-1 minute'))",
    )
    .bind(event)
    .bind(instance)
    .execute(&state.pool)
    .await;
    if let Err(e) = stamped {
        warn!("Could not record a webhook delivery: {e}");
    }
}

/// The instance `token` is the webhook token of. A wrong one is a refusal of the security log,
/// summed per address, as a key that opens nothing is.
async fn authenticated(
    state: &AppState,
    instance_id: &str,
    token: Option<String>,
    client: Option<std::net::IpAddr>,
) -> AppResult<Instance> {
    // One answer for a wrong id and a wrong token, or the route confirms which ids exist.
    let instance = state
        .instance(instance_id)
        .await
        .map_err(|_| AppError::NotFound("Unknown webhook".into()))?;

    let expected = instance.webhook_token.as_deref().unwrap_or_default();
    let given = token.unwrap_or_default();
    if expected.is_empty() || !super::auth::constant_time_eq(&given, expected) {
        let event =
            refusal(Kind::Webhook, "AuditWebhookTokenRefused").with("instance", &instance.name);
        refused(state, client, event);
        return Err(AppError::NotFound("Unknown webhook".into()));
    }
    Ok(instance)
}

async fn delivered(
    state: AppState,
    instance: Instance,
    payload: ArrWebhook,
) -> AppResult<Response> {
    // Cut once, before the log, the stamp, the answer and the set of ignored types read it.
    let carried: Option<String> = payload
        .event_type
        .as_deref()
        .map(|event| event.chars().filter(|c| !c.is_control()).take(EVENT_LENGTH).collect())
        .filter(|event: &String| !event.is_empty());
    stamp_delivery(&state, &instance.id, carried.as_deref()).await;
    // English in the log and the answer only, never in the stamp the interface reads.
    let event = carried.unwrap_or_else(|| "Unknown".to_string());

    // No back door into an instance switched off. Acknowledged, not refused: the Arr is
    // configured correctly, and a refusal would only fill its log.
    if !instance.enabled {
        info!(instance = %instance.name, "Ignored a webhook for a disabled instance");
        return answered(serde_json::json!({ "ok": true, "ignored": "instance is disabled" }));
    }

    let kind = event.to_ascii_lowercase();

    // `Test` is what the Arr sends when the user clicks "Test" in its UI.
    if kind == "test" {
        return answered(serde_json::json!({ "ok": true, "message": "Webhook reachable" }));
    }

    if !arr_webhook::acted_on(&kind) {
        let mut ignored = IGNORED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if ignored.len() < IGNORED_KEPT {
            if ignored.insert(kind) {
                info!(instance = %instance.name, event = %event, "Ignoring this webhook event");
            }
        } else if !ignored.contains(&kind) {
            info!(instance = %instance.name, "Ignoring a webhook event of a type past those remembered");
        }
        drop(ignored);
        return answered(serde_json::json!({ "ok": true, "ignored": event }));
    }

    // A Radarr address pasted into Sonarr sends series ids, which name other titles here.
    let (media, other) = match instance.instance_type.as_str() {
        "radarr" => (payload.movie, payload.series),
        _ => (payload.series, payload.movie),
    };
    if media.is_none() && other.is_some() {
        warn!(instance = %instance.name, event = %event, "A webhook for another kind of Arr was ignored");
        return answered(serde_json::json!({
            "ok": true, "ignored": "the payload is for another kind of Arr",
        }));
    }
    let Some(arr_id) = media.as_ref().and_then(|m| m.id) else {
        return answered(
            serde_json::json!({ "ok": true, "ignored": "payload without a media id" }),
        );
    };

    info!(
        instance = %instance.name,
        event = %event,
        title = media.as_ref().and_then(|m| m.title.as_deref()).unwrap_or("?"),
        "Webhook received"
    );

    // A place first, even when the lock is free, since the places bound the queue. A title
    // already waiting needs no second delivery: the waiting one reads it when its turn comes.
    let Some(item) = state.jobs.try_wait(&format!("webhook:{}:{arr_id}", instance.id), 1) else {
        return answered(serde_json::json!({
            "ok": true, "ignored": "a delivery for that item is already waiting",
        }));
    };
    let Some(place) =
        state.jobs.try_wait(&format!("webhook:{}", instance.id), MAX_WAITING_DELIVERIES)
    else {
        warn!(
            instance = %instance.name,
            "A delivery found {MAX_WAITING_DELIVERIES} already waiting and was dropped"
        );
        return answered(serde_json::json!({
            "ok": true, "ignored": "too many deliveries are waiting for the instance",
        }));
    };

    // On its own task, so an Arr giving up leaves the work to finish. Past the wait a 202 goes
    // out before the Arr's own timeout: an automatic move can wait minutes behind a running apply.
    let name = instance.name.clone();
    let work = state.jobs.spawn_tracked(process(
        state.clone(),
        instance,
        event.clone(),
        kind,
        arr_id,
        [item, place],
    ));
    match tokio::time::timeout(state.config.webhook_answer_wait, work).await {
        Ok(Ok(Ok(body))) => answered(body),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(e)) => {
            Err(AppError::Internal(format!("the delivery ended before it reported: {e}")))
        }
        Err(_) => {
            info!(instance = %name, event = %event, "A delivery goes on after its answer");
            Ok((
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({ "ok": true, "accepted": true, "event": event })),
            )
                .into_response())
        }
    }
}

/// The point of the webhook: a title just added moves while its folder is still empty. Answers
/// how many moved.
async fn automatic_move(state: &AppState, instance: &Instance, simulation_id: &str) -> usize {
    let by = crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_WEBHOOK);
    match auto_apply::apply_instance_simulation(state, simulation_id, &instance.id, &by).await {
        Ok(AutoApplyOutcome::Applied { report, .. }) => report.applied,
        Ok(_) => 0,
        // Not the delivery's failure: the Arr never resends it, and the decision waits, pending.
        Err(e) => {
            warn!("Auto-apply after webhook failed: {e}");
            0
        }
    }
}

fn answered(body: serde_json::Value) -> AppResult<Response> {
    Ok(axum::Json(body).into_response())
}

/// The work of one delivery: the title synced, enriched, evaluated and, when
/// allowed, moved. The places in the queue go back once the instance is
/// held, or the wait given up.
async fn process(
    state: AppState,
    instance: Instance,
    event: String,
    kind: String,
    arr_id: i64,
    places: [WaitingPlace; 2],
) -> AppResult<serde_json::Value> {
    let outcome = run(&state, &instance, &event, &kind, arr_id, places).await;
    if let Err(e) = &outcome {
        warn!(instance = %instance.name, event = %event, "A webhook delivery failed: {e}");
    }
    outcome
}

async fn run(
    state: &AppState,
    instance: &Instance,
    event: &str,
    kind: &str,
    arr_id: i64,
    places: [WaitingPlace; 2],
) -> AppResult<serde_json::Value> {
    // One delivery at a time per instance: a token leaked from a URL in a log buys, per call, an
    // Arr request, a metadata fetch, a simulation and maybe a write. Waited for, not skipped: an
    // Arr never retries, and a dropped delivery leaves its title unsynced until the next sweep.
    let key = format!("webhook:{}", instance.id);
    let deadline = tokio::time::Instant::now() + state.config.delivery_wait;

    // Dropped past the budget, acknowledged all the same: a refusal would only fill the Arr's log.
    let Some(delivery) = state.jobs.lock_within(&key, state.config.delivery_wait).await else {
        warn!(
            instance = %instance.name,
            "A delivery waited {}s for the one before it and was dropped",
            state.config.delivery_wait.as_secs()
        );
        return Ok(
            serde_json::json!({ "ok": true, "ignored": "another delivery held the instance" }),
        );
    };
    drop(places);
    // Read again: the instance may have been removed or switched off while this waited.
    let current = match state.instance(&instance.id).await {
        Ok(current) if current.enabled => current,
        Ok(_) => return Ok(serde_json::json!({ "ok": true, "ignored": "instance is disabled" })),
        Err(AppError::NotFound(_)) => {
            return Ok(serde_json::json!({ "ok": true, "ignored": "instance was removed" }));
        }
        Err(e) => return Err(e),
    };
    let instance = &current;

    let media_id = sync::sync_single_media(state, instance, arr_id).await?;

    // Read from the synced row, not the payload: a delivery may omit `tmdbId`, as older Sonarr
    // does, and enrichment would be skipped in silence.
    if let Some(id) = media_id.as_deref() {
        // Before the rules, or a new title's first decision falls back to the default category.
        let enriched = async {
            let media = crate::api::library::load_media(state, id).await?;
            enrichment::enrich_one(state, &media).await
        };
        if let Err(e) = enriched.await {
            warn!("Webhook enrichment failed for {id}: {e}");
        }
    }

    // `None` means the Arr lacks the item, and a simulation with no media filter runs the whole
    // instance. A delete retires the row as the full sync does. On another event a 404 may be an
    // address that is no Arr, so the full sync, which refuses an empty list, judges what is gone.
    let Some(only) = media_id.clone() else {
        let retired = if DELETE_EVENTS.contains(&kind) {
            // After any sync in flight, which would write back a row read before the deletion.
            // A scheduled sync meeting the held key skips one tick. Past the delivery budget,
            // the next full pass retires the row.
            let sync_key = format!("sync:{}", instance.id);
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Some(_sync) = state.jobs.lock_within(&sync_key, left).await else {
                warn!(
                    instance = %instance.name,
                    "A delete waited for a synchronisation past the budget and was dropped"
                );
                return Ok(serde_json::json!({
                    "ok": true, "event": event, "media_id": serde_json::Value::Null,
                    "retired": 0, "ignored": "a synchronisation held the instance",
                }));
            };
            let mut tx = crate::db::write_transaction(&state.pool).await?;
            let retired =
                sync::retire_media(&mut tx, &[sync::media_row_id(&instance.id, arr_id)]).await?;
            tx.commit().await?;
            retired
        } else {
            0
        };
        info!(instance = %instance.name, event = %event, retired, "The Arr no longer has that item");
        return Ok(serde_json::json!({
            "ok": true, "event": event, "media_id": serde_json::Value::Null,
            "retired": retired, "ignored": "the Arr no longer has that item",
        }));
    };

    // This title alone: Sonarr sends a `Download` for each episode file a season import brings.
    let result = routing::run_simulation(
        &state.pool,
        &state.library_passes,
        routing::SimulationOptions {
            trigger: crate::jobs::TRIGGER_WEBHOOK.to_string(),
            instance_ids: vec![instance.id.clone()],
            media_ids: Some(vec![only]),
            persist: true,
            max_returned: Some(1),
            language: state.language().await,
            ..Default::default()
        },
    )
    .await?;

    let moving =
        state.jobs.try_wait(&format!("auto-apply:{}", instance.id), MAX_WAITING_DELIVERIES);
    // The automatic move can wait minutes for a running apply, which the next delivery must not.
    drop(delivery);
    let auto_applied = match moving {
        Some(_place) => automatic_move(state, instance, &result.simulation_id).await,
        None => {
            info!(
                instance = %instance.name,
                "An automatic move found {MAX_WAITING_DELIVERIES} others waiting and was left \
                 to the next sync"
            );
            0
        }
    };

    Ok(serde_json::json!({
        "ok": true,
        "event": event,
        "media_id": media_id,
        "moves_required": result.moves_required,
        "auto_applied": auto_applied,
    }))
}
