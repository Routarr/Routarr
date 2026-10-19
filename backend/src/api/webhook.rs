//! Near-real-time routing through Radarr and Sonarr webhooks, authenticated by a per-instance
//! token. Radarr 5.16 and Sonarr 4.0.11 send it in a header (a Basic password or
//! `X-Routarr-Token`), an older Arr in the URL, where every proxy and the Arr's own log keep it.

use super::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::Path;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::integrations::arr_webhook::{self, TOKEN_HEADER};
use crate::jobs::registry::WaitingPlace;
use crate::models::Instance;
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

/// How long a delivery waits for the one before it, on its own task. A
/// season import arrives one file at a time, each answered before the next is
/// sent, so it never reaches it.
pub const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// The deliveries that may wait behind the one in progress, per instance: an Arr sends one at a
/// time, and four leave room for a retrying proxy. Unbounded, the waiters would hand an
/// unauthenticated caller a task each. Past the bound a delivery is acknowledged without work.
pub const MAX_WAITING_DELIVERIES: usize = 4;

/// The events that report an item is gone rather than changed.
const DELETE_EVENTS: [&str; 2] = ["moviedelete", "seriesdelete"];

/// The events ignored so far, each logged the first time it arrives.
static IGNORED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// A delivery carrying its token in the URL, for an Arr that sends no header.
pub async fn receive(
    State(state): State<AppState>,
    Path((instance_id, token)): Path<(String, String)>,
    Json(payload): Json<ArrWebhook>,
) -> AppResult<Response> {
    deliver(state, &instance_id, Some(token), payload).await
}

/// A delivery carrying its token in a header.
pub async fn receive_with_header(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<ArrWebhook>,
) -> AppResult<Response> {
    deliver(state, &instance_id, header_token(&headers), payload).await
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
async fn stamp_delivery(state: &AppState, instance: &str, event: &str) {
    let event: String = event.chars().take(64).collect();
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

async fn deliver(
    state: AppState,
    instance_id: &str,
    token: Option<String>,
    payload: ArrWebhook,
) -> AppResult<Response> {
    // One answer for a wrong id and a wrong token, or the route confirms which ids exist.
    let instance = state
        .instance(instance_id)
        .await
        .map_err(|_| AppError::NotFound("Unknown webhook".into()))?;

    let expected = instance.webhook_token.as_deref().unwrap_or_default();
    let given = token.unwrap_or_default();
    if expected.is_empty() || !super::auth::constant_time_eq(&given, expected) {
        warn!(instance = %instance.name, "Rejected a webhook with an invalid token");
        return Err(AppError::NotFound("Unknown webhook".into()));
    }
    stamp_delivery(&state, &instance.id, payload.event_type.as_deref().unwrap_or("Unknown")).await;

    // No back door into an instance switched off. Acknowledged, not refused: the Arr is
    // configured correctly, and a refusal would only fill its log.
    if !instance.enabled {
        info!(instance = %instance.name, "Ignored a webhook for a disabled instance");
        return answered(serde_json::json!({ "ok": true, "ignored": "instance is disabled" }));
    }

    let event = payload.event_type.clone().unwrap_or_else(|| "Unknown".into());
    let kind = event.to_ascii_lowercase();

    // `Test` is what the Arr sends when the user clicks "Test" in its UI.
    if kind == "test" {
        return answered(serde_json::json!({ "ok": true, "message": "Webhook reachable" }));
    }

    if !arr_webhook::acted_on(&kind) {
        let first = IGNORED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(kind);
        if first {
            info!(instance = %instance.name, event = %event, "Ignoring this webhook event");
        }
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
    let deadline = tokio::time::Instant::now() + DELIVERY_WAIT;

    // Dropped past the budget, acknowledged all the same: a refusal would only fill the Arr's log.
    let Some(_delivery) = state.jobs.lock_within(&key, DELIVERY_WAIT).await else {
        warn!(
            instance = %instance.name,
            "A delivery waited {}s for the one before it and was dropped",
            DELIVERY_WAIT.as_secs()
        );
        return Ok(
            serde_json::json!({ "ok": true, "ignored": "another delivery held the instance" }),
        );
    };
    drop(places);

    let media_id = sync::sync_single_media(state, instance, arr_id).await?;

    // Read from the synced row, not the payload: a delivery may omit `tmdbId`, as older Sonarr
    // does, and enrichment would be skipped in silence.
    if let Some(id) = media_id.as_deref() {
        // Before the rules, or a new title's first decision falls back to the default category.
        let enriched = async {
            let media = crate::api::media::load_media(state, id).await?;
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

    // The point of the webhook: a title just added moves while its folder is still empty.
    let auto_applied = match auto_apply::apply_instance_simulation(
        state,
        &result.simulation_id,
        &instance.id,
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_WEBHOOK),
    )
    .await
    {
        Ok(AutoApplyOutcome::Applied { report, .. }) => report.applied,
        Ok(_) => 0,
        // Not the delivery's failure: the Arr never resends it, and the decision waits, pending.
        Err(e) => {
            warn!("Auto-apply after webhook failed: {e}");
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
