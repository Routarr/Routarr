//! Near-real-time routing via Radarr/Sonarr webhooks.
//!
//! A delivery authenticates with a per-instance token, rotated from the
//! Instances screen. Radarr 5.16 and Sonarr 4.0.11 send it in a header, as a
//! Basic password or in `X-Routarr-Token`, and an older Arr in the URL, where
//! every proxy between the two and the Arr's own log keep it.

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

/// The header an Arr that sends custom headers carries the token in.
pub const TOKEN_HEADER: &str = "X-Routarr-Token";

/// How long a delivery waits for the one before it, on its own task. A
/// season import arrives one file at a time, each answered before the next is
/// sent, so it never reaches it.
pub const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// How many deliveries may wait behind the one in progress, per instance.
///
/// An Arr delivers sequentially and waits for each response, so a second
/// waiter is already unusual, and four leaves room for a proxy that retries.
/// The wait is what bounds this route's cost to an unauthenticated caller, and
/// a queue of waiters with no bound of its own hands that cost straight back:
/// each one a task for as long as the wait lasts. Past the bound a delivery is
/// acknowledged without work.
pub const MAX_WAITING_DELIVERIES: usize = 4;

/// The events that can change where a title belongs, lower-cased: both Arrs
/// mark their event names as due to change case. `episodefiledelete` is
/// Sonarr's counterpart of `moviefiledelete`: removing the last episode file
/// flips `has_files`, and a rule reading it routes the series elsewhere.
const ACTED_ON: [&str; 8] = [
    "download",
    "movieadded",
    "seriesadd",
    "rename",
    "moviefiledelete",
    "episodefiledelete",
    "moviedelete",
    "seriesdelete",
];

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

/// Handle one webhook delivery.
async fn deliver(
    state: AppState,
    instance_id: &str,
    token: Option<String>,
    payload: ArrWebhook,
) -> AppResult<Response> {
    // One answer whether the id or the token is wrong: the route is reachable
    // without a key, and "instance not found" would confirm which ids exist.
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

    // A disabled instance is one the user switched off. The scheduler skips
    // it, and a webhook must not be the back door that keeps syncing it,
    // re-evaluating it, and (with automatic application armed) writing to it.
    // Acknowledged rather than refused: the Arr is configured correctly, and a
    // refusal would only put an error in its log that nobody there can act on.
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

    if !ACTED_ON.contains(&kind.as_str()) {
        let first = IGNORED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(kind);
        if first {
            info!(instance = %instance.name, event = %event, "Ignoring this webhook event");
        }
        return answered(serde_json::json!({ "ok": true, "ignored": event }));
    }

    // A Radarr address pasted into Sonarr sends series ids, which name other
    // titles here.
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

    // A place in the queue first, whether or not the lock turns out to be
    // free: the place is what bounds the queue, and it goes back the moment
    // the lock is held or the wait given up. A delivery for a title already
    // waiting adds nothing: the one waiting reads the title when its turn
    // comes.
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

    // On a task of its own, so an Arr that gives up on the delivery leaves the
    // work to its end, and answered with its outcome when it ends in time.
    // Past that the Arr is told it was accepted, before its own timeout: an
    // automatic move can wait minutes for an apply already running.
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
    // One delivery at a time per instance. This route is the only one an
    // unauthenticated party reaches, and each accepted call costs a request to
    // the Arr, a metadata fetch, a simulation and, with automatic application
    // armed, a write. A token read from a URL in a log starts as much, and
    // nothing else bounds it.
    //
    // Waited for, not skipped. An Arr does not retry a webhook it considers
    // delivered, and the run this guards is scoped to *one* media item, so a
    // delivery dropped here is that item left unsynced and unevaluated until
    // the next scheduled sweep, or for ever where `auto_sync_enabled` is off.
    // The cost of waiting is a timer, and the cost of the work is what stays
    // serialised, which is the whole point of the bound. The queue of timers
    // has a bound of its own, `MAX_WAITING_DELIVERIES`.
    let key = format!("webhook:{}", instance.id);
    let deadline = tokio::time::Instant::now() + DELIVERY_WAIT;

    // Past the budget it is dropped after all, and acknowledged rather than
    // refused: an Arr retries neither, and a refusal only fills its log.
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

    // Read from the row the sync just wrote rather than from the payload: a
    // delivery may omit `tmdbId`, as older Sonarr does, and enrichment would
    // then be skipped in silence. The row carries what the Arr's own API
    // returned, which is the fuller answer.
    if let Some(id) = media_id.as_deref() {
        // Metadata must exist before the rules run, otherwise the first
        // decision for a brand-new item falls back to the default category.
        let enriched = async {
            let media = crate::api::media::load_media(state, id).await?;
            enrichment::enrich_one(state, &media).await
        };
        if let Err(e) = enriched.await {
            warn!("Webhook enrichment failed for {id}: {e}");
        }
    }

    // Nothing to evaluate: `sync_single_media` answers `None` when the Arr no
    // longer has the item. Evaluated with no media filter, that is the whole
    // instance, persisted and automatically applied, outside the lock that
    // bounds exactly that.
    //
    // On a delete event the absence is the news itself, and the row goes the
    // way the full sync takes it: with its override, and with its pending
    // proposal retired. On any other event the row is left alone. A 404 there
    // can be a base URL pointing at something that is not an Arr, and the
    // full sync (which reads the whole list and refuses to act on an empty
    // one) is the safer judge of what is gone.
    let Some(only) = media_id.clone() else {
        let retired = if DELETE_EVENTS.contains(&kind) {
            // After any synchronisation in flight. One that read the Arr
            // before the deletion writes the row back, stamped current, and
            // keeps it until the next full pass. Retired once it has
            // committed, the row goes whatever the order of the reads. The
            // sync key is held for one transaction, so a scheduled sync that
            // lands in that instant is skipped for a tick and no more. Waited
            // for within what is left of the delivery budget: past it, the
            // next full pass is what retires the row.
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

    // Re-evaluate just this item. Sonarr sends a `Download` event per imported
    // episode file, so a season import would otherwise trigger one
    // full-library simulation per episode.
    let result = routing::run_simulation(
        &state.pool,
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

    // The whole point of routing on the webhook: the film has just been added
    // and nothing has downloaded yet, so the root folder can be corrected while
    // the folder is still empty. auto_apply decides whether it is allowed to.
    let auto_applied = match auto_apply::apply_simulation(
        state,
        &result.simulation_id,
        &crate::jobs::Attribution::unattended(crate::jobs::TRIGGER_WEBHOOK),
    )
    .await
    {
        Ok(AutoApplyOutcome::Applied(report)) => report.applied,
        Ok(_) => 0,
        // A failed auto-apply does not fail the webhook. The Arr logs a failed
        // notification and never sends it again, and the decision is stored
        // here, pending, for the user to apply.
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
