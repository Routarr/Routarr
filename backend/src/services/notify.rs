//! Telling the operator, or another application, what happened while nobody
//! was looking.
//!
//! Routarr does most of its work on a schedule or on a webhook, and in a
//! homelab nobody opens the interface daily. A scheduled sync that has been
//! failing for a week, or an automatic apply that Radarr rejected, is invisible
//! until someone thinks to look.
//!
//! One address, written to in the shape its receiver reads (`Format`): Discord
//! an embed coloured by the outcome, ntfy a text with its title, priority and
//! tags in headers, Gotify a message with a priority, Apprise its own fields.
//! Any other receiver gets Routarr's JSON (`Notification`), which carries the
//! same text under the names Discord, Gotify and Apprise read, and the ids and
//! counts a program parses. With a signing secret every shape is signed as
//! Standard Webhooks specifies, so a receiver can tell it came from Routarr.

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
use crate::localization::Localizer;
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
    /// An unattended pass found more moves than one run may make, and made
    /// none. Sent on the transition only.
    AutoApplyHeld { candidates: usize, cap: usize },
    /// A sync failed, every time it does. Asked for by `notify_sync_failed`.
    SyncFailed { instance_id: String, instance: String, error: String },
    /// A simulation stored its proposals. Asked for by
    /// `notify_simulation_completed`.
    SimulationCompleted { simulation_id: String, total: usize, moves: usize },
    /// An apply or a revert finished. Asked for by `notify_moves_completed`.
    MovesCompleted { reverted: bool, applied: usize, failed: usize, skipped: usize, moving: usize },
    /// Sent from Settings to check the address and the format.
    Test,
}

impl Event {
    fn severity(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. }
            | Event::AutoApplyFailed { .. }
            | Event::SyncFailed { .. } => "error",
            Event::MovesCompleted { failed, .. } if *failed > 0 => "warning",
            Event::AutoApplyHeld { .. } => "warning",
            _ => "info",
        }
    }

    /// The payload's `event`, the one field naming what happened. `type` is
    /// Apprise's, which refuses any value but its four outcomes.
    fn kind(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. } => "instance_unreachable",
            Event::InstanceRecovered { .. } => "instance_recovered",
            Event::AutoApplyFailed { .. } => "auto_apply_failed",
            Event::AutoApplyHeld { .. } => "auto_apply_held",
            Event::SyncFailed { .. } => "sync_failed",
            Event::SimulationCompleted { .. } => "simulation_completed",
            Event::MovesCompleted { reverted: false, .. } => "apply_completed",
            Event::MovesCompleted { reverted: true, .. } => "revert_completed",
            Event::Test => "test",
        }
    }

    /// The outcome as Apprise reads it, which picks the colour and the mark it
    /// shows: info, success, warning or failure.
    fn outcome(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. }
            | Event::AutoApplyFailed { .. }
            | Event::SyncFailed { .. } => "failure",
            Event::MovesCompleted { failed, .. } if *failed > 0 => "warning",
            Event::AutoApplyHeld { .. } => "warning",
            Event::InstanceRecovered { .. } | Event::MovesCompleted { .. } => "success",
            Event::SimulationCompleted { .. } | Event::Test => "info",
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
            Event::AutoApplyHeld { candidates, cap } => format!(
                "Routarr held back {candidates} automatic moves, more than the {cap} one \
                 unattended run may make. Apply them from the Simulation screen, or raise the \
                 batch limit."
            ),
            Event::SyncFailed { instance, error, .. } => {
                format!("Routarr could not sync '{instance}': {error}")
            }
            Event::SimulationCompleted { total, moves, .. } => {
                format!("Routarr simulated the library. Titles: {total}, moves proposed: {moves}.")
            }
            Event::MovesCompleted { reverted, applied, failed, skipped, moving } => format!(
                "Routarr {} titles. Moved: {applied}, still moving: {moving}, failed: {failed}, \
                 skipped: {skipped}.",
                if *reverted { "moved back" } else { "moved" }
            ),
            Event::Test => {
                "Routarr reaches this address. Its notifications arrive here like this one."
                    .to_string()
            }
        }
    }

    /// What happened in a few words, the title a receiver shows above the
    /// message. English, like the message.
    fn headline(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. } => "Instance unreachable",
            Event::InstanceRecovered { .. } => "Instance reachable again",
            Event::AutoApplyFailed { .. } => "Automatic apply failed",
            Event::AutoApplyHeld { .. } => "Automatic apply held back",
            Event::SyncFailed { .. } => "Sync failed",
            Event::SimulationCompleted { .. } => "Simulation finished",
            Event::MovesCompleted { reverted: false, .. } => "Apply finished",
            Event::MovesCompleted { reverted: true, .. } => "Revert finished",
            Event::Test => "Test notification",
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
            Event::AutoApplyHeld { candidates, cap } => {
                serde_json::json!({ "candidates": candidates, "cap": cap })
            }
            Event::SyncFailed { instance_id, instance, .. } => {
                serde_json::json!({ "instance_id": instance_id, "instance": instance })
            }
            Event::SimulationCompleted { simulation_id, total, moves } => serde_json::json!({
                "simulation_id": simulation_id, "total": total, "moves": moves
            }),
            Event::MovesCompleted { applied, failed, skipped, moving, .. } => serde_json::json!({
                "applied": applied, "failed": failed, "skipped": skipped, "moving": moving
            }),
            Event::Test => serde_json::json!({}),
        }
    }
}

/// The values `notification_format` may hold: `auto` reads the format off the
/// address, the others name it.
pub const FORMATS: [&str; 6] = ["auto", "json", "discord", "ntfy", "gotify", "apprise"];

/// The shape a notification is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Discord,
    Ntfy,
    Gotify,
    Apprise,
}

impl Format {
    /// The format `setting` names, or under `auto` the one `url` shows.
    pub fn of(setting: &str, url: &str) -> Format {
        match setting {
            "json" => Format::Json,
            "discord" => Format::Discord,
            "ntfy" => Format::Ntfy,
            "gotify" => Format::Gotify,
            "apprise" => Format::Apprise,
            _ => Format::recognised(url),
        }
    }

    /// A Discord webhook and a topic on ntfy.sh, known by their host. Gotify,
    /// Apprise and a server of one's own serve any address, which says nothing
    /// of what answers it, so they get Routarr's JSON, which they read too.
    fn recognised(url: &str) -> Format {
        let Ok(url) = reqwest::Url::parse(url) else {
            return Format::Json;
        };
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let discord = ["discord.com", "discordapp.com", "ptb.discord.com", "canary.discord.com"];
        if discord.contains(&host.as_str()) && url.path().starts_with("/api/webhooks/") {
            return Format::Discord;
        }
        // A templated topic reads `title` and `message` out of the JSON.
        let templated = url.query_pairs().any(|(key, _)| key == "template" || key == "tpl");
        if host == "ntfy.sh" && !templated {
            return Format::Ntfy;
        }
        Format::Json
    }
}

/// What is posted: the body, its type, and the headers ntfy reads its
/// title, priority and tags from.
struct Payload {
    body: String,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
}

/// The longest description a Discord embed takes, in characters.
const DISCORD_DESCRIPTION: usize = 4096;
/// The longest body ntfy shows as text, in bytes. A longer one becomes an
/// attachment nobody reads in the notification.
const NTFY_BODY: usize = 4096;

/// `text` cut to at most `max` of what `size` counts, on a character boundary.
fn clipped(text: &str, max: usize, size: impl Fn(char) -> usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += size(*c);
            used <= max
        })
        .collect()
}

fn payload(event: &Event, at: &str, format: Format) -> Payload {
    let json = |value: serde_json::Value| Payload {
        body: value.to_string(),
        content_type: "application/json",
        headers: Vec::new(),
    };
    match format {
        Format::Json => Payload {
            body: serde_json::to_string(&notification(event, at)).unwrap_or_default(),
            content_type: "application/json",
            headers: Vec::new(),
        },
        Format::Discord => json(serde_json::json!({
            "username": "Routarr",
            "embeds": [{
                "title": event.headline(),
                "description": clipped(&event.message(), DISCORD_DESCRIPTION, |_| 1),
                "color": event.colour(),
                "timestamp": at,
                "footer": { "text": event.kind() },
            }],
            "allowed_mentions": AllowedMentions::default(),
        })),
        Format::Ntfy => Payload {
            body: clipped(&event.message(), NTFY_BODY, char::len_utf8),
            content_type: "text/plain; charset=utf-8",
            headers: vec![
                ("Title", event.headline().to_string()),
                ("Priority", event.ntfy_priority().to_string()),
                ("Tags", event.ntfy_tag().to_string()),
            ],
        },
        Format::Gotify => json(serde_json::json!({
            "title": event.headline(),
            "message": event.message(),
            "priority": event.gotify_priority(),
        })),
        Format::Apprise => json(serde_json::json!({
            "title": event.headline(),
            "body": event.message(),
            "type": event.outcome(),
            "format": "text",
        })),
    }
}

impl Event {
    /// Discord's embed colour: red for a failure, amber for a partial apply,
    /// green for a recovery or a clean run, blue for the rest.
    fn colour(&self) -> u32 {
        match self.outcome() {
            "failure" => 0xD7_3A_49,
            "warning" => 0xE3_A0_08,
            "success" => 0x2E_A0_43,
            _ => 0x3B_82_F6,
        }
    }

    /// ntfy's priority, from 1 to 5: a failure rings, a finished run does not.
    fn ntfy_priority(&self) -> u8 {
        match self.severity() {
            "error" => 4,
            "warning" => 3,
            _ => 2,
        }
    }

    /// The emoji ntfy shows beside the title, named as ntfy names it.
    fn ntfy_tag(&self) -> &'static str {
        match self.outcome() {
            "failure" => "rotating_light",
            "warning" => "warning",
            "success" => "white_check_mark",
            _ => "information_source",
        }
    }

    /// Gotify's priority, from 0 to 10. Its Android client sounds from 4 and
    /// shows a failure on screen from 8.
    fn gotify_priority(&self) -> u8 {
        match self.severity() {
            "error" => 8,
            "warning" => 5,
            _ => 2,
        }
    }
}

/// What the notification webhook receives.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Notification {
    /// What happened, in snake case: `instance_unreachable`,
    /// `instance_recovered`, `auto_apply_failed`, `sync_failed`,
    /// `simulation_completed`, `apply_completed`, `revert_completed`, or
    /// `test` when sent from Settings.
    pub event: &'static str,
    /// When it happened, in RFC 3339.
    pub timestamp: String,
    /// `error`, `warning` or `info`.
    pub severity: &'static str,
    /// The outcome as Apprise reads it: `failure`, `warning`, `success` or
    /// `info`.
    #[serde(rename = "type")]
    #[schema(rename = "type")]
    pub outcome: &'static str,
    /// Always `routarr`.
    pub source: &'static str,
    /// Always `Routarr`.
    pub title: &'static str,
    /// A sentence in English for a person.
    pub message: String,
    /// The message again, where Discord reads it.
    pub content: String,
    /// Always `{"parse": []}`: Discord pings nobody a title names, an
    /// `@everyone` in it included.
    pub allowed_mentions: AllowedMentions,
    /// The message again, where Apprise reads it.
    pub body: String,
    /// Ids and counts for a program: the instance, the simulation, how many
    /// titles moved.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

fn notification(event: &Event, at: &str) -> Notification {
    let message = event.message();
    Notification {
        event: event.kind(),
        timestamp: at.to_string(),
        severity: event.severity(),
        outcome: event.outcome(),
        source: "routarr",
        title: "Routarr",
        content: message.clone(),
        allowed_mentions: AllowedMentions::default(),
        body: message.clone(),
        message,
        data: event.data(),
    }
}

/// What Discord may ping for a message: nothing.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct AllowedMentions {
    /// Always empty.
    pub parse: Vec<String>,
}

/// How long a failed delivery waits before each new attempt. The first
/// attempt is made at once, and the last is followed by none.
#[cfg(not(test))]
const RETRIES: [Duration; 3] =
    [Duration::from_secs(10), Duration::from_secs(60), Duration::from_secs(300)];
#[cfg(test)]
const RETRIES: [Duration; 3] =
    [Duration::from_millis(10), Duration::from_millis(20), Duration::from_millis(30)];

/// The notifications of one installation, sent one at a time.
pub struct Queue {
    /// Held through a delivery and its retries: a newer event never lands
    /// before an older one still being retried, which would leave a channel
    /// on the wrong state.
    turn: tokio::sync::Mutex<()>,
    /// Deliveries being sent or waiting their turn. A webhook down for a
    /// while holds one per event, so the count is bounded, and an event past
    /// it is logged and dropped rather than queued without end. Lost on
    /// restart.
    pending: Semaphore,
}

impl Default for Queue {
    fn default() -> Self {
        Self { turn: tokio::sync::Mutex::new(()), pending: Semaphore::new(32) }
    }
}

/// Post an event to the configured webhook, if there is one and it asked for
/// this kind.
///
/// Never returns an error and never propagates one: a notification that cannot
/// be delivered must not fail the sync or the apply that produced it. It
/// waits its turn behind the deliveries before it, retries included, so a
/// caller that must not wait sends through [`send_later`].
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

    let delivery = Delivery::of(event);
    let Ok(_pending) = state.notifications.pending.try_acquire() else {
        warn!(event = delivery.kind(), "Too many notifications are waiting, this one is dropped");
        return;
    };
    let _turn = state.notifications.turn.lock().await;
    let mut wait = match delivery.attempt(state).await.map_err(|failed| failed.again()) {
        Ok(()) | Err(None) => return,
        Err(Some(asked)) => asked,
    };
    for delay in RETRIES {
        tokio::time::sleep(wait.map_or(delay, |asked| asked.max(delay))).await;
        match delivery.attempt(state).await.map_err(|failed| failed.again()) {
            Ok(()) | Err(None) => return,
            Err(Some(asked)) => wait = asked,
        }
    }
    warn!(event = delivery.kind(), "Notification not delivered after every retry");
}

/// Send [`Event::Test`] once, at once, and say why it did not arrive. Past
/// the queue: the operator waits on the answer, and a delivery being retried
/// would hold it for minutes.
pub async fn send_test(state: &AppState, localizer: &Localizer) -> AppResult<()> {
    match Delivery::of(Event::Test).attempt(state).await {
        Ok(()) => Ok(()),
        Err(Undelivered::NoAddress) => {
            Err(AppError::BadRequest(localizer.translate("NotificationTestNoAddress", &[])))
        }
        Err(Undelivered::SecretUnreadable) => {
            Err(AppError::Conflict(localizer.translate("SigningSecretUnreadable", &[])))
        }
        Err(Undelivered::Failed { error: AppError::ExternalApi { status, .. }, .. })
            if status != 0 =>
        {
            Err(AppError::UpstreamDown(
                localizer.translate("NotificationTestRefused", &[("status", &status.to_string())]),
            ))
        }
        Err(Undelivered::Failed { .. }) => {
            Err(AppError::UpstreamDown(localizer.translate("NotificationTestUnreachable", &[])))
        }
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
/// The address, the format and the secrets are read again at each attempt,
/// not kept from the first: a retry runs minutes later, and an address
/// cleared or a secret replaced in between because it leaked must not be used
/// again.
struct Delivery {
    id: String,
    event: Event,
    /// When the event happened, in RFC 3339, the same at every attempt.
    at: String,
}

/// Why an attempt did not deliver.
enum Undelivered {
    /// The address was cleared.
    NoAddress,
    /// Sent unsigned, the message would pass a receiver that checks a
    /// signature only when there is one.
    SecretUnreadable,
    /// The receiver refused or did not answer. `again` holds the wait before
    /// another attempt, the receiver's own when it named one, and is `None`
    /// for a 4xx other than 429: the receiver refuses this message and will
    /// refuse it again.
    Failed { error: AppError, again: Option<Option<Duration>> },
}

impl Undelivered {
    fn again(self) -> Option<Option<Duration>> {
        match self {
            Undelivered::Failed { again, .. } => again,
            Undelivered::NoAddress | Undelivered::SecretUnreadable => None,
        }
    }
}

impl Delivery {
    fn of(event: Event) -> Self {
        Self {
            id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            event,
        }
    }

    fn kind(&self) -> &'static str {
        self.event.kind()
    }

    async fn attempt(&self, state: &AppState) -> Result<(), Undelivered> {
        let kind = self.kind();
        let Some(url) = webhook_url(state, kind).await else {
            return Err(Undelivered::NoAddress);
        };
        // A read that fails is no answer to whether the owner signs: sent
        // unsigned, the message would pass a receiver that checks a signature
        // only when one is there. It is tried again later.
        let signing = signing_keys(state)
            .await
            .map_err(|error| Undelivered::Failed { error, again: Some(None) })?;
        let keys = match signing {
            Signing::Unsigned => Vec::new(),
            Signing::Keys(keys) => keys,
            Signing::Unreadable => {
                error!(
                    event = kind,
                    "The signing secret cannot be opened with this installation's key, so the \
                     notification is not sent. Replace the secret in Settings."
                );
                return Err(Undelivered::SecretUnreadable);
            }
        };
        let format = state.setting::<String>("notification_format", "auto".into()).await;
        let payload = payload(&self.event, &self.at, Format::of(&format, &url));
        // Signed at each attempt: a receiver refuses a timestamp too old, and a
        // retry five minutes on would carry one.
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let mut request = state
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, payload.content_type)
            .header("webhook-id", &self.id)
            .header("webhook-timestamp", &timestamp);
        for (name, value) in &payload.headers {
            request = request.header(*name, value);
        }
        if !keys.is_empty() {
            request = request
                .header("webhook-signature", sign(&keys, &self.id, &timestamp, &payload.body));
        }
        // Through `send_ok`, which says why a send failed without the address:
        // a Discord or Slack webhook URL carries its secret in the path.
        match send_ok("Notification webhook", request.body(payload.body)).await {
            Ok(()) => {
                debug!(event = kind, "Notification delivered");
                Ok(())
            }
            Err(error) => {
                warn!(event = kind, "Notification not delivered: {error}");
                let again = match &error {
                    AppError::ExternalApi { status, retry_after, .. }
                        if *status == 0 || *status == 429 || *status >= 500 =>
                    {
                        Some(retry_after.map(Duration::from_secs))
                    }
                    _ => None,
                };
                Err(Undelivered::Failed { error, again })
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

async fn signing_keys(state: &AppState) -> AppResult<Signing> {
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT secret, created_at > datetime('now', '-1 day')
           FROM webhook_secrets ORDER BY created_at DESC, rowid DESC LIMIT 2",
    )
    .fetch_all(&state.pool)
    .await?;
    let open = |sealed: &str| {
        state.secrets.open(sealed).ok().and_then(|secret| crate::crypto::signing_key(&secret))
    };
    let Some((newest, fresh)) = rows.first() else {
        return Ok(Signing::Unsigned);
    };
    let Some(newest) = open(newest) else {
        return Ok(Signing::Unreadable);
    };
    let replaced = rows.get(1).filter(|_| *fresh).and_then(|(sealed, _)| open(sealed));
    Ok(Signing::Keys(std::iter::once(newest).chain(replaced).collect()))
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
    let readable = !matches!(signing_keys(state).await?, Signing::Unreadable);
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
    let replaced = crate::services::backup::Credential::SigningSecret;
    crate::services::backup::set_withdrawn(&mut *tx, replaced, false).await?;
    tx.commit().await?;
    Ok(secret)
}

/// Stop signing.
pub async fn remove_signing_secrets(state: &AppState) -> AppResult<()> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM webhook_secrets").execute(&mut *tx).await?;
    let removed = crate::services::backup::Credential::SigningSecret;
    crate::services::backup::set_withdrawn(&mut *tx, removed, true).await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod receivers {
    use super::*;

    /// One of each event, every variant the payload can carry.
    fn every_event() -> Vec<Event> {
        vec![
            Event::InstanceUnreachable { instance: "Radarr".into(), error: "refused".into() },
            Event::InstanceRecovered { instance: "Radarr".into() },
            Event::AutoApplyFailed { failed: 1, applied: 0, first_error: "Totoro: no".into() },
            Event::AutoApplyHeld { candidates: 60, cap: 50 },
            Event::SyncFailed {
                instance_id: "inst-1".into(),
                instance: "Radarr".into(),
                error: "refused".into(),
            },
            Event::SimulationCompleted { simulation_id: "s-1".into(), total: 3, moves: 1 },
            Event::MovesCompleted { reverted: false, applied: 1, failed: 1, skipped: 0, moving: 0 },
            Event::MovesCompleted { reverted: true, applied: 1, failed: 0, skipped: 0, moving: 0 },
            Event::Test,
        ]
    }

    const AT: &str = "2026-10-04T12:00:00Z";

    fn sent(event: &Event) -> serde_json::Value {
        serde_json::to_value(notification(event, AT)).unwrap()
    }

    fn posted(event: &Event, format: Format) -> serde_json::Value {
        serde_json::from_str(&payload(event, AT, format).body).unwrap()
    }

    /// Each event names itself, says how serious it is, and carries the ids
    /// and counts a program reads, never prose.
    #[test]
    fn each_event_carries_its_name_its_severity_and_its_data() {
        use serde_json::json;
        let expected = [
            ("instance_unreachable", "error", json!({ "instance": "Radarr" })),
            ("instance_recovered", "info", json!({ "instance": "Radarr" })),
            ("auto_apply_failed", "error", json!({ "failed": 1, "applied": 0 })),
            ("auto_apply_held", "warning", json!({ "candidates": 60, "cap": 50 })),
            ("sync_failed", "error", json!({ "instance_id": "inst-1", "instance": "Radarr" })),
            (
                "simulation_completed",
                "info",
                json!({ "simulation_id": "s-1", "total": 3, "moves": 1 }),
            ),
            (
                "apply_completed",
                "warning",
                json!({ "applied": 1, "failed": 1, "skipped": 0, "moving": 0 }),
            ),
            (
                "revert_completed",
                "info",
                json!({ "applied": 1, "failed": 0, "skipped": 0, "moving": 0 }),
            ),
            ("test", "info", json!({})),
        ];
        for (event, (name, severity, data)) in every_event().iter().zip(expected) {
            let sent = sent(event);
            assert_eq!(sent["event"], name, "{sent}");
            assert_eq!(sent["severity"], severity, "{sent}");
            assert_eq!(sent["data"], data, "{sent}");
        }
    }

    /// Apprise API refuses with a 400 a `type` outside info, success, warning
    /// and failure, a `format` outside text, markdown and html, and a request
    /// with no `body` (apprise-api `views.py`). Within those four, `type` picks
    /// the colour and the mark: a failure red, a partial apply amber, a
    /// recovery and a clean run green.
    #[test]
    fn apprise_api_accepts_routarrs_json_and_shows_its_outcome() {
        let outcomes = [
            "failure", "success", "failure", "warning", "failure", "info", "warning", "success",
            "info",
        ];
        let events = every_event();
        assert_eq!(events.len(), outcomes.len());
        for (event, outcome) in events.iter().zip(outcomes) {
            let sent = sent(event);
            assert_eq!(sent["type"], outcome, "{sent}");
            let format = sent.get("format").map(|format| format.as_str().unwrap_or_default());
            assert!(format.is_none_or(|f| ["text", "markdown", "html"].contains(&f)), "{sent}");
            assert!(sent["body"].as_str().is_some_and(|body| !body.is_empty()), "{sent}");
        }
    }

    /// A title is whatever the Arr holds, `@everyone` included, and Discord
    /// pings what a message names unless `allowed_mentions` says otherwise.
    #[test]
    fn a_title_naming_everyone_pings_nobody_on_discord() {
        let event = Event::InstanceRecovered { instance: "@everyone @here <@&1>".into() };
        let sent = sent(&event);
        assert!(sent["content"].as_str().unwrap().contains("@everyone"), "{sent}");
        assert_eq!(sent["allowed_mentions"], serde_json::json!({ "parse": [] }), "{sent}");
    }

    /// Discord refuses a message with no `content` (nor embed) and one over
    /// 2000 characters.
    #[test]
    fn discord_accepts_routarrs_json_for_every_event() {
        for event in every_event() {
            let sent = sent(&event);
            let content = sent["content"].as_str().unwrap_or_default();
            assert!(!content.is_empty() && content.chars().count() <= 2000, "{sent}");
        }
    }

    /// Gotify refuses a message with no `message`, and a `priority` or a
    /// `title` of the wrong type.
    #[test]
    fn gotify_accepts_routarrs_json_for_every_event() {
        for event in every_event() {
            let sent = sent(&event);
            assert!(sent["message"].as_str().is_some_and(|text| !text.is_empty()), "{sent}");
            assert!(sent.get("priority").is_none_or(serde_json::Value::is_i64), "{sent}");
            assert!(sent.get("title").is_none_or(serde_json::Value::is_string), "{sent}");
        }
    }

    /// ntfy publishes any body posted to a topic as its text, and turns one
    /// over 4096 bytes into an attachment nobody reads in the notification.
    #[test]
    fn ntfy_shows_routarrs_json_for_every_event_as_text() {
        for event in every_event() {
            let sent = serde_json::to_string(&notification(&event, AT)).unwrap();
            assert!(sent.len() <= 4096, "{} bytes", sent.len());
        }
    }

    #[test]
    fn discord_and_ntfy_addresses_are_recognised_and_others_get_json() {
        let cases = [
            ("https://discord.com/api/webhooks/1/abc", Format::Discord),
            ("https://discordapp.com/api/webhooks/1/abc", Format::Discord),
            ("https://canary.discord.com/api/webhooks/1/abc", Format::Discord),
            ("https://discord.com/channels/1", Format::Json),
            ("https://ntfy.sh/routarr", Format::Ntfy),
            ("https://ntfy.sh/routarr?template=yes&message={{.message}}", Format::Json),
            ("https://ntfy.example.org/routarr", Format::Json),
            ("https://gotify.example.org/message?token=abc", Format::Json),
            ("not an address", Format::Json),
        ];
        for (url, format) in cases {
            assert_eq!(Format::of("auto", url), format, "{url}");
        }
    }

    #[test]
    fn a_format_named_in_settings_wins_over_the_address() {
        assert_eq!(Format::of("gotify", "https://ntfy.sh/routarr"), Format::Gotify);
        assert_eq!(Format::of("json", "https://discord.com/api/webhooks/1/a"), Format::Json);
        assert_eq!(Format::of("ntfy", "https://push.example.org/routarr"), Format::Ntfy);
        assert_eq!(Format::of("apprise", "https://apprise.example.org/notify/k"), Format::Apprise);
    }

    /// Discord refuses an embed without a title or a description, and one
    /// over 4096 characters, and pings what a message names unless
    /// `allowed_mentions` says otherwise.
    #[test]
    fn discord_gets_an_embed_coloured_by_the_outcome() {
        for event in every_event() {
            let sent = posted(&event, Format::Discord);
            let embed = &sent["embeds"][0];
            assert_eq!(embed["title"], event.headline(), "{sent}");
            let description = embed["description"].as_str().unwrap_or_default();
            assert!(!description.is_empty() && description.chars().count() <= 4096, "{sent}");
            assert_eq!(sent["allowed_mentions"], serde_json::json!({ "parse": [] }), "{sent}");
        }
        let colour = |event: &Event| posted(event, Format::Discord)["embeds"][0]["color"].clone();
        let failed = colour(&every_event()[0]);
        let recovered = colour(&every_event()[1]);
        assert_ne!(failed, recovered);
    }

    /// ntfy reads its title, priority and tags from headers, and shows the
    /// body as the message. A failure rings, a finished run stays quiet.
    #[test]
    fn ntfy_gets_the_message_as_text_and_the_rest_in_headers() {
        let header = |payload: &Payload, name: &str| {
            payload.headers.iter().find(|(key, _)| *key == name).map(|(_, value)| value.clone())
        };
        let unreachable = &every_event()[0];
        let sent = payload(unreachable, AT, Format::Ntfy);
        assert_eq!(sent.body, unreachable.message());
        assert!(sent.content_type.starts_with("text/plain"));
        assert_eq!(header(&sent, "Title").as_deref(), Some("Instance unreachable"));
        assert_eq!(header(&sent, "Priority").as_deref(), Some("4"));
        assert_eq!(header(&sent, "Tags").as_deref(), Some("rotating_light"));

        let finished = payload(&every_event()[7], AT, Format::Ntfy);
        assert_eq!(header(&finished, "Priority").as_deref(), Some("2"));
        assert_eq!(header(&finished, "Tags").as_deref(), Some("white_check_mark"));
    }

    /// An error is whatever the Arr answered, of any length, and each
    /// receiver drops or hides what passes its limit.
    #[test]
    fn a_long_error_is_cut_to_what_discord_and_ntfy_show() {
        let event = Event::SyncFailed {
            instance_id: "inst-1".into(),
            instance: "Radarr".into(),
            error: "é".repeat(5_000),
        };
        let discord = posted(&event, Format::Discord);
        let description = discord["embeds"][0]["description"].as_str().unwrap();
        assert_eq!(description.chars().count(), 4096);
        let ntfy = payload(&event, AT, Format::Ntfy).body;
        assert!(ntfy.len() <= 4096 && ntfy.len() > 4000, "{} bytes", ntfy.len());
    }

    /// Gotify's Android client is silent below 4 and puts a message on
    /// screen from 8.
    #[test]
    fn gotify_gets_a_priority_by_severity() {
        let priorities: Vec<i64> = every_event()
            .iter()
            .map(|event| posted(event, Format::Gotify)["priority"].as_i64().unwrap())
            .collect();
        assert_eq!(priorities, [8, 2, 8, 5, 8, 2, 5, 2, 2]);
        for event in every_event() {
            let sent = posted(&event, Format::Gotify);
            assert!(sent["message"].as_str().is_some_and(|text| !text.is_empty()), "{sent}");
            assert!(sent["title"].is_string(), "{sent}");
        }
    }

    #[test]
    fn apprise_gets_only_the_fields_it_reads() {
        for event in every_event() {
            let sent = posted(&event, Format::Apprise);
            let fields: Vec<&str> = sent.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(fields, ["body", "format", "title", "type"], "{sent}");
            assert_eq!(sent["type"], event.outcome());
            assert_eq!(sent["format"], "text");
        }
    }
}
