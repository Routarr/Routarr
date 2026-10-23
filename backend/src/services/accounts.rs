//! The single account of the `forms` mode, and the sessions it opens.
//!
//! One account and no role, as in the Servarr applications. Sessions are opaque random ids in
//! a table, never signed tokens, so a sign-out everywhere or a changed password ends them.

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tracing::info;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// The name the generated account carries. One account, so it needs no other.
pub const DEFAULT_USERNAME: &str = "admin";

/// How long a session lasts without use, extended by the requests that use it: seven days,
/// as Radarr's cookie.
pub const SESSION_DAYS: i64 = 7;

/// How long a session lasts at most, used or not, in hours: then the mode that
/// opened it decides again who may enter. A copied cookie stops working, and a
/// person disabled at the OpenID Connect provider is asked again within a day,
/// often without typing anything, since the provider keeps its own session.
pub fn lifetime_hours(source: &str) -> i64 {
    match source {
        "oidc" => 24,
        _ => 30 * 24,
    }
}

/// How long after an OpenID Connect sign-in its session may still make or
/// withdraw a key. Past it the provider is asked again first, since Routarr
/// holds no password to ask for.
pub const RECENT_SIGN_IN_SECONDS: i64 = 10 * 60;

/// The `Max-Age` of the cookie a session is opened with, in seconds.
pub fn opening_seconds(source: &str) -> i64 {
    (SESSION_DAYS * 24).min(lifetime_hours(source)) * 3600
}

/// How many password checks may run at once: each costs a core and about 19 MiB, and two
/// keep a second person from queueing behind the first.
const CONCURRENT_CHECKS: usize = 2;

/// How many requests may be inside the check at once, hashing or waiting. Past it one is
/// refused at once, since a queue growing with a flood only moves it to the socket table.
pub(crate) const MAX_IN_FLIGHT: usize = 10;

/// How many of those one client may hold, below `MAX_IN_FLIGHT` so one address cannot fill
/// the queue. A request with no known address counts against the queue alone.
const PER_CLIENT: usize = 3;

/// How many failed sign-ins one address may make within [`FAILURE_WINDOW`]
/// before the next waits: guessing is slowed from there, and a person who
/// mistyped a few times is not.
const FAILURES_ALLOWED: u32 = 5;

/// How long failures are remembered without a new one.
const FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The first wait, doubled at every failure after it up to [`LONGEST_WAIT`].
const FIRST_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const LONGEST_WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How many addresses the failures are remembered for. Past it the one that
/// failed longest ago is forgotten first, so a flood from many addresses
/// costs a bounded table.
const ADDRESSES_REMEMBERED: usize = 4096;

/// The endpoint is already checking as many passwords as it will.
pub struct Busy;

/// The address one client answers for: an IPv4 address as it is, an IPv6 one
/// by its /64, the block one subscriber is given. Keyed by the full IPv6
/// address, one subscriber would be as many clients as it likes.
pub fn bucket(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let [a, b, c, d, ..] = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// What one address has failed lately.
struct Failures {
    count: u32,
    last: tokio::time::Instant,
    /// The wait the last failure set, which the next one doubles.
    wait: std::time::Duration,
    until: Option<tokio::time::Instant>,
}

impl Failures {
    /// Since when nothing has happened: the last failure, or the end of the
    /// wait it set.
    fn quiet_since(&self) -> tokio::time::Instant {
        self.until.map_or(self.last, |until| until.max(self.last))
    }
}

/// Bounds what the public `/auth/login` can be made to spend: argon2id costs a core and
/// 19 MiB per check, and unbounded, a burst stops a two-core NAS.
///
/// A permit bounds the burst, since failures are recorded only after the hashes. A wait per
/// address, never per account, bounds the rate: locking the one account would hand anyone a
/// way to deny it. The hash runs on `spawn_blocking`, off the async workers.
pub struct SignInThrottle {
    // Behind `Arc` so a permit and a queue slot can be *moved into* the
    // blocking task rather than held by the request future (see `verify`).
    permits: Arc<tokio::sync::Semaphore>,
    in_flight: Arc<AtomicUsize>,
    by_client: Arc<Mutex<HashMap<IpAddr, usize>>>,
    failures: Mutex<HashMap<IpAddr, Failures>>,
}

impl Default for SignInThrottle {
    fn default() -> Self {
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(CONCURRENT_CHECKS)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            by_client: Arc::default(),
            failures: Mutex::default(),
        }
    }
}

impl SignInThrottle {
    /// How many are inside the check right now. Only a test asks.
    #[cfg(test)]
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Whether a permit is free right now. Only a test asks.
    #[cfg(test)]
    pub fn free_permits(&self) -> usize {
        self.permits.available_permits()
    }

    /// How many addresses hold a slot right now. Only a test asks.
    #[cfg(test)]
    pub fn clients_holding(&self) -> usize {
        self.by_client.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// How long `client` still has to wait after its failures, if it has to.
    /// Asked before anything is hashed, so a guess sent while waiting costs
    /// nothing and is checked against nothing, the right password included.
    pub fn held_back(&self, client: Option<IpAddr>) -> Option<std::time::Duration> {
        let address = bucket(client?);
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let until = failures.get(&address)?.until?;
        until.checked_duration_since(tokio::time::Instant::now()).filter(|left| !left.is_zero())
    }

    /// A refused sign-in from `client`. From the [`FAILURES_ALLOWED`]th
    /// within the window, each sets a wait twice the last one. The failures
    /// are forgotten once the window has passed with none, counted from the
    /// end of the wait: counted from the failure, a wait as long as the window
    /// would end with them forgotten, and the doubling would start over.
    pub fn failed(&self, client: Option<IpAddr>) {
        let Some(client) = client else { return };
        let now = tokio::time::Instant::now();
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures.retain(|_, seen| now.duration_since(seen.quiet_since()) < FAILURE_WINDOW);
        if failures.len() >= ADDRESSES_REMEMBERED
            && let Some(oldest) =
                failures.iter().min_by_key(|(_, seen)| seen.quiet_since()).map(|(a, _)| *a)
        {
            failures.remove(&oldest);
        }
        let seen = failures.entry(bucket(client)).or_insert(Failures {
            count: 0,
            last: now,
            wait: std::time::Duration::ZERO,
            until: None,
        });
        seen.count += 1;
        seen.last = now;
        if seen.count >= FAILURES_ALLOWED {
            seen.wait =
                if seen.wait.is_zero() { FIRST_WAIT } else { (seen.wait * 2).min(LONGEST_WAIT) };
            seen.until = Some(now + seen.wait);
        }
    }

    /// A sign-in from `client` went through: its failures are forgotten.
    pub fn succeeded(&self, client: Option<IpAddr>) {
        if let Some(client) = client {
            self.failures.lock().unwrap_or_else(PoisonError::into_inner).remove(&bucket(client));
        }
    }

    /// Check a password, waiting for a permit and hashing off the runtime.
    ///
    /// `Err(Busy)` means the queue is full, which the caller answers with a
    /// `503` and a `Retry-After` of one second: a short wait, not a lockout.
    pub async fn verify(
        &self,
        password: &str,
        hash: &str,
        client: Option<IpAddr>,
    ) -> Result<bool, Busy> {
        let share = match client {
            Some(ip) => {
                Some(ClientShare::take(&self.by_client, bucket(ip), PER_CLIENT).ok_or(Busy)?)
            }
            None => None,
        };
        // Counted before the wait, so the refusal happens without holding a
        // connection open behind a semaphore that is already saturated.
        if self.in_flight.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            return Err(Busy);
        }
        let leave = Leaving(Arc::clone(&self.in_flight));

        // The semaphore is never closed, so this cannot fail.
        let Ok(permit) = Arc::clone(&self.permits).acquire_owned().await else {
            return Err(Busy);
        };

        let (password, hash) = (password.to_string(), hash.to_string());
        // Both guards move into the blocking task, which a hang-up cannot cancel: held by this
        // future, they would come back with the connection while the hash still runs, and abandoned
        // requests would start one argon2 each.
        match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _leave = leave;
            let _share = share;
            verify_password(&password, &hash)
        })
        .await
        {
            Ok(matched) => Ok(matched),
            // A panic under argon2 is not a reason to let somebody in. The
            // guards were moved into the task, so they are released by its
            // unwinding rather than left behind.
            Err(e) => {
                tracing::error!("A password check panicked: {e}");
                Ok(false)
            }
        }
    }
}

/// Decrements the count however the caller leaves: early return, error or
/// panic. A counter that only goes down on the happy path drifts up until the
/// endpoint refuses everything.
struct Leaving(Arc<AtomicUsize>);

impl Drop for Leaving {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One client's slot, given back however its holder ends.
pub(crate) struct ClientShare {
    clients: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl ClientShare {
    /// A slot for `ip`, unless it already holds `at_most`.
    pub(crate) fn take(
        clients: &Arc<Mutex<HashMap<IpAddr, usize>>>,
        ip: IpAddr,
        at_most: usize,
    ) -> Option<Self> {
        let mut held = clients.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.entry(ip).or_insert(0);
        if *count >= at_most {
            return None;
        }
        *count += 1;
        Some(Self { clients: Arc::clone(clients), ip })
    }
}

impl Drop for ClientShare {
    fn drop(&mut self) {
        let mut held = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.ip) {
            *count -= 1;
            // An address that holds nothing leaves the table, or a flood from
            // many addresses would keep one entry for each of them.
            if *count == 0 {
                held.remove(&self.ip);
            }
        }
    }
}

/// Hash a password for storage. Argon2id with the crate's own parameters.
pub fn hash_password(password: &str) -> AppResult<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Internal(format!("cannot hash a password: {e}")))
}

/// Whether a password matches a stored hash.
///
/// The parameters come from the stored hash rather than from the verifier, so
/// a hash written under older settings keeps verifying after they change.
pub fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

/// The stored account, if one exists.
pub async fn account(pool: &SqlitePool) -> AppResult<Option<(String, String)>> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT username, password_hash FROM users ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(pool)
    .await?)
}

/// Create the account on first start, its password written to a 0600 file beside the
/// database, whose path the log names once. No first-run route is left open: whoever called
/// it first would take the account.
pub async fn ensure_account(pool: &SqlitePool, password_path: &std::path::Path) -> AppResult<()> {
    if account(pool).await?.is_some() {
        return Ok(());
    }

    let password = crate::crypto::generate_secret()?;
    let hash = hash_password(&password)?;
    // The file before the row: an account whose password never reached the
    // file is one nobody can open, and the next start, finding it, would
    // generate none. A file without its row is written over at that start.
    crate::crypto::write_private(password_path, password.as_bytes()).map_err(|e| {
        AppError::Config(format!("cannot write the password to {}: {e}", password_path.display()))
    })?;
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(DEFAULT_USERNAME)
        .bind(&hash)
        .execute(pool)
        .await?;

    // The path and never the password, which a log shipper would keep.
    info!(
        "Created the '{DEFAULT_USERNAME}' account. Its password is in {}",
        password_path.display()
    );
    Ok(())
}

/// Start the account again with a new password, as `routarr reset-account`
/// does for an operator locked out: the password is written beside the
/// database and handed back to be printed, and every session ends.
pub async fn reset_account(
    pool: &SqlitePool,
    password_path: &std::path::Path,
) -> AppResult<String> {
    let password = crate::crypto::generate_secret()?;
    let hash = hash_password(&password)?;
    crate::crypto::write_private(password_path, password.as_bytes()).map_err(|e| {
        AppError::Config(format!("cannot write the password to {}: {e}", password_path.display()))
    })?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM sessions").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM users").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(DEFAULT_USERNAME)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(password)
}

/// Replace the account's password, end every session it had opened, and remove the file the
/// first password was written to, which no longer opens anything.
pub async fn set_password(
    pool: &SqlitePool,
    password_path: &std::path::Path,
    password: &str,
) -> AppResult<()> {
    // Hashing costs the same as verifying, so it belongs off the runtime for
    // the same reason, even on a route only the signed-in operator reaches.
    let owned = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&owned))
        .await
        .map_err(|e| AppError::Internal(format!("hashing a password panicked: {e}")))??;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = datetime('now')")
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE source = ?")
        .bind(crate::config::AuthMode::Forms.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Err(e) = std::fs::remove_file(password_path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("Could not remove {}: {e}", password_path.display());
    }
    Ok(())
}

/// What the table holds of a session: the digest of the id its cookie
/// carries, so a copy of the database, a backup or a stolen disk, opens none.
pub(crate) fn stored(id: &str) -> String {
    digest(&[b"routarr:session:v1:", id.as_bytes()])
}

fn digest(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What a session is bound to besides its mode, compared at every request: a change of it
/// ends the sessions it concerns, whatever their lifetime has left. No `Debug`: it would print
/// the API key.
pub enum OpenedUnder {
    /// The `forms` account, whose password change and reset end its sessions themselves.
    Account,
    /// The OIDC lists, by `Config::oidc_access_digest`.
    Lists(String),
    /// The API key in force.
    Key(String),
}

impl OpenedUnder {
    /// What `opened_under` holds for the session `id`. A key is mixed with the id, so a copy of
    /// the database cannot test guesses of a weak pinned key.
    fn stored(&self, id: &str) -> String {
        match self {
            Self::Account => String::new(),
            Self::Lists(digest) => format!("lists:{digest}"),
            Self::Key(key) => {
                let parts: [&[u8]; 4] =
                    [b"routarr:key-session:v1:", id.as_bytes(), b":", key.as_bytes()];
                format!("key:{}", digest(&parts))
            }
        }
    }
}

/// What a person signed in may reach: everything, or what an application key's read scope
/// reads. Only an OIDC sign-in opens a reader's session, from the read lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reach {
    #[default]
    Full,
    Read,
}

impl Reach {
    pub fn as_str(self) -> &'static str {
        match self {
            Reach::Full => "full",
            Reach::Read => "read",
        }
    }

    /// As stored, any value but `full` read as `read`, so a damaged row grants less.
    fn stored(value: &str) -> Self {
        if value == "full" { Reach::Full } else { Reach::Read }
    }
}

/// Open a session and return its opaque id.
pub async fn open_session(
    pool: &SqlitePool,
    subject: &str,
    source: &str,
    reach: Reach,
    under: &OpenedUnder,
) -> AppResult<String> {
    let id = crate::crypto::generate_secret()?;
    sqlx::query(
        "INSERT INTO sessions (id, subject, source, expires_at, reach, opened_under)
         VALUES (?, ?, ?, datetime('now', ?), ?, ?)",
    )
    .bind(stored(&id))
    .bind(subject)
    .bind(source)
    .bind(format!("+{} seconds", opening_seconds(source)))
    .bind(reach.as_str())
    .bind(under.stored(&id))
    .execute(pool)
    .await?;
    Ok(id)
}

/// Bind the `source` session `id` to `under` from now on, as a key replaced from the session
/// that asked keeps that session.
pub async fn rebind(
    pool: &SqlitePool,
    id: &str,
    source: &str,
    under: &OpenedUnder,
) -> AppResult<()> {
    sqlx::query("UPDATE sessions SET opened_under = ? WHERE id = ? AND source = ?")
        .bind(under.stored(id))
        .bind(stored(id))
        .bind(source)
        .execute(pool)
        .await?;
    Ok(())
}

/// A live session, as answering it left it.
pub struct Session {
    pub subject: String,
    pub reach: Reach,
    /// When it was opened, in Unix seconds: what a recent sign-in is read from.
    pub opened_at: i64,
    /// The seconds the browser's cookie is given again for, when answering it
    /// moved its expiry, which the cookie does not follow until it is.
    pub renewed: Option<i64>,
}

/// The session behind an id, if it is live and `source`, the mode in force, opened it under
/// `under`, renewing it when due: a change of the mode, the OIDC lists or the API key ends
/// what it opened. Expiry is compared in SQL, against what SQLite wrote.
pub async fn live_session(
    pool: &SqlitePool,
    id: &str,
    source: &str,
    under: &OpenedUnder,
) -> AppResult<Option<Session>> {
    let lifetime = format!("-{} hours", lifetime_hours(source));
    // Renewal is due once less than `SESSION_DAYS - 1` days are left, never past the lifetime.
    let live: Option<(String, i64, String, bool)> = sqlx::query_as(
        "SELECT subject, CAST(strftime('%s', created_at) AS INTEGER), reach,
                expires_at < MIN(datetime('now', ?), datetime(created_at, ?, '-1 hour'))
           FROM sessions
          WHERE id = ? AND source = ? AND opened_under = ? AND expires_at > datetime('now')
            AND created_at > datetime('now', ?)",
    )
    .bind(format!("+{} days", SESSION_DAYS - 1))
    .bind(format!("+{} hours", lifetime_hours(source)))
    .bind(stored(id))
    .bind(source)
    .bind(under.stored(id))
    .bind(&lifetime)
    .fetch_optional(pool)
    .await?;
    let Some((subject, opened_at, reach, due_now)) = live else {
        return Ok(None);
    };
    let session = |renewed| Session { subject, reach: Reach::stored(&reach), opened_at, renewed };
    // Written at most daily: every authenticated call runs this, and SQLite has one writer.
    if !due_now {
        return Ok(Some(session(None)));
    }
    let renewed: Option<i64> = sqlx::query_scalar(
        "UPDATE sessions SET last_used_at = datetime('now'),
                expires_at = MIN(datetime('now', ?), datetime(created_at, ?))
          WHERE id = ?
            AND expires_at < MIN(datetime('now', ?), datetime(created_at, ?, '-1 hour'))
         RETURNING CAST(strftime('%s', expires_at) AS INTEGER)
                 - CAST(strftime('%s', 'now') AS INTEGER)",
    )
    .bind(format!("+{SESSION_DAYS} days"))
    .bind(format!("+{} hours", lifetime_hours(source)))
    .bind(stored(id))
    .bind(format!("+{} days", SESSION_DAYS - 1))
    .bind(format!("+{} hours", lifetime_hours(source)))
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    Ok(Some(session(renewed)))
}

/// One session as the owner sees it on the sessions screen: never its id,
/// which opens it, but a handle that names it.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct Listed {
    /// The first characters of the stored digest: enough to name the session,
    /// and nothing that opens it.
    pub handle: String,
    pub subject: String,
    /// The mode that opened it: `forms`, `oidc` or `apikey`.
    pub source: String,
    /// `full`, or `read` for a person the OIDC read lists let in.
    pub reach: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    pub created_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    pub last_used_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    pub expires_at: String,
    /// Whether the request asking holds this one.
    #[sqlx(skip)]
    pub current: bool,
}

/// The live sessions, newest first, `current` set on the one `id` opens.
pub async fn list_sessions(pool: &SqlitePool, current: Option<&str>) -> AppResult<Vec<Listed>> {
    let mut listed: Vec<Listed> = sqlx::query_as(
        "SELECT substr(id, 1, 16) AS handle, subject, source, reach, created_at, last_used_at,
                expires_at
           FROM sessions WHERE expires_at > datetime('now') ORDER BY created_at DESC, id",
    )
    .fetch_all(pool)
    .await?;
    let current = current.map(|id| stored(id)[..16].to_string());
    for session in &mut listed {
        session.current = current.as_deref() == Some(session.handle.as_str());
    }
    Ok(listed)
}

/// End the session `handle` names. `false` when none does.
pub async fn end_session(pool: &SqlitePool, handle: &str) -> AppResult<bool> {
    if handle.len() != 16 || !handle.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(false);
    }
    let ended = sqlx::query("DELETE FROM sessions WHERE substr(id, 1, 16) = ?")
        .bind(handle)
        .execute(pool)
        .await?;
    Ok(ended.rows_affected() > 0)
}

/// End every session: signing out everywhere.
pub async fn end_every_session(pool: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM sessions").execute(pool).await?.rows_affected())
}

/// End one session.
pub async fn close_session(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?").bind(stored(id)).execute(pool).await?;
    Ok(())
}

/// Drop the expired sessions, which `live_session` already refuses: housekeeping for the
/// maintenance sweep.
pub async fn purge_expired_sessions(pool: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM sessions WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_past_the_bound_is_refused_rather_than_held() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();

        // `join_all` polls each future once before any can finish, and the
        // count is taken before the first await, so exactly MAX_IN_FLIGHT get
        // in and the next one does not, whatever the scheduler does after.
        let filling = futures::future::join_all(
            (0..MAX_IN_FLIGHT).map(|_| throttle.verify("correct horse battery", &hash, None)),
        );
        let (accepted, extra) =
            tokio::join!(filling, throttle.verify("correct horse battery", &hash, None));

        assert!(
            accepted.iter().all(|r| matches!(r, Ok(true))),
            "a check within the bound was lost"
        );
        assert!(matches!(extra, Err(Busy)), "the bound did not hold");
    }

    #[tokio::test]
    async fn hanging_up_does_not_return_a_permit_the_hash_is_still_using() {
        use std::future::Future;
        use std::task::{Context, Poll};

        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();

        // Every permit but one is held here, so the one the abandoned check
        // takes is the only one a check after it can get. With two free, the
        // checks that follow recycle the other and never wait for the
        // abandoned hash at all.
        let held: Vec<_> = (1..CONCURRENT_CHECKS)
            .map(|_| Arc::clone(&throttle.permits).try_acquire_owned().expect("a free permit"))
            .collect();

        // Poll once (far enough to take the slot, take the permit and spawn
        // the hash), then drop the future, which is what a client hanging up
        // does to it.
        {
            let checking = throttle.verify("correct horse battery", &hash, None);
            tokio::pin!(checking);
            let waker = futures::task::noop_waker();
            let polled = checking.as_mut().poll(&mut Context::from_waker(&waker));
            assert!(
                matches!(polled, Poll::Pending),
                "the hash finished before it could be dropped"
            );
        }

        // The hash takes hundreds of milliseconds, and this runs in microseconds.
        assert_eq!(throttle.in_flight(), 1, "the abandoned check gave its slot back");
        assert_eq!(
            throttle.free_permits(),
            0,
            "the abandoned check gave its permit back while still hashing"
        );

        // The one permit the next check can get is the abandoned hash's, so its completing proves
        // the hash finished and gave it back.
        assert!(matches!(throttle.verify("correct horse battery", &hash, None).await, Ok(true)));

        assert_eq!(throttle.in_flight(), 0, "the slot never came back");
        assert_eq!(throttle.free_permits(), 1, "the permit never came back");
        drop(held);
        assert_eq!(throttle.free_permits(), CONCURRENT_CHECKS);
    }

    #[tokio::test]
    async fn a_client_filling_the_queue_leaves_room_for_another() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();
        let flooding = "198.51.100.7".parse().ok();
        let other = "203.0.113.9".parse().ok();

        let flood = futures::future::join_all(
            (0..MAX_IN_FLIGHT).map(|_| throttle.verify("wrong", &hash, flooding)),
        );
        let (flood, signed_in) =
            tokio::join!(flood, throttle.verify("correct horse battery", &hash, other));

        assert!(matches!(signed_in, Ok(true)), "the other client was refused");
        // The flood's address took its three places and no more, so seven of
        // its ten were held back.
        let held_back = flood.iter().filter(|r| matches!(r, Err(Busy))).count();
        assert_eq!(held_back, 7, "an address took more than three places");
    }

    #[tokio::test]
    async fn slots_come_back_after_every_outcome() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();
        let client = "198.51.100.7".parse().ok();

        for _ in 0..(MAX_IN_FLIGHT * 2).max(PER_CLIENT * 3) {
            assert!(matches!(throttle.verify("wrong", &hash, client).await, Ok(false)));
        }
        assert!(
            matches!(throttle.verify("correct horse battery", &hash, client).await, Ok(true)),
            "the count leaked a slot"
        );
        assert_eq!(throttle.clients_holding(), 0, "an address kept a slot it no longer uses");
    }

    #[tokio::test]
    async fn addresses_of_one_ipv6_slash_64_hold_one_share() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();
        let at = |ip: &str| ip.parse().ok();

        let block = futures::future::join_all(
            ["2001:db8::1", "2001:db8::2", "2001:db8::3", "2001:db8::4"]
                .map(|ip| throttle.verify("wrong", &hash, at(ip))),
        );
        let (block, other) = tokio::join!(
            block,
            throttle.verify("correct horse battery", &hash, at("2001:db8:1::1"))
        );

        assert_eq!(block.iter().filter(|r| matches!(r, Err(Busy))).count(), 1);
        assert!(matches!(other, Ok(true)), "another block was refused");
    }

    #[tokio::test(start_paused = true)]
    async fn the_wait_doubles_after_five_failures_and_a_success_forgets_it() {
        let throttle = SignInThrottle::default();
        let client = "2001:db8::7".parse().ok();
        let neighbour = "2001:db8::8".parse().ok();
        let seconds = |left: Option<std::time::Duration>| left.map(|d| d.as_secs());

        for _ in 0..4 {
            throttle.failed(client);
        }
        assert_eq!(throttle.held_back(client), None, "held back before the fifth failure");
        throttle.failed(neighbour);
        assert_eq!(seconds(throttle.held_back(client)), Some(30));

        let mut expected = 30;
        for _ in 0..8 {
            tokio::time::advance(std::time::Duration::from_secs(expected)).await;
            assert_eq!(throttle.held_back(client), None, "still held back after {expected}s");
            throttle.failed(client);
            expected = (expected * 2).min(15 * 60);
            assert_eq!(seconds(throttle.held_back(client)), Some(expected));
        }
        assert_eq!(expected, 15 * 60);

        throttle.succeeded(neighbour);
        assert_eq!(throttle.held_back(client), None, "a success left the wait in place");
        for _ in 0..4 {
            throttle.failed(client);
        }
        assert_eq!(throttle.held_back(client), None, "a success left the failures counted");

        tokio::time::advance(FAILURE_WINDOW).await;
        throttle.failed(client);
        assert_eq!(throttle.held_back(client), None, "a quiet window left the failures counted");
    }

    #[test]
    fn a_password_verifies_against_its_own_hash_and_nothing_else() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("Correct horse", &hash));
        assert!(!verify_password("", &hash));
    }

    /// Two accounts with the same password must not share a hash, or one
    /// cracked hash would read as two.
    #[test]
    fn the_same_password_hashes_differently_every_time() {
        let first = hash_password("hunter2").unwrap();
        let second = hash_password("hunter2").unwrap();
        assert_ne!(first, second);
        assert!(verify_password("hunter2", &first));
        assert!(verify_password("hunter2", &second));
    }

    /// A stored value that is not a PHC string must fail closed rather than
    /// panic: it is the shape a corrupted row or a hand-edited database has.
    #[test]
    fn a_hash_that_is_not_one_refuses_every_password() {
        assert!(!verify_password("anything", "not a hash"));
        assert!(!verify_password("", ""));
    }

    #[tokio::test]
    async fn a_session_is_extended_only_once_it_is_close_to_expiring() {
        let pool = crate::db::test_pool().await;

        // Fresh: nothing to extend, and no write to pay for.
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES (?, 'admin', 'forms', datetime('now', '+7 days'))",
        )
        .bind(stored("fresh"))
        .execute(&pool)
        .await
        .unwrap();
        let before: Option<String> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
                .bind(stored("fresh"))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            live_session(&pool, "fresh", "forms", &OpenedUnder::Account)
                .await
                .unwrap()
                .map(|s| s.subject)
                .as_deref(),
            Some("admin")
        );
        let after: Option<String> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
                .bind(stored("fresh"))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, after, "a session with days left was written to anyway");

        // Close to the edge: extended, or a session in daily use would die.
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES (?, 'admin', 'forms', datetime('now', '+2 hours'))",
        )
        .bind(stored("stale"))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            live_session(&pool, "stale", "forms", &OpenedUnder::Account)
                .await
                .unwrap()
                .map(|s| s.subject)
                .as_deref(),
            Some("admin")
        );
        let extended: String = sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = ?")
            .bind(stored("stale"))
            .fetch_one(&pool)
            .await
            .unwrap();
        let cutoff: String =
            sqlx::query_scalar("SELECT datetime('now', '+6 days')").fetch_one(&pool).await.unwrap();
        assert!(extended > cutoff, "the window did not slide: {extended}");
    }

    #[tokio::test]
    async fn a_session_past_its_absolute_lifetime_answers_nothing_even_in_use() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, created_at, expires_at)
             VALUES (?, 'admin', 'forms', datetime('now', '-31 days'), datetime('now', '+6 days')),
                    (?, 'u-42', 'oidc', datetime('now', '-25 hours'), datetime('now', '+6 days')),
                    (?, 'admin', 'forms', datetime('now', '-29 days'), datetime('now', '+6 days'))",
        )
        .bind(stored("month-old"))
        .bind(stored("day-old"))
        .bind(stored("in-time"))
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            live_session(&pool, "month-old", "forms", &OpenedUnder::Account)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            live_session(&pool, "day-old", "oidc", &OpenedUnder::Account).await.unwrap().is_none()
        );
        assert!(
            live_session(&pool, "in-time", "forms", &OpenedUnder::Account).await.unwrap().is_some()
        );
    }

    #[tokio::test]
    async fn renewal_never_extends_past_the_absolute_lifetime() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, created_at, expires_at)
             VALUES (?, 'admin', 'forms', datetime('now', '-29 days'), datetime('now', '+1 hour'))",
        )
        .bind(stored("closing"))
        .execute(&pool)
        .await
        .unwrap();
        let session =
            live_session(&pool, "closing", "forms", &OpenedUnder::Account).await.unwrap().unwrap();
        let given = session.renewed.expect("renewed close to its expiry");
        assert!(given <= 24 * 3600 + 5, "renewed past the lifetime by {given} seconds");
        let ends: bool = sqlx::query_scalar(
            "SELECT expires_at <= datetime(created_at, '+720 hours') FROM sessions WHERE id = ?",
        )
        .bind(stored("closing"))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(ends);

        let lists = OpenedUnder::Lists("the lists".into());
        let id = open_session(&pool, "u-42", "oidc", Reach::Full, &lists).await.unwrap();
        let session = live_session(&pool, &id, "oidc", &lists).await.unwrap().unwrap();
        assert_eq!(session.renewed, None, "a session already at its lifetime was written to");
    }

    #[tokio::test]
    async fn the_account_is_created_once_and_keeps_its_password() {
        let pool = crate::db::test_pool().await;
        let dir = crate::tests::TempDir::new("accounts");
        let path = dir.join("routarr.password");

        ensure_account(&pool, &path).await.unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let (username, hash) = account(&pool).await.unwrap().unwrap();
        assert_eq!(username, DEFAULT_USERNAME);
        assert!(verify_password(&first, &hash));

        // A second start must not replace the account under the operator.
        ensure_account(&pool, &path).await.unwrap();
        let (_, again) = account(&pool).await.unwrap().unwrap();
        assert_eq!(hash, again);
    }

    #[tokio::test]
    async fn a_password_that_cannot_be_written_leaves_no_account_behind() {
        let pool = crate::db::test_pool().await;
        let dir = crate::tests::TempDir::new("accounts-unwritable");
        let path = dir.join("routarr.password");
        // A directory where the file goes refuses the write whoever runs this.
        std::fs::create_dir(&path).unwrap();

        assert!(ensure_account(&pool, &path).await.is_err());
        assert!(account(&pool).await.unwrap().is_none(), "an account outlived its password");

        std::fs::remove_dir(&path).unwrap();
        ensure_account(&pool, &path).await.unwrap();
        let password = std::fs::read_to_string(&path).unwrap();
        let (_, hash) = account(&pool).await.unwrap().unwrap();
        assert!(verify_password(&password, &hash));
    }

    #[tokio::test]
    async fn a_session_answers_until_it_is_closed() {
        let pool = crate::db::test_pool().await;
        let account = OpenedUnder::Account;
        let id = open_session(&pool, "admin", "forms", Reach::Full, &account).await.unwrap();

        assert_eq!(
            live_session(&pool, &id, "forms", &account)
                .await
                .unwrap()
                .map(|s| s.subject)
                .as_deref(),
            Some("admin")
        );
        assert!(live_session(&pool, "not a session", "forms", &account).await.unwrap().is_none());

        close_session(&pool, &id).await.unwrap();
        assert!(live_session(&pool, &id, "forms", &account).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn changing_the_password_ends_the_sessions_it_opened() {
        let pool = crate::db::test_pool().await;
        let dir = crate::tests::TempDir::new("accounts-pw");
        ensure_account(&pool, &dir.join("routarr.password")).await.unwrap();

        let id = open_session(&pool, "admin", "forms", Reach::Full, &OpenedUnder::Account)
            .await
            .unwrap();
        set_password(&pool, std::path::Path::new("/nonexistent/routarr.password"), "a new one")
            .await
            .unwrap();

        assert!(live_session(&pool, &id, "forms", &OpenedUnder::Account).await.unwrap().is_none());
        let (_, hash) = account(&pool).await.unwrap().unwrap();
        assert!(verify_password("a new one", &hash));
    }

    /// The purge takes the expired session and leaves the live one beside it,
    /// which still answers: a purge reaching it would sign everyone out at
    /// every maintenance pass.
    #[tokio::test]
    async fn an_expired_session_neither_answers_nor_lingers() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES (?, 'admin', 'forms', datetime('now', '-1 day')),
                    (?, 'admin', 'forms', datetime('now', '+1 day'))",
        )
        .bind(stored("stale"))
        .bind(stored("live"))
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            live_session(&pool, "stale", "forms", &OpenedUnder::Account).await.unwrap().is_none()
        );
        assert_eq!(purge_expired_sessions(&pool).await.unwrap(), 1);
        assert!(
            live_session(&pool, "live", "forms", &OpenedUnder::Account).await.unwrap().is_some()
        );
    }
}
