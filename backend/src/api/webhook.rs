//! Near-real-time routing via Radarr/Sonarr webhooks.
//!
//! Handling new additions has to be possible through polling or
//! through a webhook. Radarr and Sonarr cannot send custom headers, so the
//! endpoint authenticates with a per-instance token embedded in the URL, which
//! can be rotated from the Instances screen.

use super::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
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

/// How long a delivery waits for the one before it.
///
/// Comfortably under the timeout an Arr gives a notification, so a delivery is
/// answered rather than left to time out at the other end — and long enough
/// that a season import, which arrives sequentially anyway, never reaches it.
pub const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// How many deliveries may wait behind the one in progress, per instance.
///
/// An Arr delivers sequentially and waits for each response, so a second
/// waiter is already unusual; four leaves room for a proxy that retries. The
/// wait is what bounds this route's cost to an unauthenticated caller, and a
/// queue of waiters with no bound of its own hands that cost straight back —
/// each one a connection and a task for as long as the wait lasts. Past the
/// bound a delivery is acknowledged without work.
pub const MAX_WAITING_DELIVERIES: usize = 4;

/// The events that report an item is gone rather than changed.
const DELETE_EVENTS: [&str; 2] = ["MovieDelete", "SeriesDelete"];

/// Handle one webhook delivery.
pub async fn receive(
    State(state): State<AppState>,
    Path((instance_id, token)): Path<(String, String)>,
    Json(payload): Json<ArrWebhook>,
) -> AppResult<Json<serde_json::Value>> {
    // One answer whether the id or the token is wrong: the route is reachable
    // without a key, and "instance not found" would confirm which ids exist.
    let instance = state
        .instance(&instance_id)
        .await
        .map_err(|_| AppError::NotFound("Unknown webhook".into()))?;

    let expected = instance.webhook_token.as_deref().unwrap_or_default();
    if expected.is_empty() || !constant_time_eq(&token, expected) {
        warn!(instance = %instance.name, "Rejected a webhook with an invalid token");
        return Err(AppError::NotFound("Unknown webhook".into()));
    }

    // A disabled instance is one the user switched off; the scheduler already
    // skips it, and a webhook must not be the back door that keeps syncing it,
    // re-evaluating it, and — with automatic application armed — writing to it.
    // Acknowledged rather than refused: the Arr is configured correctly and
    // would only retry a rejection it can do nothing about.
    if !instance.enabled {
        info!(instance = %instance.name, "Ignored a webhook for a disabled instance");
        return Ok(Json(serde_json::json!({ "ok": true, "ignored": "instance is disabled" })));
    }

    let event = payload.event_type.clone().unwrap_or_else(|| "Unknown".into());

    // `Test` is what the Arr sends when the user clicks "Test" in its UI.
    if event.eq_ignore_ascii_case("Test") {
        return Ok(Json(serde_json::json!({ "ok": true, "message": "Webhook reachable" })));
    }

    // Only events that can change where a media item belongs are worth acting on.
    if !matches!(
        event.as_str(),
        "Download"
            | "MovieAdded"
            | "SeriesAdd"
            | "Rename"
            | "MovieFileDelete"
            // Sonarr's counterpart of MovieFileDelete, which was handled while
            // this was not: removing the last episode file flips `has_files`,
            // and a rule reading it then routes the series somewhere else.
            | "EpisodeFileDelete"
    ) && !DELETE_EVENTS.contains(&event.as_str())
    {
        return Ok(Json(serde_json::json!({ "ok": true, "ignored": event })));
    }

    let media = payload.movie.or(payload.series);
    let Some(arr_id) = media.as_ref().and_then(|m| m.id) else {
        return Ok(Json(
            serde_json::json!({ "ok": true, "ignored": "payload without a media id" }),
        ));
    };

    info!(
        instance = %instance.name,
        event = %event,
        title = media.as_ref().and_then(|m| m.title.as_deref()).unwrap_or("?"),
        "Webhook received"
    );

    // One delivery at a time per instance. This route is the only one an
    // unauthenticated party reaches, and each accepted call costs a request to
    // the Arr, a metadata fetch, a simulation and — with automatic application
    // armed — a write. The token travels in the URL, so it is in the proxy's
    // log and in Radarr's own; once read, nothing bounded what it could start.
    //
    // Waited for, not skipped. An Arr does not retry a webhook it considers
    // delivered, and the run this guards is scoped to *one* media item — so a
    // delivery dropped here is that item left unsynced and unevaluated until
    // the next scheduled sweep, or for ever where `auto_sync_enabled` is off.
    // The cost of waiting is a timer; the cost of the work is what stays
    // serialised, which is the whole point of the bound. The queue of timers
    // has a bound of its own — see `MAX_WAITING_DELIVERIES`.
    let key = format!("webhook:{}", instance.id);
    let deadline = tokio::time::Instant::now() + DELIVERY_WAIT;

    // A place in the queue first, whether or not the lock turns out to be
    // free: the place is what bounds the queue, and it goes back the moment
    // the lock is held or the wait given up. Then the lock, in arrival order —
    // a newcomer never passes a delivery already waiting.
    let Some(place) = state.jobs.try_wait(&key, MAX_WAITING_DELIVERIES) else {
        warn!(
            instance = %instance.name,
            "A delivery found {MAX_WAITING_DELIVERIES} already waiting and was dropped"
        );
        return Ok(Json(serde_json::json!({
            "ok": true, "ignored": "too many deliveries are waiting for the instance",
        })));
    };
    // Past the budget it is dropped after all, and acknowledged rather than
    // refused: an Arr retries neither, and a refusal only fills its log.
    let Some(_delivery) = state.jobs.lock_within(&key, DELIVERY_WAIT).await else {
        warn!(
            instance = %instance.name,
            "A delivery waited {}s for the one before it and was dropped",
            DELIVERY_WAIT.as_secs()
        );
        return Ok(Json(
            serde_json::json!({ "ok": true, "ignored": "another delivery held the instance" }),
        ));
    };
    drop(place);

    let media_id = sync::sync_single_media(&state, &instance, arr_id).await?;

    // Read from the row the sync just wrote rather than from the payload. The
    // comment below has always said metadata must exist before the rules run,
    // but the identifier was taken from the delivery — and an Arr that omits
    // `tmdbId`, as older Sonarr does, skipped enrichment in silence. The row
    // carries what the Arr's own API returned, which is the fuller answer.
    if let Some(id) = media_id.as_deref() {
        let identity: Option<(Option<i64>, String)> =
            sqlx::query_as("SELECT tmdb_id, media_type FROM media WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?;

        // Metadata must exist before the rules run, otherwise the first decision
        // for a brand-new item always falls back to the default category.
        if let Some((Some(tmdb_id), media_type)) = identity
            && let Err(e) = enrichment::enrich_one(&state, tmdb_id, &media_type).await
        {
            warn!("Webhook enrichment failed for TMDb {tmdb_id}: {e}");
        }
    }

    // Nothing to evaluate: `sync_single_media` answers `None` when the Arr no
    // longer has the item. Evaluated with no media filter, that is the whole
    // instance — persisted and automatically applied — outside the lock that
    // bounds exactly that.
    //
    // On a delete event the absence is the news itself, and the row goes the
    // way the full sync takes it: with its override, and with its pending
    // proposal retired. On any other event the row is left alone. A 404 there
    // can be a base URL pointing at something that is not an Arr, and the
    // full sync — which reads the whole list and refuses to act on an empty
    // one — is the safer judge of what is gone.
    let Some(only) = media_id.clone() else {
        let retired = if DELETE_EVENTS.contains(&event.as_str()) {
            // After any synchronisation in flight. One that read the Arr
            // before the deletion writes the row back, stamped current, and
            // keeps it until the next full pass; retired once it has
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
                return Ok(Json(serde_json::json!({
                    "ok": true, "event": event, "media_id": serde_json::Value::Null,
                    "retired": 0, "ignored": "a synchronisation held the instance",
                })));
            };
            let mut tx = state.pool.begin().await?;
            let retired =
                sync::retire_media(&mut tx, &[sync::media_row_id(&instance.id, arr_id)]).await?;
            tx.commit().await?;
            retired
        } else {
            0
        };
        info!(instance = %instance.name, event = %event, retired, "The Arr no longer has that item");
        return Ok(Json(serde_json::json!({
            "ok": true, "event": event, "media_id": serde_json::Value::Null,
            "retired": retired, "ignored": "the Arr no longer has that item",
        })));
    };

    // Re-evaluate just this item. Radarr sends a `Download` event per imported
    // file, so a season import would otherwise trigger one full-library
    // simulation per episode.
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
        &state,
        &result.simulation_id,
        crate::jobs::TRIGGER_WEBHOOK,
    )
    .await
    {
        Ok(AutoApplyOutcome::Applied(report)) => report.applied,
        Ok(_) => 0,
        // A failed auto-apply must not fail the webhook: Radarr would retry the
        // event, and the decision is still pending for the user to apply.
        Err(e) => {
            warn!("Auto-apply after webhook failed: {e}");
            0
        }
    };

    Ok(Json(serde_json::json!({
        "ok": true,
        "event": event,
        "media_id": media_id,
        "moves_required": result.moves_required,
        "auto_applied": auto_applied,
    })))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.ct_eq(b).into()
}
