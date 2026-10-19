//! Keys the operator gives other applications.
//!
//! The master key and a session reach everything. An application key reaches
//! what its scopes grant and answers only the guardrails it was given, so a
//! request bot that pins titles cannot also move a library. The token reads
//! `rtr_<id>_<secret>`: the id finds the row without comparing against every
//! key, and only a hash of the secret is stored.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::error::{AppError, AppResult};
use crate::services::executor::confirm;
use crate::state::AppState;

/// What every application token starts with, and no master key does: the
/// middleware tells the two apart before it looks anything up.
pub const TOKEN_PREFIX: &str = "rtr_";

/// The longest life a key may be given, in days.
pub const EXPIRY_DAYS_MAX: u32 = 3650;

/// How long before its expiry the owner is told a key is about to stop.
const EXPIRY_NOTICE: &str = "+7 days";

/// The longest name a key may carry. It is written on every task, decision
/// and pin the application makes, and shown in their tables.
pub const NAME_MAX: usize = 64;

/// How many requests a key may make in a second, past a burst of
/// [`BURST`]: a dashboard polling and a request bot placing titles stay far
/// below, and a loop does not run the owner's Arrs and sources hot.
pub const PER_SECOND: u32 = 10;
pub const BURST: u32 = 50;

/// What each key has left of its burst, refilled at [`PER_SECOND`].
#[derive(Default)]
pub struct Rates(std::sync::Mutex<std::collections::HashMap<String, (f64, tokio::time::Instant)>>);

impl Rates {
    /// Every key's allowance whole again, for a test that walks every route
    /// with one key and is not about its rate.
    #[cfg(test)]
    pub fn refill(&self) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    /// Take one request from `key`'s allowance, or say how long until one is
    /// there.
    pub fn take(&self, key: &str) -> Result<(), std::time::Duration> {
        let now = tokio::time::Instant::now();
        let mut rates = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (left, at) = rates.entry(key.to_string()).or_insert((f64::from(BURST), now));
        let refilled = now.duration_since(*at).as_secs_f64() * f64::from(PER_SECOND);
        *left = (*left + refilled).min(f64::from(BURST));
        *at = now;
        if *left >= 1.0 {
            *left -= 1.0;
            Ok(())
        } else {
            Err(std::time::Duration::from_secs_f64((1.0 - *left) / f64::from(PER_SECOND)))
        }
    }
}

/// A scope an application key may hold. Each is granted on its own: holding
/// `Write` grants nothing of `Operate`, and every key reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The status, the library, the proposals, the tasks and the pins.
    Read,
    /// A sync, a simulation, an apply or a revert.
    Operate,
    /// A pin set or removed.
    Write,
    /// The rules, the categories, the folder mappings, the declared
    /// destinations and the rule tests created, changed or removed: what
    /// decides where every title goes from the next simulation on.
    Configure,
}

/// What a valid application key allows, as the middleware hands it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub may_confirm: Vec<String>,
    pub may_move_files: bool,
    pub expires_at: Option<String>,
}

/// What a presented token turns out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    /// No live key, or a secret that does not match.
    Unknown,
    /// The right secret for a key past its expiry, which stays refused until revoked.
    Expired {
        id: String,
        name: String,
        expires_at: String,
    },
    Live(Grant),
}

impl Scope {
    /// The name the API reads and writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Operate => "operate",
            Self::Write => "write",
            Self::Configure => "configure",
        }
    }
}

impl Grant {
    pub fn allows(&self, scope: Scope) -> bool {
        scope == Scope::Read || self.scopes.contains(&scope)
    }

    /// Whether this key may answer the guardrail asking under `name`.
    pub fn may_answer(&self, name: &str) -> bool {
        self.may_confirm.iter().any(|granted| granted == name)
    }
}

/// An application key as the owner sees it, its secret aside.
#[derive(Debug, Serialize)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub may_confirm: Vec<String>,
    pub may_move_files: bool,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    pub created_at: String,
    pub created_by: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    pub last_used_at: Option<String>,
    /// Null for a key that never expires.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    pub expires_at: Option<String>,
    /// Whether the key is past its expiry, every call it makes refused.
    pub expired: bool,
}

/// What the owner asks for when making a key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewApplication {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub may_confirm: Vec<String>,
    #[serde(default)]
    pub may_move_files: bool,
    /// How many days the key is accepted for, from 1 to [`EXPIRY_DAYS_MAX`]. None for ever.
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

/// A key just made, with the token shown this once.
#[derive(Serialize)]
pub struct Minted {
    #[serde(flatten)]
    pub application: Application,
    pub token: String,
}

/// The token is the key itself, so a debug line shows the application only.
impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Minted")
            .field("application", &self.application)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    name: String,
    scopes: String,
    may_confirm: String,
    may_move_files: bool,
    created_at: String,
    created_by: Option<String>,
    last_used_at: Option<String>,
    expires_at: Option<String>,
    expired: bool,
}

impl Row {
    fn into_application(self) -> Application {
        Application {
            scopes: parse_list(&self.scopes),
            may_confirm: parse_list(&self.may_confirm),
            id: self.id,
            name: self.name,
            may_move_files: self.may_move_files,
            created_at: self.created_at,
            created_by: self.created_by,
            last_used_at: self.last_used_at,
            expires_at: self.expires_at,
            expired: self.expired,
        }
    }
}

/// A stored list, or none: a row edited by hand into something unreadable
/// grants nothing rather than failing every request that presents it.
fn parse_list<T: serde::de::DeserializeOwned>(stored: &str) -> Vec<T> {
    serde_json::from_str(stored).unwrap_or_default()
}

const COLUMNS: &str = "id, name, scopes, may_confirm, may_move_files, created_at, created_by,
     last_used_at, expires_at, COALESCE(expires_at <= datetime('now'), 0) AS expired";

/// The keys not revoked, expired ones included, newest first.
pub async fn list(pool: &SqlitePool) -> AppResult<Vec<Application>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM api_keys WHERE revoked_at IS NULL ORDER BY created_at DESC, name"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Row::into_application).collect())
}

/// Make a key and return it with its token, which is never readable again.
pub async fn create(
    state: &AppState,
    new: NewApplication,
    created_by: Option<&str>,
) -> AppResult<Minted> {
    let name = new.name.trim().to_string();
    if name.is_empty() {
        let refusal = state.localizer().await.translate("ErrorApplicationNameRequired", &[]);
        return Err(AppError::BadRequest(refusal));
    }
    if name.chars().count() > NAME_MAX {
        let refusal = state
            .localizer()
            .await
            .translate("ErrorApplicationNameTooLong", &[("max", &NAME_MAX.to_string())]);
        return Err(AppError::BadRequest(refusal));
    }
    // The names a person or the master key writes on what they ask for: a key
    // carrying one would read, on History, as them.
    let reserved = [crate::services::accounts::DEFAULT_USERNAME, "apikey", "anonymous"];
    if reserved.iter().any(|taken| taken.eq_ignore_ascii_case(&name)) {
        let refusal =
            state.localizer().await.translate("ErrorApplicationNameReserved", &[("name", &name)]);
        return Err(AppError::BadRequest(refusal));
    }
    if new.expires_in_days.is_some_and(|days| !(1..=EXPIRY_DAYS_MAX).contains(&days)) {
        return Err(AppError::BadRequest(format!(
            "expires_in_days must be between 1 and {EXPIRY_DAYS_MAX}"
        )));
    }
    if let Some(unknown) =
        new.may_confirm.iter().find(|name| !confirm::ALL.contains(&name.as_str()))
    {
        return Err(AppError::BadRequest(format!("No guardrail asks under '{unknown}'")));
    }

    let mut scopes: Vec<Scope> =
        new.scopes.into_iter().filter(|scope| *scope != Scope::Read).collect();
    scopes.sort_by_key(|scope| *scope as u8);
    scopes.dedup();
    let mut may_confirm = new.may_confirm;
    may_confirm.sort();
    may_confirm.dedup();

    let id = crate::crypto::generate_secret()?[..16].to_string();
    let secret = crate::crypto::generate_secret()?;
    let expires_at = new.expires_in_days.map(|days| {
        crate::services::routing::format_timestamp(
            chrono::Utc::now() + chrono::Duration::days(i64::from(days)),
        )
    });
    let inserted = sqlx::query(
        "INSERT INTO api_keys
            (id, name, secret_hash, scopes, may_confirm, may_move_files, created_by, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(digest(&secret))
    .bind(serde_json::to_string(&scopes)?)
    .bind(serde_json::to_string(&may_confirm)?)
    .bind(new.may_move_files)
    .bind(created_by)
    .bind(&expires_at)
    .execute(&state.pool)
    .await;
    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            let refusal =
                state.localizer().await.translate("ErrorApplicationNameTaken", &[("name", &name)]);
            return Err(AppError::Conflict { reason: "name_taken", message: refusal });
        }
        Err(e) => return Err(e.into()),
    }

    let row: Row =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM api_keys WHERE id = ?")))
            .bind(&id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Minted {
        application: row.into_application(),
        token: format!("{TOKEN_PREFIX}{id}_{secret}"),
    })
}

/// Revoke a live key. Its row stays, so what it wrote still names it.
pub async fn revoke(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let revoked = sqlx::query(
        "UPDATE api_keys SET revoked_at = datetime('now') WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    if revoked.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("No live application key has the id {id}")));
    }
    Ok(())
}

/// Revoke every live key, and say how many there were.
pub async fn revoke_all(pool: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("UPDATE api_keys SET revoked_at = datetime('now') WHERE revoked_at IS NULL")
        .execute(pool)
        .await?
        .rows_affected())
}

/// What a presented token allows. An expiry is told only once the secret matched, so a token
/// guessed for a known id learns nothing.
///
/// The last use is written at most once a minute, off the request: a caller
/// polling every second would otherwise put a write behind each read, and a
/// read that waits on a sync's transaction for a timestamp is a read nobody
/// asked to slow down.
pub async fn resolve(pool: &SqlitePool, token: &str) -> AppResult<Resolved> {
    let Some((id, secret)) = token.strip_prefix(TOKEN_PREFIX).and_then(|rest| rest.split_once('_'))
    else {
        return Ok(Resolved::Unknown);
    };

    #[derive(sqlx::FromRow)]
    struct Live {
        name: String,
        secret_hash: String,
        scopes: String,
        may_confirm: String,
        may_move_files: bool,
        last_used_at: Option<String>,
        expires_at: Option<String>,
        expired: bool,
    }
    let live: Option<Live> = sqlx::query_as(
        "SELECT name, secret_hash, scopes, may_confirm, may_move_files, last_used_at, expires_at,
                COALESCE(expires_at <= datetime('now'), 0) AS expired
         FROM api_keys WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(live) = live else {
        return Ok(Resolved::Unknown);
    };
    let matches: bool = digest(secret).as_bytes().ct_eq(live.secret_hash.as_bytes()).into();
    if !matches {
        return Ok(Resolved::Unknown);
    }
    if let (true, Some(expires_at)) = (live.expired, live.expires_at.clone()) {
        return Ok(Resolved::Expired { id: id.to_string(), name: live.name, expires_at });
    }

    if last_use_due(live.last_used_at.as_deref(), chrono::Utc::now()) {
        let (pool, id) = (pool.clone(), id.to_string());
        tokio::spawn(async move {
            let touched =
                sqlx::query("UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?")
                    .bind(&id)
                    .execute(&pool)
                    .await;
            if let Err(e) = touched {
                tracing::debug!("The last use of an application key went unrecorded: {e}");
            }
        });
    }

    Ok(Resolved::Live(Grant {
        id: id.to_string(),
        name: live.name,
        scopes: parse_list(&live.scopes),
        may_confirm: parse_list(&live.may_confirm),
        may_move_files: live.may_move_files,
        expires_at: live.expires_at,
    }))
}

/// Tell the owner of each key expiring within a week, once per key, and say how many.
pub async fn warn_of_expiry(state: &AppState) -> AppResult<usize> {
    let expiring: Vec<(String, String, String)> = sqlx::query_as(
        "UPDATE api_keys SET expiry_notified_at = datetime('now')
          WHERE revoked_at IS NULL AND expiry_notified_at IS NULL
            AND expires_at > datetime('now') AND expires_at <= datetime('now', ?)
         RETURNING id, name, expires_at",
    )
    .bind(EXPIRY_NOTICE)
    .fetch_all(&state.pool)
    .await?;
    for (id, name, expires_at) in &expiring {
        let event = crate::services::notify::Event::ApplicationKeyExpiring {
            id: id.clone(),
            name: name.clone(),
            expires_at: expires_at.clone(),
        };
        crate::services::notify::send_later(state, event);
    }
    Ok(expiring.len())
}

/// The stored form of a secret. One SHA-256 is enough: the secret is 256
/// random bits, which no dictionary holds and no slow hash would protect
/// further. The label keeps this digest from matching one made for anything
/// else.
fn digest(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"routarr:application-key:v1:");
    hasher.update(secret.as_bytes());
    hasher.finalize().iter().fold(String::with_capacity(64), |mut hex, byte| {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Whether a use at `now` is recorded: once a minute at most, or every call
/// a script makes would be a write.
fn last_use_due(last: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    last.and_then(crate::services::routing::parse_timestamp)
        .is_none_or(|last| now - last > chrono::Duration::minutes(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A use within the minute of the last one recorded leaves it, and one
    /// past it, or after none or one that does not read, is recorded.
    #[test]
    fn a_use_is_recorded_once_a_minute_at_most() {
        let now = crate::services::routing::parse_timestamp("2026-10-11 12:00:00").unwrap();
        assert!(last_use_due(None, now));
        assert!(!last_use_due(Some("2026-10-11 11:59:30"), now));
        assert!(!last_use_due(Some("2026-10-11 11:59:00"), now));
        assert!(last_use_due(Some("2026-10-11 11:58:59"), now));
        assert!(last_use_due(Some("not a time"), now));
    }

    fn grant(scopes: &[Scope]) -> Grant {
        Grant {
            id: "id".into(),
            name: "bot".into(),
            scopes: scopes.to_vec(),
            may_confirm: vec![confirm::BATCH.into()],
            may_move_files: false,
            expires_at: None,
        }
    }

    #[test]
    fn every_key_reads_and_each_other_scope_stands_alone() {
        let pinning = grant(&[Scope::Write]);
        assert!(pinning.allows(Scope::Read));
        assert!(pinning.allows(Scope::Write));
        assert!(!pinning.allows(Scope::Operate), "write grants nothing of operate");

        let operating = grant(&[Scope::Operate]);
        assert!(operating.allows(Scope::Operate));
        assert!(!operating.allows(Scope::Write), "operate grants nothing of write");
    }

    #[test]
    fn a_key_answers_only_the_guardrails_it_was_given() {
        let key = grant(&[Scope::Operate]);
        assert!(key.may_answer(confirm::BATCH));
        assert!(!key.may_answer(confirm::CAPACITY));
    }

    /// The digest of `a` as SHA-256 writes it, label included, in lower-case
    /// hex: a key stored by one build is admitted by the next.
    #[test]
    fn the_digest_is_the_labelled_sha256_in_lower_case_hex() {
        assert_eq!(digest("a"), "af935298aaa0bc96b8eee0e01dd228c76fc52bd2a142bc0d20c7c1e457383c19");
    }
}
