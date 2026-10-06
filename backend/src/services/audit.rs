//! The security log: every decision about who may do what, and every change
//! to a credential or a setting, as one line each at target `routarr::audit`.
//!
//! A line names the event, its outcome, who asked and from which address,
//! and never a credential: not a key, a token, a password or a code. A refusal
//! is a warning, which a fail2ban filter reads (`SECURITY.md`). The refusals an
//! anonymous caller can send as fast as it likes are written once a minute per
//! address, with how many came since.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use tokio::time::{Duration, Instant};

/// How long the same refusal from the same address is summed rather than
/// written again.
const QUIET: Duration = Duration::from_secs(60);

/// How many addresses the sums are kept for, the oldest forgotten first.
const ADDRESSES_KEPT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Allowed,
    Refused,
}

/// One decision or change worth a line.
pub struct Event<'a> {
    /// What it is about, stable across releases: `sign_in`, `sign_out`,
    /// `api_key`, `application_key`, `scope`, `origin`, `password`, `backup`,
    /// `restore`, `configuration`, `settings` or `signing_secret`.
    pub kind: &'static str,
    pub outcome: Outcome,
    /// Who asked, as the authentication mode named them, when it did.
    pub subject: Option<&'a str>,
    pub client: Option<IpAddr>,
    /// What happened, in a sentence that carries no credential.
    pub detail: String,
    /// Whether a caller can send this as fast as it likes, so that it is
    /// summed per address rather than written each time.
    pub summed: bool,
}

/// One refusal from one address: since when it is summed, and how many.
type Sums = HashMap<(&'static str, IpAddr), (Instant, u64)>;

/// The sums of the refusals being held back, per event and address.
#[derive(Default)]
pub struct Log(Mutex<Sums>);

impl Log {
    pub fn record(&self, event: Event<'_>) {
        let repeated = match (event.summed, event.client) {
            (true, Some(client)) => match self.held_back(event.kind, client) {
                Some(repeated) => repeated,
                None => return,
            },
            _ => 0,
        };
        let subject = event.subject.unwrap_or("-");
        let client = event.client.map_or_else(|| "unknown".to_string(), |ip| ip.to_string());
        let kind = event.kind;
        match event.outcome {
            Outcome::Allowed => tracing::info!(
                target: "routarr::audit",
                event = kind,
                outcome = "allowed",
                subject,
                client = %client,
                "{}",
                event.detail
            ),
            Outcome::Refused if repeated > 0 => tracing::warn!(
                target: "routarr::audit",
                event = kind,
                outcome = "refused",
                subject,
                client = %client,
                repeated,
                "{}, and {repeated} more like it in the last minute",
                event.detail
            ),
            Outcome::Refused => tracing::warn!(
                target: "routarr::audit",
                event = kind,
                outcome = "refused",
                subject,
                client = %client,
                "{}",
                event.detail
            ),
        }
    }

    /// `None` while the same refusal from `client` is being summed, else how
    /// many were summed since the last line.
    fn held_back(&self, kind: &'static str, client: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let mut sums = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((since, count)) = sums.get_mut(&(kind, client)) {
            if now.duration_since(*since) < QUIET {
                *count += 1;
                return None;
            }
            let repeated = *count;
            *since = now;
            *count = 0;
            return Some(repeated);
        }
        sums.retain(|_, (since, _)| now.duration_since(*since) < QUIET);
        if sums.len() >= ADDRESSES_KEPT
            && let Some(oldest) = sums.iter().min_by_key(|(_, (since, _))| *since).map(|(k, _)| *k)
        {
            sums.remove(&oldest);
        }
        sums.insert((kind, client), (now, 0));
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first refusal from an address is written, the ones within the
    /// minute after it are summed, and the first after the minute carries
    /// the sum. Another address is written at once.
    #[tokio::test(start_paused = true)]
    async fn refusals_from_one_address_are_summed_once_a_minute() {
        let log = Log::default();
        let client: IpAddr = "198.51.100.7".parse().unwrap();
        let other: IpAddr = "203.0.113.9".parse().unwrap();

        assert_eq!(log.held_back("api_key", client), Some(0));
        for _ in 0..41 {
            assert_eq!(log.held_back("api_key", client), None);
        }
        assert_eq!(log.held_back("api_key", other), Some(0));
        assert_eq!(log.held_back("scope", client), Some(0), "another event was summed with it");

        tokio::time::advance(QUIET).await;
        assert_eq!(log.held_back("api_key", client), Some(41));
        assert_eq!(log.held_back("api_key", client), None);
    }
}
