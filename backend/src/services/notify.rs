//! Telling the operator, or another application, what happened while nobody
//! was looking.
//!
//! Routarr does most of its work on a schedule or on a webhook, and in a
//! homelab nobody opens the interface daily. A scheduled sync that has been
//! failing for a week, or an automatic apply that Radarr rejected, is invisible
//! until someone thinks to look.
//!
//! One mechanism, not a gallery of connectors: a POST of JSON to a URL the user
//! configures. The payload carries the same text under the field names the usual
//! receivers read (`content` for Discord, `message` for Gotify and ntfy, `body`
//! for Apprise), alongside Routarr's own structured fields for anything that
//! parses properly. With a signing secret it is signed as Standard Webhooks
//! specifies, so a receiver can tell it came from Routarr.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::Sha256;
use tokio::sync::Semaphore;
use tracing::{debug, error, warn};

use crate::error::{AppError, AppResult};
use crate::integrations::send_ok;
use crate::state::AppState;

/// Something worth telling.
///
/// The failures are sent whenever a webhook is set: each will not resolve
/// itself, and the operator has to act on it. What finished well is sent only
/// to a webhook that asked for it, by its setting: a channel a person reads
/// must not fill with a message per simulation.
#[derive(Debug, Clone)]
pub enum Event {
    /// A sync could not reach an Arr. Sent on the transition only.
    InstanceUnreachable { instance: String, error: String },
    /// The same instance answered again. Closes the loop so the operator does
    /// not have to go and check whether the problem is still there.
    InstanceRecovered { instance: String },
    /// An unattended apply wrote to an Arr and the Arr refused.
    AutoApplyFailed { failed: usize, applied: usize, first_error: String },
    /// A sync failed, every time it does. Asked for by `notify_sync_failed`.
    SyncFailed { instance_id: String, instance: String, error: String },
    /// A simulation stored its proposals. Asked for by
    /// `notify_simulation_completed`.
    SimulationCompleted { simulation_id: String, total: usize, moves: usize },
    /// An apply or a revert finished. Asked for by `notify_moves_completed`.
    MovesCompleted { reverted: bool, applied: usize, failed: usize, skipped: usize },
}

impl Event {
    fn severity(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. }
            | Event::AutoApplyFailed { .. }
            | Event::SyncFailed { .. } => "error",
            Event::MovesCompleted { failed, .. } if *failed > 0 => "warning",
            _ => "info",
        }
    }

    /// The name the payload's `event` has always carried.
    fn kind(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. } => "instance_unreachable",
            Event::InstanceRecovered { .. } => "instance_recovered",
            Event::AutoApplyFailed { .. } => "auto_apply_failed",
            Event::SyncFailed { .. } => "sync_failed",
            Event::SimulationCompleted { .. } => "simulation_completed",
            Event::MovesCompleted { reverted: false, .. } => "apply_completed",
            Event::MovesCompleted { reverted: true, .. } => "revert_completed",
        }
    }

    /// The Standard Webhooks `type`, dotted.
    fn event_type(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. } => "instance.unreachable",
            Event::InstanceRecovered { .. } => "instance.recovered",
            Event::AutoApplyFailed { .. } => "auto_apply.failed",
            Event::SyncFailed { .. } => "sync.failed",
            Event::SimulationCompleted { .. } => "simulation.completed",
            Event::MovesCompleted { reverted: false, .. } => "apply.completed",
            Event::MovesCompleted { reverted: true, .. } => "revert.completed",
        }
    }

    /// The setting a webhook asks for this event with, `None` for a failure,
    /// which is always sent.
    fn asked_by(&self) -> Option<&'static str> {
        match self {
            Event::SyncFailed { .. } => Some("notify_sync_failed"),
            Event::SimulationCompleted { .. } => Some("notify_simulation_completed"),
            Event::MovesCompleted { .. } => Some("notify_moves_completed"),
            _ => None,
        }
    }

    /// The human sentence. Deliberately English: this goes to a machine relay,
    /// not to the interface, and a notification whose language depends on a UI
    /// setting is a notification nobody can grep.
    fn message(&self) -> String {
        match self {
            Event::InstanceUnreachable { instance, error } => {
                format!("Routarr can no longer reach '{instance}': {error}")
            }
            Event::InstanceRecovered { instance } => {
                format!("Routarr can reach '{instance}' again.")
            }
            Event::AutoApplyFailed { failed, applied, first_error } => format!(
                "Routarr failed to apply routing decisions automatically. Failures: {failed} \
                 ({applied} succeeded). First error: {first_error}"
            ),
            Event::SyncFailed { instance, error, .. } => {
                format!("Routarr could not sync '{instance}': {error}")
            }
            Event::SimulationCompleted { total, moves, .. } => {
                format!("Routarr simulated the library. Titles: {total}, moves proposed: {moves}.")
            }
            Event::MovesCompleted { reverted, applied, failed, skipped } => format!(
                "Routarr {} titles. Moved: {applied}, failed: {failed}, skipped: {skipped}.",
                if *reverted { "moved back" } else { "moved" }
            ),
        }
    }

    /// Ids and counts, never prose: what a program reads.
    fn data(&self) -> serde_json::Value {
        match self {
            Event::InstanceUnreachable { instance, .. } | Event::InstanceRecovered { instance } => {
                serde_json::json!({ "instance": instance })
            }
            Event::AutoApplyFailed { failed, applied, .. } => {
                serde_json::json!({ "failed": failed, "applied": applied })
            }
            Event::SyncFailed { instance_id, instance, .. } => {
                serde_json::json!({ "instance_id": instance_id, "instance": instance })
            }
            Event::SimulationCompleted { simulation_id, total, moves } => serde_json::json!({
                "simulation_id": simulation_id, "total": total, "moves": moves
            }),
            Event::MovesCompleted { applied, failed, skipped, .. } => {
                serde_json::json!({ "applied": applied, "failed": failed, "skipped": skipped })
            }
        }
    }
}

/// What the notification webhook receives.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Notification {
    /// What happened, in snake case: `instance_unreachable`,
    /// `instance_recovered`, `auto_apply_failed`, `sync_failed`,
    /// `simulation_completed`, `apply_completed` or `revert_completed`.
    pub event: &'static str,
    /// The same, dotted as Standard Webhooks writes it: `instance.unreachable`,
    /// `sync.failed`, `apply.completed`...
    #[serde(rename = "type")]
    pub event_type: &'static str,
    /// When it happened, in RFC 3339.
    pub timestamp: String,
    /// `error`, `warning` or `info`.
    pub severity: &'static str,
    /// Always `routarr`.
    pub source: &'static str,
    /// Always `Routarr`.
    pub title: &'static str,
    /// A sentence in English for a person.
    pub message: String,
    /// The message again, where Discord reads it.
    pub content: String,
    /// The message again, where Apprise reads it.
    pub body: String,
    /// Ids and counts for a program: the instance, the simulation, how many
    /// titles moved.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// How long a failed delivery waits before each new attempt. The first
/// attempt is made at once, and the last is followed by none.
#[cfg(not(test))]
const RETRIES: [Duration; 3] =
    [Duration::from_secs(10), Duration::from_secs(60), Duration::from_secs(300)];
#[cfg(test)]
const RETRIES: [Duration; 3] =
    [Duration::from_millis(10), Duration::from_millis(20), Duration::from_millis(30)];

/// Deliveries waiting to be tried again. A webhook down for a while holds one
/// task per event, so the count is bounded, and an event past it is logged
/// and dropped rather than queued without end. Lost on restart.
static RETRYING: Semaphore = Semaphore::const_new(32);

/// Post an event to the configured webhook, if there is one and it asked for
/// this kind.
///
/// Never returns an error and never propagates one: a notification that cannot
/// be delivered must not fail the sync or the apply that produced it. The first
/// attempt is awaited, bounded by the outbound HTTP timeout, and the retries a
/// refusal earns run on a task of their own.
pub async fn send(state: &AppState, event: Event) {
    if webhook_url(state, event.kind()).await.is_none() {
        return;
    }
    if let Some(setting) = event.asked_by()
        && !state.bool_setting(setting, false).await
    {
        debug!(event = event.kind(), "Not asked for by the webhook");
        return;
    }

    let message = event.message();
    let payload = Notification {
        event: event.kind(),
        event_type: event.event_type(),
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        severity: event.severity(),
        source: "routarr",
        title: "Routarr",
        content: message.clone(),
        body: message.clone(),
        message,
        data: event.data(),
    };
    let Ok(body) = serde_json::to_string(&payload) else {
        return;
    };
    let delivery =
        Delivery { id: format!("msg_{}", uuid::Uuid::new_v4().simple()), body, kind: event.kind() };

    let retry_after = match delivery.attempt(state).await {
        Ok(()) => return,
        Err(retry_after) => retry_after,
    };
    let Some(retry_after) = retry_after else {
        return;
    };
    let Ok(waiting) = RETRYING.try_acquire() else {
        warn!(event = delivery.kind, "Too many notifications are waiting, this one is dropped");
        return;
    };
    waiting.forget();
    let state = state.clone();
    tokio::spawn(async move {
        let _released = Released;
        let mut wait = retry_after;
        for delay in RETRIES {
            tokio::time::sleep(wait.map_or(delay, |asked| asked.max(delay))).await;
            match delivery.attempt(&state).await {
                Ok(()) => return,
                Err(Some(asked)) => wait = asked,
                Err(None) => return,
            }
        }
        warn!(event = delivery.kind, "Notification not delivered after every retry");
    });
}

/// Gives back the place a retrying delivery held, however its task ends.
struct Released;

impl Drop for Released {
    fn drop(&mut self) {
        RETRYING.add_permits(1);
    }
}

/// `send`, on a task of its own, for an event a caller must not wait on: the
/// end of an apply answers its report without a webhook's latency.
pub fn send_later(state: &AppState, event: Event) {
    let state = state.clone();
    tokio::spawn(async move { send(&state, event).await });
}

async fn webhook_url(state: &AppState, kind: &str) -> Option<String> {
    let stored = state.setting::<String>("notification_webhook_url", String::new()).await;
    if stored.trim().is_empty() {
        debug!(event = kind, "No notification webhook configured");
        return None;
    }
    match state.secrets.open(stored.trim()) {
        Ok(url) => Some(url),
        Err(e) => {
            warn!(event = kind, "The notification webhook could not be opened: {e}");
            None
        }
    }
}

/// One message, tried as many times as it takes, under one id.
///
/// The address and the secrets are read again at each attempt, not kept from
/// the first: a retry runs minutes later, and an address cleared or a secret
/// replaced in between because it leaked must not be used again.
struct Delivery {
    id: String,
    body: String,
    kind: &'static str,
}

impl Delivery {
    /// One attempt. `Err(Some(wait))` is worth another after at least `wait`,
    /// `Err(None)` is not: a 4xx other than 429 says the receiver refuses this
    /// message, and it will refuse it again, and a cleared address or a secret
    /// that cannot be read has nowhere safe to send it.
    async fn attempt(&self, state: &AppState) -> Result<(), Option<Option<Duration>>> {
        let Some(url) = webhook_url(state, self.kind).await else {
            return Err(None);
        };
        let keys = match signing_keys(state).await {
            Signing::Unsigned => Vec::new(),
            Signing::Keys(keys) => keys,
            // Sent unsigned, the message would pass a receiver that checks a
            // signature only when there is one.
            Signing::Unreadable => {
                error!(
                    event = self.kind,
                    "The signing secret cannot be opened with this installation's key, so the \
                     notification is not sent. Replace the secret in Settings."
                );
                return Err(None);
            }
        };
        // Signed at each attempt: a receiver refuses a timestamp too old, and a
        // retry five minutes on would carry one.
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let mut request = state
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("webhook-id", &self.id)
            .header("webhook-timestamp", &timestamp)
            .body(self.body.clone());
        if !keys.is_empty() {
            request =
                request.header("webhook-signature", sign(&keys, &self.id, &timestamp, &self.body));
        }
        // Through `send_ok`, which says why a send failed without the address:
        // a Discord or Slack webhook URL carries its secret in the path.
        match send_ok("Notification webhook", request).await {
            Ok(()) => {
                debug!(event = self.kind, "Notification delivered");
                Ok(())
            }
            Err(e) => {
                warn!(event = self.kind, "Notification not delivered: {e}");
                Err(match e {
                    AppError::ExternalApi { status, retry_after, .. }
                        if status == 0 || status == 429 || status >= 500 =>
                    {
                        Some(retry_after.map(Duration::from_secs))
                    }
                    _ => None,
                })
            }
        }
    }
}

/// `webhook-signature`: one `v1,<base64>` per key, space separated, each an
/// HMAC-SHA256 of `id.timestamp.body`.
pub fn sign(keys: &[Vec<u8>], id: &str, timestamp: &str, body: &str) -> String {
    keys.iter()
        .filter_map(|key| {
            let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
            mac.update(format!("{id}.{timestamp}.{body}").as_bytes());
            Some(format!("v1,{}", B64.encode(mac.finalize().into_bytes())))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a notification is signed with.
enum Signing {
    /// No secret: the notification goes out unsigned, as the owner chose.
    Unsigned,
    /// The newest secret, and the one it replaced for a day after, so a
    /// receiver can be updated without a gap.
    Keys(Vec<Vec<u8>>),
    /// A secret is set and the newest cannot be opened, as after a restore
    /// beside another master key.
    Unreadable,
}

async fn signing_keys(state: &AppState) -> Signing {
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT secret, created_at > datetime('now', '-1 day')
           FROM webhook_secrets ORDER BY created_at DESC, rowid DESC LIMIT 2",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let open = |sealed: &str| {
        state.secrets.open(sealed).ok().and_then(|secret| crate::crypto::signing_key(&secret))
    };
    let Some((newest, fresh)) = rows.first() else {
        return Signing::Unsigned;
    };
    let Some(newest) = open(newest) else {
        return Signing::Unreadable;
    };
    let replaced = rows.get(1).filter(|_| *fresh).and_then(|(sealed, _)| open(sealed));
    Signing::Keys(std::iter::once(newest).chain(replaced).collect())
}

/// Whether notifications are signed, and since when.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SigningStatus {
    pub signed: bool,
    pub since: Option<String>,
    /// False when a secret is set and cannot be opened with this
    /// installation's key: nothing is sent until it is replaced.
    pub readable: bool,
}

pub async fn signing_status(state: &AppState) -> AppResult<SigningStatus> {
    let since: Option<String> = sqlx::query_scalar("SELECT MAX(created_at) FROM webhook_secrets")
        .fetch_one(&state.pool)
        .await?;
    let readable = !matches!(signing_keys(state).await, Signing::Unreadable);
    Ok(SigningStatus { signed: since.is_some(), since, readable })
}

/// Make a new secret, keep the one it replaces for a day, and return it once.
pub async fn rotate_signing_secret(state: &AppState) -> AppResult<String> {
    let secret = crate::crypto::generate_signing_secret()?;
    let mut tx = state.pool.begin().await?;
    // Only the newest stays beside the new one: the one before it had its day.
    sqlx::query(
        "DELETE FROM webhook_secrets WHERE rowid NOT IN
           (SELECT rowid FROM webhook_secrets ORDER BY created_at DESC, rowid DESC LIMIT 1)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO webhook_secrets (secret) VALUES (?)")
        .bind(state.secrets.seal(&secret)?)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(secret)
}

/// Stop signing.
pub async fn remove_signing_secrets(state: &AppState) -> AppResult<()> {
    sqlx::query("DELETE FROM webhook_secrets").execute(&state.pool).await?;
    Ok(())
}
