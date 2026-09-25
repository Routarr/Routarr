//! The single account of the `forms` mode, and the sessions it opens.
//!
//! One account, no role. The Servarr applications model it the same way: their
//! `UserService` reads `SingleOrDefault()` and their `User` carries a name and
//! a password hash and nothing else. Access here is all or nothing, and which
//! mode granted it is [`crate::api::auth`]'s business, not this module's.
//!
//! Sessions are opaque random ids in a table, never signed tokens. A token that
//! carries its own validity cannot be revoked before it expires; a row can be
//! deleted, which is what "log out everywhere" means and what a changed
//! password has to do.

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::info;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// The name the generated account carries. One account, so it needs no other.
pub const DEFAULT_USERNAME: &str = "admin";

/// How long a session lasts without use, renewed on every request that uses it.
///
/// Seven days, the value Radarr's cookie carries, and sliding for the same
/// reason: a tab left open over a weekend should not ask again on Monday.
pub const SESSION_DAYS: i64 = 7;

/// How many password checks may run at once.
///
/// Two, not one, so a second person signing in is not queued behind the first —
/// and not more, because each one costs a core and about 19 MiB for the length
/// of a hash. Measured on this code: **355 ms per verification**, against 1 ms
/// for `/ping`.
const CONCURRENT_CHECKS: usize = 2;

/// How many requests may be inside the check at once — hashing *and* waiting.
///
/// Past this they are refused immediately rather than held: a queue that grows
/// with the flood is the flood, moved from the processor to the socket table.
const MAX_IN_FLIGHT: usize = 10;

/// The endpoint is already checking as many passwords as it will.
pub struct Busy;

/// Bounds what `/auth/login` can be made to spend.
///
/// `/auth/login` is public and argon2id is deliberately expensive, so the cost
/// that makes a password hard to guess also makes the endpoint expensive to
/// serve. What that threatens is not the password — the generated one is 256
/// bits — but the machine: unbounded, N simultaneous requests take N cores and
/// N × 19 MiB, and on a two-core NAS the whole application stops answering.
///
/// **Concurrency, not a count.** A lockout after N failures bounds the
/// sustained rate and not the burst, because the failures are recorded after
/// the hashes they were meant to prevent: thirty simultaneous attempts all hash
/// before any of them closes the door — measured, thirty out of thirty. And it
/// buys that with a way to deny sign-in to the one account there is. A permit
/// bounds the resource itself and can refuse service to nobody: whoever waits,
/// waits for one hash.
///
/// The hash runs on `spawn_blocking`. Left on the runtime it holds a worker for
/// 355 ms, so on a small machine two sign-ins stall every other request —
/// the interface, the scheduler, the webhooks.
pub struct SignInThrottle {
    // Behind `Arc` so a permit and a queue slot can be *moved into* the
    // blocking task rather than held by the request future — see `verify`.
    permits: Arc<tokio::sync::Semaphore>,
    in_flight: Arc<AtomicUsize>,
}

impl Default for SignInThrottle {
    fn default() -> Self {
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(CONCURRENT_CHECKS)),
            in_flight: Arc::new(AtomicUsize::new(0)),
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

    /// Check a password, waiting for a permit and hashing off the runtime.
    ///
    /// `Err(Busy)` means the queue is full, which the caller answers with a
    /// `503` and a `Retry-After` — a wait of milliseconds, not a lockout.
    pub async fn verify(&self, password: &str, hash: &str) -> Result<bool, Busy> {
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
        // Both guards travel *into* the blocking task, and that is the whole
        // point. A client that hangs up drops this future, but a blocking task
        // cannot be cancelled — the hash runs to the end regardless. Held by the
        // future, the permit and the queue slot would come back the moment the
        // connection did, and a flood of abandoned requests would start one
        // argon2 per connection up to the size of the blocking pool: the very
        // thing this type exists to stop, reachable by hanging up.
        match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _leave = leave;
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

/// Decrements the count however the caller leaves — early return, error or
/// panic. A counter that only goes down on the happy path drifts up until the
/// endpoint refuses everything.
struct Leaving(Arc<AtomicUsize>);

impl Drop for Leaving {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
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

/// Create the account on first start, printing its password exactly once.
///
/// The same shape as the API key: generated, written to a 0600 file beside the
/// database, and logged once. Nothing is left open while it does not exist —
/// a first-run route that anyone may call is a race for the account, and the
/// installation that loses it has no way back in.
pub async fn ensure_account(pool: &SqlitePool, password_path: &std::path::Path) -> AppResult<()> {
    if account(pool).await?.is_some() {
        return Ok(());
    }

    let password = crate::crypto::generate_secret()?;
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
        .bind(Uuid::new_v4().to_string())
        .bind(DEFAULT_USERNAME)
        .bind(hash_password(&password)?)
        .execute(pool)
        .await?;

    crate::crypto::write_private(password_path, password.as_bytes()).map_err(|e| {
        AppError::Config(format!("cannot write the password to {}: {e}", password_path.display()))
    })?;

    info!(
        "Created the '{DEFAULT_USERNAME}' account at {}. Sign in with: {password}",
        password_path.display()
    );
    Ok(())
}

/// Replace the account's password, and end every session it had opened.
///
/// A password is changed because the old one is no longer trusted, so leaving
/// the sessions it opened alive would change nothing an attacker holds.
pub async fn set_password(pool: &SqlitePool, password: &str) -> AppResult<()> {
    // Hashing costs the same as verifying, so it belongs off the runtime for
    // the same reason — even on a route only the signed-in operator reaches.
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
    Ok(())
}

/// Open a session and return its opaque id.
pub async fn open_session(pool: &SqlitePool, subject: &str, source: &str) -> AppResult<String> {
    let id = crate::crypto::generate_secret()?;
    sqlx::query(
        "INSERT INTO sessions (id, subject, source, expires_at)
         VALUES (?, ?, ?, datetime('now', ?))",
    )
    .bind(&id)
    .bind(subject)
    .bind(source)
    .bind(format!("+{SESSION_DAYS} days"))
    .execute(pool)
    .await?;
    Ok(id)
}

/// The subject a session belongs to, if it is live and `source` opened it,
/// renewing it as it answers.
///
/// `source` is the mode in force. The database outlives a change of
/// `ROUTARR_AUTH`, and an operator who moves from `forms` to `oidc` does so to
/// change who may enter: a session the old mode opened would otherwise stay
/// valid, and slide for as long as it is used.
///
/// Expiry is compared in SQL rather than in Rust: the timestamps are written by
/// SQLite's own `datetime`, and comparing them anywhere else means agreeing on
/// a format twice.
pub async fn session_subject(
    pool: &SqlitePool,
    id: &str,
    source: &str,
) -> AppResult<Option<String>> {
    let subject: Option<String> = sqlx::query_scalar(
        "SELECT subject FROM sessions
         WHERE id = ? AND source = ? AND expires_at > datetime('now')",
    )
    .bind(id)
    .bind(source)
    .fetch_optional(pool)
    .await?;

    if subject.is_some() {
        // Sliding, like Radarr's: use is what keeps a session alive. Extended
        // only once it has less than a day to run, rather than on every
        // request: this runs on each authenticated call, SQLite takes one
        // writer at a time, and the Tasks screen polls every three seconds
        // while a job runs. A window that slides a day early slides just as
        // well, at a fraction of the writes.
        let _ = sqlx::query(
            "UPDATE sessions SET last_used_at = datetime('now'),
             expires_at = datetime('now', ?)
             WHERE id = ? AND expires_at < datetime('now', ?)",
        )
        .bind(format!("+{SESSION_DAYS} days"))
        .bind(id)
        .bind(format!("+{} days", SESSION_DAYS - 1))
        .execute(pool)
        .await;
    }
    Ok(subject)
}

/// End one session.
pub async fn close_session(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Drop the sessions nobody can use any more.
///
/// Run by the maintenance sweep rather than on every request: an expired row
/// already fails `session_subject`, so this is housekeeping and not a guard.
pub async fn purge_expired_sessions(pool: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM sessions WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound is on requests *inside* the check, hashing or waiting. Held
    /// instead of refused, a flood would simply move from the processor to the
    /// socket table.
    #[tokio::test]
    async fn one_past_the_bound_is_refused_rather_than_held() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();

        // `join_all` polls each future once before any can finish, and the
        // count is taken before the first await — so exactly MAX_IN_FLIGHT get
        // in and the next one does not, whatever the scheduler does after.
        let filling = futures::future::join_all(
            (0..MAX_IN_FLIGHT).map(|_| throttle.verify("correct horse battery", &hash)),
        );
        let (accepted, extra) =
            tokio::join!(filling, throttle.verify("correct horse battery", &hash));

        assert!(
            accepted.iter().all(|r| matches!(r, Ok(true))),
            "a check within the bound was lost"
        );
        assert!(matches!(extra, Err(Busy)), "the bound did not hold");
    }

    /// A blocking task cannot be cancelled, so hanging up must not hand the
    /// permit back while the hash it paid for is still running. Held by the
    /// request future instead of by the task, a flood of abandoned connections
    /// would start one argon2 each up to the size of the blocking pool.
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

        // Poll once — far enough to take the slot, take the permit and spawn
        // the hash — then drop the future, which is what a client hanging up
        // does to it.
        {
            let checking = throttle.verify("correct horse battery", &hash);
            tokio::pin!(checking);
            let waker = futures::task::noop_waker();
            let polled = checking.as_mut().poll(&mut Context::from_waker(&waker));
            assert!(
                matches!(polled, Poll::Pending),
                "the hash finished before it could be dropped"
            );
        }

        // The hash takes hundreds of milliseconds; this runs in microseconds.
        assert_eq!(throttle.in_flight(), 1, "the abandoned check gave its slot back");
        assert_eq!(
            throttle.free_permits(),
            0,
            "the abandoned check gave its permit back while still hashing"
        );

        // The one permit the check after it can get is the abandoned hash's,
        // so its completing *is* the proof that the hash finished and gave the
        // permit back — no budget to tune and nothing to go flaky under a
        // slower build, since no other permit exists to recycle.
        assert!(matches!(throttle.verify("correct horse battery", &hash).await, Ok(true)));

        assert_eq!(throttle.in_flight(), 0, "the slot never came back");
        assert_eq!(throttle.free_permits(), 1, "the permit never came back");
        drop(held);
        assert_eq!(throttle.free_permits(), CONCURRENT_CHECKS);
    }

    /// A slot is given back however the check ends, or the endpoint refuses
    /// everything after enough of them.
    #[tokio::test]
    async fn slots_come_back_after_every_outcome() {
        let throttle = SignInThrottle::default();
        let hash = hash_password("correct horse battery").unwrap();

        for _ in 0..(MAX_IN_FLIGHT * 2) {
            assert!(matches!(throttle.verify("wrong", &hash).await, Ok(false)));
        }
        assert!(
            matches!(throttle.verify("correct horse battery", &hash).await, Ok(true)),
            "the count leaked a slot"
        );
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

    /// The window has to keep sliding — a session in daily use must not expire
    /// on the seventh day — while costing a write only when it is close.
    #[tokio::test]
    async fn a_session_is_extended_only_once_it_is_close_to_expiring() {
        let pool = crate::db::test_pool().await;

        // Fresh: nothing to extend, and no write to pay for.
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES ('fresh', 'admin', 'forms', datetime('now', '+7 days'))",
        )
        .execute(&pool)
        .await
        .unwrap();
        let before: Option<String> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = 'fresh'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            session_subject(&pool, "fresh", "forms").await.unwrap().as_deref(),
            Some("admin")
        );
        let after: Option<String> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = 'fresh'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, after, "a session with days left was written to anyway");

        // Close to the edge: extended, or a session in daily use would die.
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES ('stale', 'admin', 'forms', datetime('now', '+2 hours'))",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            session_subject(&pool, "stale", "forms").await.unwrap().as_deref(),
            Some("admin")
        );
        let extended: String =
            sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = 'stale'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let cutoff: String =
            sqlx::query_scalar("SELECT datetime('now', '+6 days')").fetch_one(&pool).await.unwrap();
        assert!(extended > cutoff, "the window did not slide: {extended}");
    }

    #[tokio::test]
    async fn the_account_is_created_once_and_keeps_its_password() {
        let pool = crate::db::test_pool().await;
        let dir = std::env::temp_dir().join(format!("routarr-accounts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
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

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_session_answers_until_it_is_closed() {
        let pool = crate::db::test_pool().await;
        let id = open_session(&pool, "admin", "forms").await.unwrap();

        assert_eq!(session_subject(&pool, &id, "forms").await.unwrap().as_deref(), Some("admin"));
        assert!(session_subject(&pool, "not a session", "forms").await.unwrap().is_none());

        close_session(&pool, &id).await.unwrap();
        assert!(session_subject(&pool, &id, "forms").await.unwrap().is_none());
    }

    /// A password is changed because the old one is no longer trusted; the
    /// sessions it opened are exactly what an attacker would still be holding.
    #[tokio::test]
    async fn changing_the_password_ends_the_sessions_it_opened() {
        let pool = crate::db::test_pool().await;
        let dir = std::env::temp_dir().join(format!("routarr-accounts-pw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        ensure_account(&pool, &dir.join("routarr.password")).await.unwrap();

        let id = open_session(&pool, "admin", "forms").await.unwrap();
        set_password(&pool, "a new one").await.unwrap();

        assert!(session_subject(&pool, &id, "forms").await.unwrap().is_none());
        let (_, hash) = account(&pool).await.unwrap().unwrap();
        assert!(verify_password("a new one", &hash));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_expired_session_neither_answers_nor_lingers() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, source, expires_at)
             VALUES ('stale', 'admin', 'forms', datetime('now', '-1 day'))",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(session_subject(&pool, "stale", "forms").await.unwrap().is_none());
        assert_eq!(purge_expired_sessions(&pool).await.unwrap(), 1);
    }
}
