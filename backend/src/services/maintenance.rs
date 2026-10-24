//! Retention and housekeeping: every simulation writes a decision per actionable title, and
//! retention keeps the audit trail without letting the database grow without bound.

use std::collections::HashSet;

use sqlx::{AssertSqlSafe, SqlitePool};
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::services::metadata;
use crate::state::AppState;

/// What a start brings in line before anything reads the database: secrets sealed under the
/// current key, stored values inside this build's bounds, and the source list the environment
/// asks for. `main` calls this, so a test runs what a start runs.
pub async fn converge(state: &AppState) -> AppResult<()> {
    reseal_secrets(state).await?;
    converge_setting_bounds(state).await?;
    converge_metadata_sources(state).await?;
    Ok(())
}

/// List TMDB when the environment hands in its key and nobody chose the sources yet: the
/// shipped order is the Arr alone, and a stored list is a choice the environment must not undo.
/// Converged at startup, since routing reads the stored list and never the environment.
pub async fn converge_metadata_sources(state: &AppState) -> AppResult<bool> {
    if state.config.tmdb_api_key.is_none() {
        return Ok(false);
    }
    let listed = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('metadata_providers', ?)
         ON CONFLICT(key) DO NOTHING",
    )
    .bind(format!("{},{}", metadata::ARR, metadata::TMDB))
    .execute(&state.pool)
    .await?
    .rows_affected()
        > 0;
    if listed {
        info!("TMDB_API_KEY is set, so TMDB joins the metadata sources after Radarr and Sonarr");
    }
    Ok(listed)
}

/// Bring stored `Bounded` settings inside this build's ranges: the Settings screen sends every
/// field, so one value outside its range fails every save. A retention count is raised to its
/// floor and never lowered, which would delete what lies beyond it (`settings::bounds` answers
/// no ceiling for one). At startup, so the ranges stay stated once, beside their settings.
pub async fn converge_setting_bounds(state: &AppState) -> AppResult<usize> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings").fetch_all(&state.pool).await?;

    let mut converged = 0;
    for (key, value) in rows {
        let Some((min, max)) = crate::services::settings::bounds(&key) else {
            continue;
        };
        // Not a number: left for `check` to refuse, since guessing one invents a setting.
        let Ok(current) = value.trim().parse::<i64>() else {
            continue;
        };
        let clamped = current.clamp(min, max);
        if clamped == current {
            continue;
        }

        sqlx::query("UPDATE settings SET value = ?, updated_at = datetime('now') WHERE key = ?")
            .bind(clamped.to_string())
            .bind(&key)
            .execute(&state.pool)
            .await?;
        if current < min {
            warn!("Setting '{key}' was {current}, below the minimum of {min}, stored as {min}");
        } else {
            warn!("Setting '{key}' was {current}, above the maximum of {max}, stored as {max}");
        }
        converged += 1;
    }
    Ok(converged)
}

/// Rewrite every stored secret under the current master key, so a plaintext value or one
/// sealed under `ROUTARR_PREVIOUS_SECRET_KEY` needs no entering again.
pub async fn reseal_secrets(state: &AppState) -> AppResult<usize> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, api_key FROM instances").fetch_all(&state.pool).await?;

    let mut resealed = 0;
    for (id, stored) in rows {
        if !state.secrets.needs_reseal(&stored) {
            continue;
        }

        // Left untouched: overwriting a key that cannot be opened destroys the only copy.
        let Ok(plaintext) = state.secrets.open(&stored) else {
            tracing::error!(
                instance_id = %id,
                "Cannot decrypt this instance's API key. Set ROUTARR_PREVIOUS_SECRET_KEY or enter it again"
            );
            continue;
        };

        sqlx::query("UPDATE instances SET api_key = ? WHERE id = ?")
            .bind(state.secrets.seal(&plaintext)?)
            .bind(&id)
            .execute(&state.pool)
            .await?;
        resealed += 1;
    }

    if resealed > 0 {
        info!("Re-encrypted {resealed} instance API key(s) under the current master key");
    }

    // The notification's signing secrets: one left under a retired key signs nothing, and the
    // receiver refuses every message without a word here.
    let secrets: Vec<(i64, String)> =
        sqlx::query_as("SELECT rowid, secret FROM webhook_secrets").fetch_all(&state.pool).await?;
    for (rowid, stored) in secrets {
        if !state.secrets.needs_reseal(&stored) {
            continue;
        }
        let Ok(plaintext) = state.secrets.open(&stored) else {
            tracing::error!("Cannot decrypt a webhook signing secret. Generate a new one");
            continue;
        };
        sqlx::query("UPDATE webhook_secrets SET secret = ? WHERE rowid = ?")
            .bind(state.secrets.seal(&plaintext)?)
            .bind(rowid)
            .execute(&state.pool)
            .await?;
        resealed += 1;
    }

    // The settings `KNOWN` seals: one left under a retired key shows only as conditions that
    // stop matching or notifications that stop arriving.
    let keys = crate::services::settings::sealed_keys();
    let mut query = sqlx::query_as::<_, (String, String)>(AssertSqlSafe(format!(
        "SELECT key, value FROM settings WHERE key IN ({})",
        crate::db::placeholders(keys.len())
    )));
    for key in &keys {
        query = query.bind(*key);
    }
    let settings = query.fetch_all(&state.pool).await?;

    let mut settings_resealed = 0;
    for (key, stored) in settings {
        if stored.trim().is_empty() || !state.secrets.needs_reseal(&stored) {
            continue;
        }
        let Ok(plaintext) = state.secrets.open(&stored) else {
            tracing::error!(
                setting = %key,
                "Cannot decrypt this setting. Set ROUTARR_PREVIOUS_SECRET_KEY or enter it again"
            );
            continue;
        };
        sqlx::query("UPDATE settings SET value = ?, updated_at = datetime('now') WHERE key = ?")
            .bind(state.secrets.seal(&plaintext)?)
            .bind(&key)
            .execute(&state.pool)
            .await?;
        settings_resealed += 1;
    }

    if settings_resealed > 0 {
        info!("Re-encrypted {settings_resealed} sealed setting(s) under the current master key");
    }
    let resealed = resealed + settings_resealed;
    Ok(resealed)
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct MaintenanceReport {
    pub decisions_removed: u64,
    pub logs_removed: u64,
    pub jobs_removed: u64,
    pub metadata_cache_removed: u64,
    pub source_identifiers_removed: u64,
    pub sessions_removed: u64,
    pub security_events_removed: u64,
}

/// Purge stale rows according to the retention settings.
pub async fn run(state: &AppState, by: &Attribution) -> AppResult<MaintenanceReport> {
    let Some(_lock) = state.jobs.try_lock("maintenance") else {
        let message = state.localizer().await.translate("ErrorPurgeRunning", &[]);
        return Err(AppError::InProgress { reason: "purge_running", message });
    };

    let mut job =
        state.jobs.start(JobKind::Maintenance, by, None, Detail::new("JobDetailPurging")).await?;
    crate::race::checked("maintenance::run", "").await;
    let outcome = purge(state).await;

    // A connection the pool never closes would never refresh the planner's statistics.
    if let Err(e) = sqlx::query("PRAGMA optimize").execute(&state.pool).await {
        warn!("Could not refresh the query planner's statistics: {e}");
    }
    // SQLite folds the write-ahead log only past a thousand pages, and a copy of the file alone
    // would miss everything since, up to the whole schema. The archives are the safe copy.
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)").execute(&state.pool).await {
        warn!("Could not fold the write-ahead log into the database: {e}");
    }

    match &outcome {
        Ok(report) => {
            job.report(report);
            job.succeed(
                Detail::new("JobDetailPurged")
                    .with("decisions", report.decisions_removed)
                    .with("logs", report.logs_removed)
                    .with("jobs", report.jobs_removed),
            )
            .await
        }
        Err(e) => job.fail(e).await,
    }

    outcome
}

async fn purge(state: &AppState) -> AppResult<MaintenanceReport> {
    let decision_days: i64 = state.bounding_setting("decision_retention_days").await?;
    let log_days: i64 = state.bounding_setting("log_retention_days").await?;
    let security_days: i64 = state.bounding_setting("security_log_retention_days").await?;
    let pool = &state.pool;

    let mut report = MaintenanceReport::default();

    // Logs first: a decision a log names is kept, so one whose last log goes
    // now goes in this same pass rather than at the next.
    if log_days > 0 {
        report.logs_removed = delete_older_than(
            pool,
            "DELETE FROM execution_logs WHERE executed_at < datetime('now', ?)",
            log_days,
        )
        .await?;

        report.jobs_removed = delete_older_than(
            pool,
            "DELETE FROM jobs WHERE status != 'running' AND started_at < datetime('now', ?)",
            log_days,
        )
        .await?;
    }

    if decision_days > 0 {
        // Applied decisions are the audit trail, never purged. A decision a log names, a
        // reverted move or a failure, goes with its last log: `execution_logs.decision_id` has
        // no `ON DELETE`, so deleting it first would fail the whole run, every hour.
        report.decisions_removed = delete_older_than(
            pool,
            "DELETE FROM decisions WHERE decided_at < datetime('now', ?)
                AND status IN ('pending', 'skipped', 'failed')
                AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
            decision_days,
        )
        .await?;
    }

    // A superseded proposal carries no history and goes whatever the window, a retention of 0
    // included, or a pass writing a row per title would grow the table without end.
    report.decisions_removed += sqlx::query(
        "DELETE FROM decisions WHERE superseded = 1 AND status = 'pending'
            AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
    )
    .execute(pool)
    .await?
    .rows_affected();

    // An expired session already fails the lookup: this only stops the table growing.
    report.sessions_removed = super::accounts::purge_expired_sessions(pool).await?;

    if security_days > 0 {
        report.security_events_removed = delete_older_than(
            pool,
            "DELETE FROM security_events WHERE at < datetime('now', ?)",
            security_days,
        )
        .await?;
    }

    // `decisions` has no foreign key on `media_id`, so an applied decision outlives its media.
    // A proposal or a skip whose media is gone can never be applied, and goes here. Applied
    // and failed decisions are the history.
    report.decisions_removed += sqlx::query(
        "DELETE FROM decisions
          WHERE status IN ('pending', 'skipped')
            AND NOT EXISTS (SELECT 1 FROM media m WHERE m.id = decisions.media_id)
            AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
    )
    .execute(pool)
    .await?
    .rows_affected();

    // A resolution naming no media would keep its cache row alive for ever, so it goes first.
    report.source_identifiers_removed = prune_resolutions(pool).await?;

    report.metadata_cache_removed = sqlx::query(
        // Matched across every namespace, an id colliding across them only keeping a row longer.
        // One NOT EXISTS per namespace is an indexed seek, where a single OR scans `media` per row.
        // A source addressed by search caches under its own id, which only `source_identifiers`
        // ties to the library: without that clause, the anime library is fetched every hour.
        "DELETE FROM metadata_cache
          WHERE NOT EXISTS (SELECT 1 FROM media m WHERE m.media_type = metadata_cache.media_type
                              AND m.tmdb_id = CAST(metadata_cache.external_id AS INTEGER)
                              AND metadata_cache.external_id GLOB '[0-9]*')
            AND NOT EXISTS (SELECT 1 FROM media m WHERE m.media_type = metadata_cache.media_type
                              AND m.tvdb_id = CAST(metadata_cache.external_id AS INTEGER)
                              AND metadata_cache.external_id GLOB '[0-9]*')
            AND NOT EXISTS (SELECT 1 FROM media m WHERE m.media_type = metadata_cache.media_type
                              AND m.imdb_id = metadata_cache.external_id)
            AND NOT EXISTS (SELECT 1 FROM source_identifiers s
                             WHERE s.source = metadata_cache.source
                               AND s.media_type = metadata_cache.media_type
                               AND s.external_id = metadata_cache.external_id)",
    )
    .execute(pool)
    .await?
    .rows_affected();

    // TMDB's terms forbid keeping its answers past six months, whether a key
    // still refreshes them or not.
    report.metadata_cache_removed += sqlx::query(
        "DELETE FROM metadata_cache WHERE source = ? AND cached_at < datetime('now', ?)",
    )
    .bind(super::metadata::TMDB)
    .bind(format!("-{} days", super::metadata::TMDB_CACHE_DAYS))
    .execute(pool)
    .await?
    .rows_affected();

    let removed = [
        report.decisions_removed,
        report.logs_removed,
        report.jobs_removed,
        report.metadata_cache_removed,
        report.source_identifiers_removed,
        report.sessions_removed,
        report.security_events_removed,
    ];
    if removed.iter().any(|count| *count > 0) {
        info!(
            "Retention removed decisions: {}, logs: {}, tasks: {}, cache rows: {}, resolutions: \
             {}, sessions: {}, security events: {}",
            removed[0], removed[1], removed[2], removed[3], removed[4], removed[5], removed[6]
        );
        // No VACUUM: SQLite reuses the freed pages, and a VACUUM would rewrite the whole file
        // while the scheduler holds the pool.
    }

    Ok(report)
}

/// What `metadata::local_key_of` reads off a media row.
#[derive(sqlx::FromRow)]
struct LibraryIdentity {
    title: String,
    year: Option<i64>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
}

/// Drop the search resolutions whose library key names no media any more. The key's title form
/// has no SQL spelling, so it comes from `metadata::local_key_of`, not from a `DELETE`.
async fn prune_resolutions(pool: &SqlitePool) -> AppResult<u64> {
    let keys: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT local_key FROM source_identifiers").fetch_all(pool).await?;
    if keys.is_empty() {
        return Ok(0);
    }

    let rows: Vec<LibraryIdentity> =
        sqlx::query_as("SELECT title, year, tmdb_id, tvdb_id, imdb_id FROM media")
            .fetch_all(pool)
            .await?;
    let live: HashSet<String> = rows
        .iter()
        .map(|row| {
            super::metadata::local_key_of(
                row.tmdb_id,
                row.tvdb_id,
                row.imdb_id.as_deref(),
                &row.title,
                row.year,
            )
        })
        .collect();

    // A chunk at a time, each its own write: one transaction would hold the write lock the
    // whole prune long, and a key a sync brings back meanwhile only costs the next search.
    let stale: Vec<&str> =
        keys.iter().map(|(key,)| key.as_str()).filter(|key| !live.contains(*key)).collect();
    let mut removed = 0;
    for chunk in stale.chunks(crate::services::routing::BIND_CHUNK) {
        let sql = format!(
            "DELETE FROM source_identifiers WHERE local_key IN ({})",
            crate::db::placeholders(chunk.len())
        );
        let mut query = sqlx::query(AssertSqlSafe(sql.as_str()));
        for key in chunk {
            query = query.bind(*key);
        }
        removed += query.execute(pool).await?.rows_affected();
    }
    Ok(removed)
}

/// `&'static str`, so no caller can pass a statement assembled at run time. The cut-off is
/// bound, never interpolated.
async fn delete_older_than(pool: &SqlitePool, sql: &'static str, days: i64) -> AppResult<u64> {
    Ok(sqlx::query(sql).bind(format!("-{days} days")).execute(pool).await?.rows_affected())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_decision_that_caused_a_write_does_not_break_the_purge() {
        let state = crate::state::AppState::for_tests().await;

        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                target_category, action, status, decided_at)
             VALUES ('d-1', 'gone', 'Akira', 'movie', 'inst-1', 'anime', 'move', 'skipped',
                     datetime('now'))",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        for (id, action) in [("l-1", "move"), ("l-2", "revert")] {
            sqlx::query(
                "INSERT INTO execution_logs (id, decision_id, action, success, executed_at)
                 VALUES (?, 'd-1', ?, 1, datetime('now'))",
            )
            .bind(id)
            .bind(action)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        // The media is gone, so the orphan sweep targets this decision.
        purge(&state).await.expect("the purge must not fail on an audited decision");

        let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions WHERE id = 'd-1'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(kept, 1, "a decision that was actually written and reverted is history");

        let logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_logs")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(logs, 2, "the audit trail must survive");
    }

    use super::*;

    /// The instance and media rows a decision refers to.
    async fn seed_media(state: &AppState, id: &str) {
        sqlx::query(
            "INSERT OR IGNORE INTO instances (id, name, instance_type, base_url, api_key,
             webhook_token)
             VALUES ('i1', 'Radarr', 'radarr', 'http://radarr:7878', 'k', 'tok')",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title)
             VALUES (?, 'i1', 1, 'movie', 'T')",
        )
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_reverted_or_failed_move_goes_in_the_pass_that_removes_its_log() {
        let state = crate::state::AppState::for_tests().await;
        seed_media(&state, "m-1").await;
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                target_category, action, status, decided_at, reverted_at)
             VALUES ('d-1', 'm-1', 'T', 'movie', 'i1', 'anime', 'move', 'skipped',
                     datetime('now', '-40 days'), datetime('now', '-40 days')),
                    ('d-2', 'm-1', 'T', 'movie', 'i1', 'anime', 'move', 'failed',
                     datetime('now', '-40 days'), NULL)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        for (id, decision, action, success) in
            [("l-1", "d-1", "move", 1), ("l-2", "d-1", "revert", 1), ("l-3", "d-2", "move", 0)]
        {
            sqlx::query(
                "INSERT INTO execution_logs (id, decision_id, action, success, executed_at)
                 VALUES (?, ?, ?, ?, datetime('now', '-40 days'))",
            )
            .bind(id)
            .bind(decision)
            .bind(action)
            .bind(success)
            .execute(&state.pool)
            .await
            .unwrap();
        }
        let kept = || async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM decisions")
                .fetch_one(&state.pool)
                .await
                .unwrap()
        };
        let retain_logs = |days: &'static str| {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ('log_retention_days', ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(days)
            .execute(&state.pool)
        };

        retain_logs("90").await.unwrap();
        purge(&state).await.unwrap();
        assert_eq!(kept().await, 2, "a move went while its log remained");

        retain_logs("30").await.unwrap();
        purge(&state).await.unwrap();
        assert_eq!(kept().await, 0, "a move outlived the log that named it");
    }

    #[tokio::test]
    async fn superseded_pending_decisions_are_purged_immediately() {
        for retention in ["30", "0"] {
            let state = AppState::for_tests().await;
            // Without the media row, the orphan purge would claim these first.
            seed_media(&state, "m1").await;
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('decision_retention_days', ?)")
                .bind(retention)
                .execute(&state.pool)
                .await
                .unwrap();

            for (id, superseded) in [("keep-fresh", 0), ("drop-superseded", 1)] {
                sqlx::query(
                    "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                     target_category, action, status, decided_at, superseded)
                     VALUES (?, 'm1', 'T', 'movie', 'i1', 'standard', 'move', 'pending',
                             datetime('now'), ?)",
                )
                .bind(id)
                .bind(superseded)
                .execute(&state.pool)
                .await
                .unwrap();
            }

            purge(&state).await.unwrap();

            let remaining: Vec<String> = sqlx::query_scalar("SELECT id FROM decisions")
                .fetch_all(&state.pool)
                .await
                .unwrap();
            assert_eq!(remaining, vec!["keep-fresh"], "retention {retention}");
        }
    }

    /// A retention the database fails to read deletes nothing: taken for the
    /// default, it would delete what a longer retention keeps.
    #[tokio::test]
    async fn a_retention_that_cannot_be_read_purges_nothing() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO execution_logs (id, action, success, executed_at)
             VALUES ('l-1', 'move', 1, datetime('now', '-400 days'))",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query("ALTER TABLE settings RENAME TO settings_unreadable")
            .execute(&state.pool)
            .await
            .unwrap();

        assert!(purge(&state).await.is_err(), "the purge went ahead");
        let logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_logs")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(logs, 1);
    }

    /// History outlives both its age and its media: `m1` is never seeded.
    #[tokio::test]
    async fn an_applied_decision_outlives_its_retention_and_its_media() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
             target_category, action, status, decided_at)
             VALUES ('old-applied', 'm1', 'T', 'movie', 'i1', 'standard', 'move', 'applied',
                     datetime('now', '-400 days'))",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        purge(&state).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "the audit trail must not be purged by the pending sweep");
    }

    #[tokio::test]
    async fn a_plaintext_key_is_sealed_at_startup() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key)
             VALUES ('i1', 'Radarr', 'radarr', 'http://x', 'plaintext-key')",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        assert_eq!(reseal_secrets(&state).await.unwrap(), 1);

        let stored: String = sqlx::query_scalar("SELECT api_key FROM instances WHERE id = 'i1'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert!(stored.starts_with("enc:v2:"));
        assert_eq!(state.secrets.open(&stored).unwrap(), "plaintext-key");

        // Idempotent: a second pass has nothing left to do.
        assert_eq!(reseal_secrets(&state).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn an_undecryptable_key_is_left_alone_rather_than_destroyed() {
        let state = AppState::for_tests().await;
        // Sealed with a key this instance does not have.
        let foreign = crate::crypto::SecretBox::load(
            Some("some-other-key"),
            None,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .unwrap()
        .seal("secret")
        .unwrap();

        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key)
             VALUES ('i1', 'Radarr', 'radarr', 'http://x', ?)",
        )
        .bind(&foreign)
        .execute(&state.pool)
        .await
        .unwrap();

        assert_eq!(reseal_secrets(&state).await.unwrap(), 0);

        let stored: String = sqlx::query_scalar("SELECT api_key FROM instances WHERE id = 'i1'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(stored, foreign, "the only copy of the secret must survive");
    }

    async fn cache_rows(state: &AppState) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT source, external_id FROM metadata_cache ORDER BY source, external_id",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap()
    }

    /// The orphan beside it is the control: a purge keeping every row would pass.
    #[tokio::test]
    async fn metadata_resolved_by_search_survives_the_purge() {
        let state = AppState::for_tests().await;
        seed_media(&state, "m1").await;
        sqlx::query("UPDATE media SET tmdb_id = 500 WHERE id = 'm1'")
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
             VALUES ('anilist', 'movie', 'tmdb:500', '47')",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        for (source, id) in [("anilist", "47"), ("tmdb", "500"), ("tmdb", "42")] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type)
                 VALUES (?, ?, 'movie')",
            )
            .bind(source)
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        let report = purge(&state).await.unwrap();

        assert_eq!(report.metadata_cache_removed, 1, "only the orphan goes");
        assert_eq!(
            cache_rows(&state).await,
            vec![("anilist".into(), "47".into()), ("tmdb".into(), "500".into())]
        );
    }

    #[tokio::test]
    async fn a_tmdb_answer_older_than_six_months_goes_whatever_names_it() {
        let state = AppState::for_tests().await;
        seed_media(&state, "m1").await;
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, tmdb_id)
             VALUES ('m2', 'i1', 2, 'movie', 'T', 501)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE media SET tmdb_id = 500, imdb_id = 'ttm1' WHERE id = 'm1'")
            .execute(&state.pool)
            .await
            .unwrap();
        for (source, id, age) in [
            ("tmdb", "500", "-181 days"),
            ("tmdb", "501", "-179 days"),
            ("omdb", "ttm1", "-900 days"),
        ] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type, cached_at)
                 VALUES (?, ?, 'movie', datetime('now', ?))",
            )
            .bind(source)
            .bind(id)
            .bind(age)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        let report = purge(&state).await.unwrap();

        assert_eq!(report.metadata_cache_removed, 1);
        assert_eq!(
            cache_rows(&state).await,
            vec![("omdb".into(), "ttm1".into()), ("tmdb".into(), "501".into())]
        );
    }

    /// Each namespace keeps its own rows: a series cached by its TheTVDB id, a
    /// film by its IMDb id, and the same numbers under the other type go.
    #[tokio::test]
    async fn the_cache_keeps_what_a_tvdb_or_imdb_id_still_names() {
        let state = AppState::for_tests().await;
        seed_media(&state, "m1").await;
        sqlx::query(
            "UPDATE media SET media_type = 'series', tvdb_id = 76885, imdb_id = 'tt0213338'
             WHERE id = 'm1'",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        for (source, id, kind) in [
            ("tvdb", "76885", "series"),
            ("omdb", "tt0213338", "series"),
            ("tvdb", "76885", "movie"),
            ("omdb", "tt0000001", "series"),
        ] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type)
                 VALUES (?, ?, ?)",
            )
            .bind(source)
            .bind(id)
            .bind(kind)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        let report = purge(&state).await.unwrap();

        assert_eq!(report.metadata_cache_removed, 2, "the orphans of either namespace stayed");
        assert_eq!(
            cache_rows(&state).await,
            vec![("omdb".into(), "tt0213338".into()), ("tvdb".into(), "76885".into())]
        );
    }

    #[tokio::test]
    async fn a_search_resolution_for_vanished_media_is_dropped_with_its_cache() {
        let state = AppState::for_tests().await;
        seed_media(&state, "m1").await;
        sqlx::query("UPDATE media SET tmdb_id = 500 WHERE id = 'm1'")
            .execute(&state.pool)
            .await
            .unwrap();
        for (key, id) in
            [("tmdb:500", Some("47")), ("tmdb:999", Some("48")), ("title:gone|2001", None)]
        {
            sqlx::query(
                "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
                 VALUES ('anilist', 'movie', ?, ?)",
            )
            .bind(key)
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        }
        for id in ["47", "48"] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type)
                 VALUES ('anilist', ?, 'movie')",
            )
            .bind(id)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        let report = purge(&state).await.unwrap();

        assert_eq!(report.source_identifiers_removed, 2);
        assert_eq!(report.metadata_cache_removed, 1);
        let keys: Vec<(String,)> = sqlx::query_as("SELECT local_key FROM source_identifiers")
            .fetch_all(&state.pool)
            .await
            .unwrap();
        assert_eq!(keys, vec![("tmdb:500".into(),)]);
        assert_eq!(cache_rows(&state).await, vec![("anilist".into(), "47".into())]);
    }

    #[tokio::test]
    async fn a_proposal_for_media_that_no_longer_exists_is_dropped() {
        let state = AppState::for_tests().await;
        for (id, status) in [("d-pending", "pending"), ("d-skipped", "skipped")] {
            sqlx::query(
                "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                 instance_name, target_category, action, status, decided_at)
                 VALUES (?, 'gone', 'Akira', 'movie', 'gone', 'Radarr', 'anime', 'move', ?,
                         datetime('now'))",
            )
            .bind(id)
            .bind(status)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        let report = purge(&state).await.unwrap();

        assert_eq!(report.decisions_removed, 2);
    }
}
