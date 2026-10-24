//! What a source may still be sent today: its daily quota, or nothing once it blocked the address.
//!
//! OMDb answers a free key 1,000 requests a UTC day and refuses every one
//! after. The enrichment, the lookups of a placement or a webhook and the
//! probes all take from one count, kept in the database.

use sqlx::SqlitePool;

use crate::error::AppResult;

/// One source's requests for the day.
#[derive(Debug, Clone)]
pub struct DailyQuota {
    source: &'static str,
    limit: i64,
}

impl DailyQuota {
    pub fn new(source: &'static str, limit: i64) -> Self {
        Self { source, limit }
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    /// Reserve up to `wanted` of today's requests, counted as sent: how many
    /// may be sent, none once the day's quota is spent. The first request of
    /// a day starts a new count.
    pub async fn reserve(&self, pool: &SqlitePool, wanted: usize) -> AppResult<usize> {
        let mut tx = crate::db::write_transaction(pool).await?;
        let spent: i64 = sqlx::query_scalar(
            "SELECT spent FROM source_requests WHERE source = ? AND day = date('now')",
        )
        .bind(self.source)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(0);
        let granted = (self.limit - spent).clamp(0, i64::try_from(wanted).unwrap_or(i64::MAX));
        sqlx::query(
            "INSERT INTO source_requests (source, day, spent) VALUES (?, date('now'), ?)
             ON CONFLICT(source) DO UPDATE SET day = excluded.day, spent = excluded.spent",
        )
        .bind(self.source)
        .bind(spent + granted)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(usize::try_from(granted).unwrap_or(0))
    }

    /// Give back requests reserved today and never sent. Given back after the
    /// day they were reserved, they are nothing to give: a new count began.
    pub async fn give_back(&self, pool: &SqlitePool, unsent: usize) -> AppResult<()> {
        sqlx::query(
            "UPDATE source_requests SET spent = MAX(spent - ?, 0)
              WHERE source = ? AND day = date('now')",
        )
        .bind(i64::try_from(unsent).unwrap_or(i64::MAX))
        .bind(self.source)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Count today's quota spent, when the source refuses for its quota before
    /// the count says so: another program may share the key. A quota raised
    /// later the same day opens again.
    pub async fn exhaust(&self, pool: &SqlitePool) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO source_requests (source, day, spent) VALUES (?, date('now'), ?)
             ON CONFLICT(source) DO UPDATE SET
                spent = MAX(CASE WHEN day = excluded.day THEN spent ELSE 0 END, excluded.spent),
                day = excluded.day",
        )
        .bind(self.source)
        .bind(self.limit)
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// A source that refuses an address for the rest of the day once it blocks it, as Tenrai's
/// anti-abuse 403 does: nothing more is sent until the next UTC day. A row of `source_requests`
/// dated today holds it, as it holds OMDb's spent day, so a restart keeps it.
#[derive(Debug, Clone)]
pub struct DayHold {
    source: &'static str,
}

impl DayHold {
    pub fn new(source: &'static str) -> Self {
        Self { source }
    }

    /// Whether the source refused for the rest of today. Unreadable, it is asked: a request
    /// refused again costs less than a source silenced on a database error.
    pub async fn holds(&self, pool: &SqlitePool) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM source_requests WHERE source = ? AND day = date('now'))",
        )
        .bind(self.source)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
    }

    /// Hold the source back until the next UTC day.
    pub async fn begin(&self, pool: &SqlitePool) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO source_requests (source, day, spent) VALUES (?, date('now'), 0)
             ON CONFLICT(source) DO UPDATE SET day = excluded.day",
        )
        .bind(self.source)
        .execute(pool)
        .await?;
        Ok(())
    }
}
