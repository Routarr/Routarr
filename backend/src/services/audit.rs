//! The security log: every decision about who may do what, and every change
//! to a credential or a setting, as one line each at target `routarr::audit`
//! and one row of `security_events`, which the security log screen reads.
//!
//! A line names the event, its outcome, who asked and from which address,
//! and never a credential: not a key, a token, a password or a code. A refusal
//! is a warning, which a fail2ban filter reads (`SECURITY.md`). The refusals an
//! anonymous caller can send as fast as it likes are written once a minute per
//! address, with how many came since.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};

/// How long the same refusal from the same address is summed rather than
/// written again.
const QUIET: Duration = Duration::from_secs(60);

/// How many addresses the sums are kept for, the oldest forgotten first.
const ADDRESSES_KEPT: usize = 4096;

/// How many events wait for the database before the next ones are counted
/// as lost rather than queued. The log line is written either way.
const QUEUED: usize = 4096;

/// How many events one transaction stores.
const BATCH: usize = 256;

/// What an event is about. Stable across releases: the log lines, the table
/// and a fail2ban filter all carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SignIn,
    SignOut,
    ApiKey,
    ApplicationKey,
    Scope,
    Origin,
    Password,
    Proof,
    Backup,
    Restore,
    Configuration,
    Settings,
    SigningSecret,
    SecurityLog,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SignIn => "sign_in",
            Kind::SignOut => "sign_out",
            Kind::ApiKey => "api_key",
            Kind::ApplicationKey => "application_key",
            Kind::Scope => "scope",
            Kind::Origin => "origin",
            Kind::Password => "password",
            Kind::Proof => "proof",
            Kind::Backup => "backup",
            Kind::Restore => "restore",
            Kind::Configuration => "configuration",
            Kind::Settings => "settings",
            Kind::SigningSecret => "signing_secret",
            Kind::SecurityLog => "security_log",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allowed,
    Refused,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Refused => "refused",
        }
    }
}

/// One decision or change worth a line.
pub struct Event<'a> {
    pub kind: Kind,
    pub outcome: Outcome,
    /// Who asked, as the authentication mode named them, when it did.
    pub subject: Option<&'a str>,
    pub client: Option<IpAddr>,
    /// What happened, as a key of `backend/locales/en.json`: the log line reads
    /// it in English, the screen in the reader's language. Neither it nor its
    /// parameters ever carry a credential.
    pub message: &'static str,
    pub params: Vec<(&'static str, String)>,
    /// Whether a caller can send this as fast as it likes, so that it is
    /// summed per address rather than written each time.
    pub summed: bool,
}

impl<'a> Event<'a> {
    pub fn new(kind: Kind, outcome: Outcome, message: &'static str) -> Self {
        Self {
            kind,
            outcome,
            subject: None,
            client: None,
            message,
            params: Vec::new(),
            summed: false,
        }
    }

    pub fn by(mut self, subject: Option<&'a str>, client: Option<IpAddr>) -> Self {
        self.subject = subject;
        self.client = client;
        self
    }

    pub fn with(mut self, name: &'static str, value: impl ToString) -> Self {
        self.params.push((name, value.to_string()));
        self
    }

    pub fn summed(mut self) -> Self {
        self.summed = true;
        self
    }
}

/// One refusal from one address: since when it is summed, and how many.
type Sums = HashMap<(Kind, IpAddr), (Instant, u64)>;

/// A row on its way to `security_events`.
struct Row {
    at: String,
    kind: Kind,
    outcome: Outcome,
    subject: Option<String>,
    client: Option<String>,
    message: &'static str,
    params: String,
    repeated: u64,
}

enum Order {
    Store(Row),
    /// Answered once every row sent before it is stored.
    Flush(oneshot::Sender<()>),
}

/// The sums of the refusals being held back, and the queue to the table.
#[derive(Default)]
pub struct Log {
    sums: Mutex<Sums>,
    store: Option<mpsc::Sender<Order>>,
    lost: Arc<AtomicU64>,
}

impl Log {
    /// A log whose events are also kept in `security_events`, by a task of its
    /// own: recording never waits on the database, which a refusal sent from
    /// a flood of addresses would otherwise hold.
    pub fn storing(pool: SqlitePool) -> Self {
        let (store, orders) = mpsc::channel(QUEUED);
        let lost = Arc::new(AtomicU64::new(0));
        tokio::spawn(write(pool, orders, Arc::clone(&lost)));
        Self { sums: Mutex::default(), store: Some(store), lost }
    }

    pub fn record(&self, event: Event<'_>) {
        let repeated = match (event.summed, event.client) {
            (true, Some(client)) => match self.held_back(event.kind, client) {
                Some(repeated) => repeated,
                None => return,
            },
            _ => 0,
        };
        let params: Vec<(&str, &str)> =
            event.params.iter().map(|(name, value)| (*name, value.as_str())).collect();
        let detail = crate::localization::Localizer::new("en").translate(event.message, &params);
        let subject = event.subject.unwrap_or("-");
        let client = event.client.map_or_else(|| "unknown".to_string(), |ip| ip.to_string());
        let kind = event.kind.as_str();
        match event.outcome {
            Outcome::Allowed => tracing::info!(
                target: "routarr::audit",
                event = kind,
                outcome = "allowed",
                subject,
                client = %client,
                "{detail}"
            ),
            Outcome::Refused if repeated > 0 => tracing::warn!(
                target: "routarr::audit",
                event = kind,
                outcome = "refused",
                subject,
                client = %client,
                repeated,
                "{detail}, and {repeated} more like it in the last minute"
            ),
            Outcome::Refused => tracing::warn!(
                target: "routarr::audit",
                event = kind,
                outcome = "refused",
                subject,
                client = %client,
                "{detail}"
            ),
        }

        let Some(store) = &self.store else { return };
        let params: serde_json::Map<String, serde_json::Value> = event
            .params
            .into_iter()
            .map(|(name, value)| (name.to_string(), serde_json::Value::String(value)))
            .collect();
        let row = Row {
            at: crate::services::routing::format_timestamp(chrono::Utc::now()),
            kind: event.kind,
            outcome: event.outcome,
            subject: event.subject.map(str::to_string),
            client: event.client.map(|ip| ip.to_string()),
            message: event.message,
            params: serde_json::Value::Object(params).to_string(),
            repeated,
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = store.try_send(Order::Store(row)) {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Wait until every event recorded so far is in the table: what a test
    /// reads after a request, and what a stop does before the database closes.
    /// Never from a handler holding a transaction, which the writer waits on.
    pub async fn flush(&self) {
        let Some(store) = &self.store else { return };
        let (done, stored) = oneshot::channel();
        if store.send(Order::Flush(done)).await.is_ok() {
            stored.await.ok();
        }
    }

    /// `None` while the same refusal from `client` is being summed, else how
    /// many were summed since the last line.
    fn held_back(&self, kind: Kind, client: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let mut sums = self.sums.lock().unwrap_or_else(PoisonError::into_inner);
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

/// Store what arrives, a batch per transaction, until every sender is gone.
async fn write(pool: SqlitePool, mut orders: mpsc::Receiver<Order>, lost: Arc<AtomicU64>) {
    while let Some(first) = orders.recv().await {
        let mut rows = Vec::new();
        let mut waiting = Vec::new();
        let mut next = Some(first);
        while let Some(order) = next {
            match order {
                Order::Store(row) => rows.push(row),
                Order::Flush(done) => waiting.push(done),
            }
            next = if rows.len() < BATCH { orders.try_recv().ok() } else { None };
        }
        let dropped = lost.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            rows.push(Row {
                at: crate::services::routing::format_timestamp(chrono::Utc::now()),
                kind: Kind::SecurityLog,
                outcome: Outcome::Refused,
                subject: None,
                client: None,
                message: "AuditEventsLost",
                params: serde_json::json!({ "count": dropped.to_string() }).to_string(),
                repeated: 0,
            });
        }
        if let Err(e) = insert(&pool, &rows).await {
            tracing::warn!("The security log could not store {} event(s): {e}", rows.len());
        }
        for done in waiting {
            done.send(()).ok();
        }
    }
}

async fn insert(pool: &SqlitePool, rows: &[Row]) -> sqlx::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO security_events
                 (at, kind, outcome, subject, client, message, params, repeated)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.at)
        .bind(row.kind.as_str())
        .bind(row.outcome.as_str())
        .bind(&row.subject)
        .bind(&row.client)
        .bind(row.message)
        .bind(&row.params)
        .bind(i64::try_from(row.repeated).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
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

        assert_eq!(log.held_back(Kind::ApiKey, client), Some(0));
        for _ in 0..41 {
            assert_eq!(log.held_back(Kind::ApiKey, client), None);
        }
        assert_eq!(log.held_back(Kind::ApiKey, other), Some(0));
        assert_eq!(log.held_back(Kind::Scope, client), Some(0), "another event was summed with it");

        tokio::time::advance(QUIET).await;
        assert_eq!(log.held_back(Kind::ApiKey, client), Some(41));
        assert_eq!(log.held_back(Kind::ApiKey, client), None);
    }

    /// A full queue never holds the request that records: what does not fit
    /// is counted, and the count is stored once there is room, so the screen
    /// says events are missing rather than looking complete.
    #[tokio::test]
    async fn events_a_full_queue_could_not_take_are_counted_in_the_table() {
        let pool = crate::state::AppState::for_tests().await.pool;
        let (store, orders) = mpsc::channel(1);
        let lost = Arc::new(AtomicU64::new(0));
        let log = Log { sums: Mutex::default(), store: Some(store), lost: Arc::clone(&lost) };
        for _ in 0..3 {
            log.record(Event::new(Kind::Settings, Outcome::Allowed, "AuditSettingsSaved"));
        }
        assert_eq!(lost.load(Ordering::Relaxed), 2);

        tokio::spawn(write(pool.clone(), orders, lost));
        log.flush().await;

        let stored: Vec<(String, String)> =
            sqlx::query_as("SELECT message, params FROM security_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            stored,
            [
                ("AuditSettingsSaved".to_string(), "{}".to_string()),
                ("AuditEventsLost".to_string(), r#"{"count":"2"}"#.to_string()),
            ]
        );
    }
}
