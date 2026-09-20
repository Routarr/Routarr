//! Telling the operator something went wrong while nobody was looking.
//!
//! Routarr does most of its work on a schedule or on a webhook, and in a
//! homelab nobody opens the interface daily. A scheduled sync that has been
//! failing for a week, or an automatic apply that Radarr rejected, is invisible
//! until someone thinks to look.
//!
//! One mechanism, not a gallery of connectors: a POST of JSON to a URL the user
//! configures. The payload carries the same text under the field names the usual
//! receivers read — `content` for Discord, `message` for Gotify and ntfy, `body`
//! for Apprise — alongside Routarr's own structured fields for anything that
//! parses properly.

use serde_json::json;
use tracing::{debug, warn};

use crate::state::AppState;

/// Something worth interrupting someone for.
///
/// Deliberately short. Every variant is a failure that will not resolve itself
/// and that the operator has to act on; a state the interface already shows is
/// not an event.
#[derive(Debug, Clone)]
pub enum Event {
    /// A scheduled sync could not reach an Arr. Sent on the transition only.
    InstanceUnreachable { instance: String, error: String },
    /// The same instance answered again. Closes the loop so the operator does
    /// not have to go and check whether the problem is still there.
    InstanceRecovered { instance: String },
    /// An unattended apply wrote to an Arr and the Arr refused.
    AutoApplyFailed { failed: usize, applied: usize, first_error: String },
}

impl Event {
    fn severity(&self) -> &'static str {
        match self {
            Event::InstanceRecovered { .. } => "info",
            _ => "error",
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Event::InstanceUnreachable { .. } => "instance_unreachable",
            Event::InstanceRecovered { .. } => "instance_recovered",
            Event::AutoApplyFailed { .. } => "auto_apply_failed",
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
                "Routarr failed to apply routing decisions automatically — failures: {failed} \
                 ({applied} succeeded). First error: {first_error}"
            ),
        }
    }
}

/// Post an event to the configured webhook, if there is one.
///
/// Never returns an error and never propagates one: a notification that cannot
/// be delivered must not fail the sync or the apply that produced it. Awaited
/// rather than spawned, so the outbound HTTP timeout bounds it and tests stay
/// deterministic.
pub async fn send(state: &AppState, event: Event) {
    let url = state.setting::<String>("notification_webhook_url", String::new()).await;
    let url = url.trim();
    if url.is_empty() {
        debug!(event = event.kind(), "No notification webhook configured");
        return;
    }

    let message = event.message();
    let payload = json!({
        // Routarr's own shape, for anything that parses the body properly.
        "event": event.kind(),
        "severity": event.severity(),
        "source": "routarr",
        "title": "Routarr",
        "message": message,
        // Aliases carrying the same text, so the common receivers work without
        // a translation layer: Discord reads `content`, Apprise reads `body`.
        "content": message,
        "body": message,
    });

    match state.http.post(url).json(&payload).send().await {
        Ok(response) if response.status().is_success() => {
            debug!(event = event.kind(), "Notification delivered")
        }
        Ok(response) => warn!(
            event = event.kind(),
            status = %response.status(),
            "Notification webhook rejected the message"
        ),
        Err(e) => warn!(event = event.kind(), "Notification webhook unreachable: {e}"),
    }
}
