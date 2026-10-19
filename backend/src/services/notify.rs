//! Telling the operator, or another application, what happened while nobody was looking. One
//! address gets the shape its receiver reads (`Format`), Routarr's JSON (`Notification`) when
//! unknown, signed as Standard Webhooks specifies when a signing secret is set.

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
use crate::services::executor::Moved;
use crate::state::AppState;

/// Something worth telling. A failure is sent whenever a webhook is set, a success only when
/// its setting asks, so a channel a person reads does not fill up. An Arr's `error` is stored
/// text, said in the delivery's language (`localization::said`).
#[derive(Debug, Clone)]
pub enum Event {
    /// A sync could not reach an Arr. Sent on the transition only.
    InstanceUnreachable { instance: String, error: String },
    /// The same instance answered again.
    InstanceRecovered { instance: String },
    /// An unattended apply wrote to an Arr and the Arr refused. `title` and
    /// `error` are the first refusal.
    AutoApplyFailed { failed: usize, applied: usize, title: String, error: String },
    /// An unattended pass found more moves than one run may make, and made
    /// none. Sent on the transition only.
    AutoApplyHeld { instance: String, candidates: usize, cap: usize },
    /// A run found rule tests failing where none did, which holds auto-apply.
    RuleTestsFailing { failed: usize, total: usize },
    /// A sync failed, every time it does. Asked for by `notify_sync_failed`.
    SyncFailed { instance_id: String, instance: String, error: String },
    /// A simulation stored its proposals. Asked for by `notify_simulation_completed`.
    SimulationCompleted { simulation_id: String, total: usize, moves: usize },
    /// An apply or a revert finished in task `task_id`, naming its first titles. Asked for by
    /// `notify_moves_completed`.
    MovesCompleted {
        reverted: bool,
        applied: usize,
        failed: usize,
        skipped: usize,
        moving: usize,
        task_id: String,
        moves: Vec<Moved>,
    },
    /// A sync found where the moves an apply left `moving` ended, `total` of them, the first
    /// named. Asked for by `notify_moves_completed`, so a media server scans once the files are
    /// there.
    MovesSettled { total: usize, moves: Vec<Moved> },
    /// A scheduled backup could not be written, every time it fails. `error`
    /// is what the system answered, as it answered it.
    BackupFailed { error: String },
    /// Sent from Settings to check the address and the format.
    Test,
}

impl Event {
    fn severity(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. }
            | Event::AutoApplyFailed { .. }
            | Event::SyncFailed { .. }
            | Event::BackupFailed { .. } => "error",
            Event::MovesCompleted { failed, .. } if *failed > 0 => "warning",
            Event::MovesSettled { moves, .. } if failed_among(moves) > 0 => "warning",
            Event::AutoApplyHeld { .. } | Event::RuleTestsFailing { .. } => "warning",
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
            Event::RuleTestsFailing { .. } => "rule_tests_failing",
            Event::SyncFailed { .. } => "sync_failed",
            Event::BackupFailed { .. } => "backup_failed",
            Event::SimulationCompleted { .. } => "simulation_completed",
            Event::MovesCompleted { reverted: false, .. } => "apply_completed",
            Event::MovesCompleted { reverted: true, .. } => "revert_completed",
            Event::MovesSettled { .. } => "moves_settled",
            Event::Test => "test",
        }
    }

    /// The outcome as Apprise reads it, which picks the colour and the mark it
    /// shows: info, success, warning or failure.
    fn outcome(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. }
            | Event::AutoApplyFailed { .. }
            | Event::SyncFailed { .. }
            | Event::BackupFailed { .. } => "failure",
            Event::MovesCompleted { failed, .. } if *failed > 0 => "warning",
            Event::MovesSettled { moves, .. } if failed_among(moves) > 0 => "warning",
            Event::AutoApplyHeld { .. } | Event::RuleTestsFailing { .. } => "warning",
            Event::InstanceRecovered { .. }
            | Event::MovesCompleted { .. }
            | Event::MovesSettled { .. } => "success",
            Event::SimulationCompleted { .. } | Event::Test => "info",
        }
    }

    /// The setting a webhook asks for this event with, `None` for a failure,
    /// which is always sent.
    fn asked_by(&self) -> Option<&'static str> {
        match self {
            Event::SyncFailed { .. } => Some("notify_sync_failed"),
            Event::SimulationCompleted { .. } => Some("notify_simulation_completed"),
            Event::MovesCompleted { .. } | Event::MovesSettled { .. } => {
                Some("notify_moves_completed")
            }
            _ => None,
        }
    }

    /// The sentence for a person, in `localizer`'s language.
    fn message(&self, localizer: &Localizer) -> String {
        let said = |stored: &str| crate::localization::said(stored, localizer);
        let count = |n: &usize| n.to_string();
        match self {
            Event::InstanceUnreachable { instance, error } => localizer.translate(
                "NotificationMessageInstanceUnreachable",
                &[("instance", instance), ("error", &said(error))],
            ),
            Event::InstanceRecovered { instance } => localizer
                .translate("NotificationMessageInstanceRecovered", &[("instance", instance)]),
            Event::AutoApplyFailed { failed, applied, title, error } => localizer.translate(
                "NotificationMessageAutoApplyFailed",
                &[
                    ("failed", &count(failed)),
                    ("applied", &count(applied)),
                    ("title", title),
                    ("error", &said(error)),
                ],
            ),
            Event::AutoApplyHeld { instance, candidates, cap } => localizer.translate(
                "NotificationMessageAutoApplyHeld",
                &[("instance", instance), ("candidates", &count(candidates)), ("cap", &count(cap))],
            ),
            Event::RuleTestsFailing { failed, total } => localizer.translate(
                "NotificationMessageRuleTestsFailing",
                &[("failed", &count(failed)), ("total", &count(total))],
            ),
            Event::SyncFailed { instance, error, .. } => localizer.translate(
                "NotificationMessageSyncFailed",
                &[("instance", instance), ("error", &said(error))],
            ),
            Event::BackupFailed { error } => {
                localizer.translate("NotificationMessageBackupFailed", &[("error", error)])
            }
            Event::SimulationCompleted { total, moves, .. } => localizer.translate(
                "NotificationMessageSimulationCompleted",
                &[("total", &count(total)), ("moves", &count(moves))],
            ),
            Event::MovesCompleted { reverted, applied, failed, skipped, moving, moves, .. } => {
                let key = if *reverted {
                    "NotificationMessageRevertCompleted"
                } else {
                    "NotificationMessageApplyCompleted"
                };
                let counts = localizer.translate(
                    key,
                    &[
                        ("applied", &count(applied)),
                        ("moving", &count(moving)),
                        ("failed", &count(failed)),
                        ("skipped", &count(skipped)),
                    ],
                );
                with_titles(counts, moves, applied + failed + moving, localizer)
            }
            Event::MovesSettled { total, moves } => {
                let failed = failed_among(moves);
                let counts = localizer.translate(
                    "NotificationMessageMovesSettled",
                    &[
                        ("applied", &count(&(total.saturating_sub(failed)))),
                        ("failed", &count(&failed)),
                    ],
                );
                with_titles(counts, moves, *total, localizer)
            }
            Event::Test => localizer.translate("NotificationMessageTest", &[]),
        }
    }

    /// What happened in a few words, the title a receiver shows above the
    /// message.
    fn headline(&self, localizer: &Localizer) -> String {
        let key = match self {
            Event::InstanceUnreachable { .. } => "NotificationHeadlineInstanceUnreachable",
            Event::InstanceRecovered { .. } => "NotificationHeadlineInstanceRecovered",
            Event::AutoApplyFailed { .. } => "NotificationHeadlineAutoApplyFailed",
            Event::AutoApplyHeld { .. } => "NotificationHeadlineAutoApplyHeld",
            Event::RuleTestsFailing { .. } => "NotificationHeadlineRuleTestsFailing",
            Event::SyncFailed { .. } => "NotificationHeadlineSyncFailed",
            Event::BackupFailed { .. } => "NotificationHeadlineBackupFailed",
            Event::SimulationCompleted { .. } => "NotificationHeadlineSimulationCompleted",
            Event::MovesCompleted { reverted: false, .. } => "NotificationHeadlineApplyCompleted",
            Event::MovesCompleted { reverted: true, .. } => "NotificationHeadlineRevertCompleted",
            Event::MovesSettled { .. } => "NotificationHeadlineMovesSettled",
            Event::Test => "NotificationHeadlineTest",
        };
        localizer.translate(key, &[])
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
            Event::AutoApplyHeld { instance, candidates, cap } => {
                serde_json::json!({ "instance": instance, "candidates": candidates, "cap": cap })
            }
            Event::RuleTestsFailing { failed, total } => {
                serde_json::json!({ "failed": failed, "total": total })
            }
            Event::SyncFailed { instance_id, instance, .. } => {
                serde_json::json!({ "instance_id": instance_id, "instance": instance })
            }
            Event::SimulationCompleted { simulation_id, total, moves } => serde_json::json!({
                "simulation_id": simulation_id, "total": total, "moves": moves
            }),
            Event::MovesCompleted { applied, failed, skipped, moving, task_id, moves, .. } => {
                let total = applied + failed + moving;
                serde_json::json!({
                    "applied": applied, "failed": failed, "skipped": skipped, "moving": moving,
                    "task_id": task_id, "moves_total": total, "truncated": total > moves.len(),
                    "moves": moves,
                })
            }
            Event::MovesSettled { total, moves } => serde_json::json!({
                "moves_total": total, "truncated": *total > moves.len(), "moves": moves,
            }),
            Event::BackupFailed { .. } | Event::Test => serde_json::json!({}),
        }
    }
}

/// How many of `moves` failed.
fn failed_among(moves: &[Moved]) -> usize {
    moves.iter().filter(|moved| moved.status == "failed").count()
}

/// How many titles a message for a person names, the payload's `moves` carrying the rest.
const TITLES_SAID: usize = 5;

/// `counts`, then the first titles of `moves` and how many more of `total` there are.
fn with_titles(counts: String, moves: &[Moved], total: usize, localizer: &Localizer) -> String {
    let shown: Vec<&str> = moves.iter().take(TITLES_SAID).map(|m| m.title.as_str()).collect();
    if shown.is_empty() {
        return counts;
    }
    let titles = shown.join(&localizer.translate("ListSeparator", &[]));
    let line = match total.saturating_sub(shown.len()) {
        0 => localizer.translate("NotificationTitles", &[("titles", &titles)]),
        more => localizer.translate(
            "NotificationTitlesMore",
            &[("titles", &titles), ("count", &more.to_string())],
        ),
    };
    format!("{counts}\n{line}")
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

fn payload(event: &Event, at: &str, format: Format, localizer: &Localizer) -> Payload {
    let json = |value: serde_json::Value| Payload {
        body: value.to_string(),
        content_type: "application/json",
        headers: Vec::new(),
    };
    match format {
        Format::Json => Payload {
            body: serde_json::to_string(&notification(event, at, localizer)).unwrap_or_default(),
            content_type: "application/json",
            headers: Vec::new(),
        },
        Format::Discord => json(serde_json::json!({
            "username": "Routarr",
            "embeds": [{
                "title": event.headline(localizer),
                "description": clipped(&event.message(localizer), DISCORD_DESCRIPTION, |_| 1),
                "color": event.colour(),
                "timestamp": at,
                "footer": { "text": event.kind() },
            }],
            "allowed_mentions": AllowedMentions::default(),
        })),
        Format::Ntfy => Payload {
            body: clipped(&event.message(localizer), NTFY_BODY, char::len_utf8),
            content_type: "text/plain; charset=utf-8",
            headers: vec![
                ("Title", event.headline(localizer)),
                ("Priority", event.ntfy_priority().to_string()),
                ("Tags", event.ntfy_tag().to_string()),
            ],
        },
        Format::Gotify => json(serde_json::json!({
            "title": event.headline(localizer),
            "message": event.message(localizer),
            "priority": event.gotify_priority(),
        })),
        Format::Apprise => json(serde_json::json!({
            "title": event.headline(localizer),
            "body": event.message(localizer),
            "type": event.outcome(),
            "format": "text",
        })),
    }
}

impl Event {
    /// Discord's embed colour: red for a failure, amber for a warning, green for a success.
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
    /// `instance_recovered`, `auto_apply_failed`, `auto_apply_held`, `rule_tests_failing`,
    /// `sync_failed`, `backup_failed`, `simulation_completed`,
    /// `apply_completed`, `revert_completed`, or `test` when sent from
    /// Settings. The list may grow.
    pub event: &'static str,
    /// When it happened, in RFC 3339.
    #[schema(format = DateTime)]
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
    /// A sentence for a person, in the language set for Routarr's
    /// interface. A program reads `event` and `data`, which never change with
    /// it.
    pub message: String,
    /// The message again, where Discord reads it.
    pub content: String,
    /// Always `{"parse": []}`: Discord pings nobody a title names, an
    /// `@everyone` in it included.
    pub allowed_mentions: AllowedMentions,
    /// The message again, where Apprise reads it.
    pub body: String,
    /// Ids and counts for a program: the instance, the simulation, how many titles moved. For
    /// `apply_completed`, `revert_completed` and `moves_settled` also `moves_total`, `truncated`
    /// and `moves`, the first 50 titles, each `{title, media_type, instance, instance_id, tmdb_id,
    /// tvdb_id, imdb_id, from_path, to_path, status}` with `status` `applied`, `moving` or
    /// `failed` and the paths as the Arr holds them. The two first carry `task_id`, which
    /// `GET /logs?task_id=` lists in full.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

fn notification(event: &Event, at: &str, localizer: &Localizer) -> Notification {
    let message = event.message(localizer);
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
const RETRIES: [Duration; 3] = if cfg!(test) { TESTED_RETRIES } else { SHIPPED_RETRIES };
/// The waits the contract tells a receiver.
const SHIPPED_RETRIES: [Duration; 3] =
    [Duration::from_secs(10), Duration::from_secs(60), Duration::from_secs(300)];
const TESTED_RETRIES: [Duration; 3] =
    [Duration::from_millis(10), Duration::from_millis(20), Duration::from_millis(30)];

/// The notifications of one installation, sent one at a time.
pub struct Queue {
    /// Held through a delivery and its retries: a newer event never lands
    /// before an older one still being retried, which would leave a channel
    /// on the wrong state.
    turn: tokio::sync::Mutex<()>,
    /// Deliveries being sent or waiting their turn, bounded so a webhook down for a while
    /// drops and logs the events past it. Lost on restart.
    pending: Semaphore,
}

impl Default for Queue {
    fn default() -> Self {
        Self { turn: tokio::sync::Mutex::new(()), pending: Semaphore::new(32) }
    }
}

/// Post an event to the configured webhook, if there is one and it asked for this kind. Never
/// fails, so a lost notification never fails the sync or apply behind it. It waits behind the
/// deliveries before it, retries included, so a caller that must not wait uses [`send_later`].
pub async fn send(state: &AppState, event: Event) {
    if webhook_url(state, event.kind()).await.is_none() {
        return;
    }
    if let Some(setting) = event.asked_by()
        && !state.bool_setting(setting).await
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
        Err(Undelivered::SecretUnreadable) => Err(AppError::Conflict {
            reason: "secret_unreadable",
            message: localizer.translate("SigningSecretUnreadable", &[]),
        }),
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
    let stored = state.setting::<String>("notification_webhook_url").await;
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

/// One message, under the same id at every attempt. The address, the format and the secrets
/// are read again at each one: one cleared or replaced meanwhile must not be used.
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
    /// The receiver refused or did not answer. `again` holds the wait before another attempt,
    /// the receiver's own when it named one, and is `None` for a 4xx other than 429.
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
        // Retried later: sent unsigned, it would pass a receiver checking only present signatures.
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
        let format = state.setting::<String>("notification_format").await;
        let localizer = state.localizer().await;
        let payload = payload(&self.event, &self.at, Format::of(&format, &url), &localizer);
        // Signed at each attempt: a receiver refuses a timestamp too old, as a late retry's.
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
        // Through `send_ok`, whose errors omit the address: a webhook URL carries its secret.
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
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
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
            Event::AutoApplyFailed {
                failed: 1,
                applied: 0,
                title: "Totoro".into(),
                error: "no".into(),
            },
            Event::AutoApplyHeld { instance: "Radarr 4K".into(), candidates: 60, cap: 50 },
            Event::RuleTestsFailing { failed: 1, total: 4 },
            Event::SyncFailed {
                instance_id: "inst-1".into(),
                instance: "Radarr".into(),
                error: "refused".into(),
            },
            Event::SimulationCompleted { simulation_id: "s-1".into(), total: 3, moves: 1 },
            Event::MovesCompleted {
                reverted: false,
                applied: 1,
                failed: 1,
                skipped: 0,
                moving: 0,
                task_id: "t-1".into(),
                moves: vec![moved("Akira", "applied"), moved("Totoro", "failed")],
            },
            Event::MovesCompleted {
                reverted: true,
                applied: 1,
                failed: 0,
                skipped: 0,
                moving: 0,
                task_id: "t-2".into(),
                moves: vec![moved("Akira", "applied")],
            },
            Event::MovesSettled { total: 1, moves: vec![moved("Akira", "applied")] },
            Event::BackupFailed { error: "disk full".into() },
            Event::Test,
        ]
    }

    /// A film of `title` moved from the standard folder to the anime one.
    fn moved(title: &str, status: &'static str) -> Moved {
        Moved {
            title: title.into(),
            media_type: "movie".into(),
            instance: Some("Radarr".into()),
            instance_id: "inst-1".into(),
            tmdb_id: Some(149),
            tvdb_id: None,
            imdb_id: Some("tt0094625".into()),
            from_path: Some(format!("/movies/standard/{title} (1988)")),
            to_path: (status != "failed").then(|| format!("/movies/anime/{title} (1988)")),
            status,
        }
    }

    const AT: &str = "2026-10-04T12:00:00Z";

    fn english() -> Localizer {
        Localizer::new("en")
    }

    fn sent(event: &Event) -> serde_json::Value {
        serde_json::to_value(notification(event, AT, &english())).unwrap()
    }

    fn posted(event: &Event, format: Format) -> serde_json::Value {
        serde_json::from_str(&payload(event, AT, format, &english()).body).unwrap()
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
            (
                "auto_apply_held",
                "warning",
                json!({ "instance": "Radarr 4K", "candidates": 60, "cap": 50 }),
            ),
            ("rule_tests_failing", "warning", json!({ "failed": 1, "total": 4 })),
            ("sync_failed", "error", json!({ "instance_id": "inst-1", "instance": "Radarr" })),
            (
                "simulation_completed",
                "info",
                json!({ "simulation_id": "s-1", "total": 3, "moves": 1 }),
            ),
            (
                "apply_completed",
                "warning",
                json!({
                    "applied": 1, "failed": 1, "skipped": 0, "moving": 0, "task_id": "t-1",
                    "moves_total": 2, "truncated": false,
                    "moves": [as_json(&moved("Akira", "applied")), as_json(&moved("Totoro", "failed"))],
                }),
            ),
            (
                "revert_completed",
                "info",
                json!({
                    "applied": 1, "failed": 0, "skipped": 0, "moving": 0, "task_id": "t-2",
                    "moves_total": 1, "truncated": false, "moves": [as_json(&moved("Akira", "applied"))],
                }),
            ),
            (
                "moves_settled",
                "info",
                json!({
                    "moves_total": 1, "truncated": false, "moves": [as_json(&moved("Akira", "applied"))],
                }),
            ),
            ("backup_failed", "error", json!({})),
            ("test", "info", json!({})),
        ];
        for (event, (name, severity, data)) in every_event().iter().zip(expected) {
            let sent = sent(event);
            assert_eq!(sent["event"], name, "{sent}");
            assert_eq!(sent["severity"], severity, "{sent}");
            assert_eq!(sent["data"], data, "{sent}");
        }
    }

    /// Apprise API refuses with a 400 a `type` outside its four outcomes, a `format` outside
    /// text, markdown and html, and a request with no `body`. `type` picks the colour shown.
    #[test]
    fn apprise_api_accepts_routarrs_json_and_shows_its_outcome() {
        let outcomes = [
            "failure", "success", "failure", "warning", "warning", "failure", "info", "warning",
            "success", "success", "failure", "info",
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

    fn as_json(moved: &Moved) -> serde_json::Value {
        serde_json::to_value(moved).unwrap()
    }

    /// Fifty titles, the most a move notification lists, stay readable where a person reads them:
    /// ntfy's text under 4096 bytes, Discord's content under 2000 characters, five titles named.
    #[test]
    fn a_long_move_list_stays_readable_in_every_format_a_person_reads() {
        let titles: Vec<String> =
            (0..50).map(|n| format!("A film with a long title, number {n}")).collect();
        let event = Event::MovesCompleted {
            reverted: false,
            applied: 120,
            failed: 0,
            skipped: 0,
            moving: 0,
            task_id: "t-1".into(),
            moves: titles.iter().map(|title| moved(title, "applied")).collect(),
        };

        let ntfy = payload(&event, AT, Format::Ntfy, &english()).body;
        assert!(ntfy.len() <= 4096, "{} bytes", ntfy.len());
        assert!(ntfy.contains("Others: 115"), "{ntfy}");
        let content = sent(&event)["content"].as_str().unwrap().to_string();
        assert!(content.chars().count() <= 2000, "{content}");
        assert!(content.contains("number 4") && !content.contains("number 5"), "{content}");
        assert_eq!(sent(&event)["data"]["moves"].as_array().unwrap().len(), 50);
    }

    /// ntfy publishes a body posted to a topic as its text, and turns one over 4096 bytes into an
    /// attachment nobody reads in the notification: the ntfy format stays under it.
    #[test]
    fn ntfy_shows_every_event_as_text() {
        for event in every_event() {
            let sent = payload(&event, AT, Format::Ntfy, &english()).body;
            assert!(sent.len() <= 4096, "{} bytes", sent.len());
        }
    }

    /// A release waits as the published contract says it does: the test
    /// builds wait milliseconds, so only this sees the shipped waits.
    #[test]
    fn a_release_retries_on_the_schedule_the_contract_states() {
        assert_eq!(SHIPPED_RETRIES.map(|wait| wait.as_secs()), [10, 60, 300]);
        let contract = include_str!("../../openapi/v1.json");
        assert!(contract.contains("is tried again after 10 s, 60 s and 5 min"));
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
            assert_eq!(embed["title"], event.headline(&english()), "{sent}");
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
        let sent = payload(unreachable, AT, Format::Ntfy, &english());
        assert_eq!(sent.body, unreachable.message(&english()));
        assert!(sent.content_type.starts_with("text/plain"));
        assert_eq!(header(&sent, "Title").as_deref(), Some("Instance unreachable"));
        assert_eq!(header(&sent, "Priority").as_deref(), Some("4"));
        assert_eq!(header(&sent, "Tags").as_deref(), Some("rotating_light"));

        let finished = payload(&every_event()[8], AT, Format::Ntfy, &english());
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
        let ntfy = payload(&event, AT, Format::Ntfy, &english()).body;
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
        assert_eq!(priorities, [8, 2, 8, 5, 5, 8, 2, 5, 2, 2, 8, 2]);
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
