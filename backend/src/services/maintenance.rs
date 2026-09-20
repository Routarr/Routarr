//! Retention and housekeeping.
//!
//! Every simulation appends one decision row per actionable media item, so on a
//! large library the `decisions` table grows without bound. Retention keeps the
//! audit trail useful without letting the database balloon.

use std::collections::HashSet;

use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::jobs::JobKind;
use crate::state::AppState;

/// Bring stored `Bounded` settings inside the ranges this build enforces.
///
/// A bound added to a setting is retroactive in one direction and not the
/// other: `PUT /settings` validates the whole payload, and the Settings screen
/// always sends every field, so a value stored before the bound existed makes
/// *every* save fail — naming a key in a tab the operator never opened. The
/// converged value is the one that runs from then on, and the log says so.
///
/// A retention count is raised to its floor like any other and never lowered:
/// lowering it removes what is beyond it, and nothing but the operator's own
/// save may do that — `settings::bounds` answers no ceiling for one, and
/// `offline_warnings` names a stored value above it.
///
/// Converged at startup rather than in a migration, so the ranges stay stated
/// once, beside the settings they bound.
pub async fn converge_setting_bounds(state: &AppState) -> AppResult<usize> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings").fetch_all(&state.pool).await?;

    let mut converged = 0;
    for (key, value) in rows {
        let Some((min, max)) = crate::api::settings::bounds(&key) else {
            continue;
        };
        // A value that is not a number at all is left for `check` to refuse:
        // guessing one would be inventing a setting nobody chose.
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
            warn!("Setting '{key}' was {current}, below the minimum of {min}; stored as {min}");
        } else {
            warn!("Setting '{key}' was {current}, above the maximum of {max}; stored as {max}");
        }
        converged += 1;
    }
    Ok(converged)
}

/// Rewrite every stored secret under the current master key.
///
/// Runs at startup so an upgrade from a plaintext database, or a rotation of
/// `ROUTARR_SECRET_KEY`, converges without the user re-entering every API key.
pub async fn reseal_secrets(state: &AppState) -> AppResult<usize> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, api_key FROM instances").fetch_all(&state.pool).await?;

    let mut resealed = 0;
    for (id, stored) in rows {
        if !state.secrets.needs_reseal(&stored) {
            continue;
        }

        // A key we cannot open is left untouched: overwriting it would destroy
        // the only copy. The instance surfaces as unreachable instead.
        let Ok(plaintext) = state.secrets.open(&stored) else {
            tracing::error!(
                instance_id = %id,
                "Cannot decrypt this instance's API key — set ROUTARR_PREVIOUS_SECRET_KEY or re-enter it"
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

    // The metadata credentials, sealed the same way and living in `settings`.
    // Covering `instances` alone leaves them unreadable after a rotation — and
    // unlike an Arr, a metadata source failing to open shows up only as
    // conditions that quietly stop matching.
    let settings: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE '%\\_api\\_key' ESCAPE '\\'",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut settings_resealed = 0;
    for (key, stored) in settings {
        if stored.trim().is_empty() || !state.secrets.needs_reseal(&stored) {
            continue;
        }
        let Ok(plaintext) = state.secrets.open(&stored) else {
            tracing::error!(
                setting = %key,
                "Cannot decrypt this metadata key — set ROUTARR_PREVIOUS_SECRET_KEY or re-enter it"
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
        info!("Re-encrypted {settings_resealed} metadata key(s) under the current master key");
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
}

/// Purge stale rows according to the retention settings.
pub async fn run(state: &AppState, trigger: &str) -> AppResult<MaintenanceReport> {
    let Some(_lock) = state.jobs.try_lock("maintenance") else {
        return Ok(MaintenanceReport::default());
    };

    let job = state.jobs.start(JobKind::Maintenance, trigger, None, "Purging stale rows").await?;
    let outcome = purge(state).await;

    match &outcome {
        Ok(report) => {
            job.succeed(&format!(
                "{} decisions, {} logs, {} jobs removed",
                report.decisions_removed, report.logs_removed, report.jobs_removed
            ))
            .await
        }
        Err(e) => job.fail(&e.to_string()).await,
    }

    outcome
}

async fn purge(state: &AppState) -> AppResult<MaintenanceReport> {
    let decision_days: i64 = state.setting("decision_retention_days", 30).await;
    let log_days: i64 = state.setting("log_retention_days", 90).await;
    let pool = &state.pool;

    let mut report = MaintenanceReport::default();

    if decision_days > 0 {
        // Applied and failed decisions are the audit trail and are never purged
        // here, whatever the window: only proposals age out, and superseded
        // ones carry no history and go immediately.
        //
        // Every delete also spares a decision that carries an execution log.
        // `revert` sets a decision back to `skipped`, so a move that was really
        // written and then undone looks exactly like a stale proposal — and
        // `execution_logs.decision_id` has no `ON DELETE` clause, so SQLite
        // rejects the delete and the *whole* maintenance run fails, hourly and
        // for ever. A decision that caused a write is history, not a proposal.
        report.decisions_removed = delete_older_than(
            pool,
            "DELETE FROM decisions WHERE decided_at < datetime('now', ?)
                AND status IN ('pending', 'skipped')
                AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
            decision_days,
        )
        .await?;

        report.decisions_removed +=
            sqlx::query(
                "DELETE FROM decisions WHERE superseded = 1 AND status = 'pending'
                    AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
            )
                .execute(pool)
                .await?
                .rows_affected();
    }

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

    // Housekeeping rather than a guard: an expired row already fails the
    // lookup, this is what stops the table growing for ever.
    report.sessions_removed = super::accounts::purge_expired_sessions(pool).await?;
    // Sign-in attempts nobody came back from, swept for the same reason.
    super::oidc::purge_expired_flows(pool).await?;

    // `decisions` deliberately carries no foreign key on `media_id`: an applied
    // decision must outlive the media it moved, or the audit trail would erase
    // itself. The cost is orphans — delete an instance, or let a sync drop media
    // that vanished upstream, and its *unresolved* proposals stay behind. They
    // can never be applied (the executor joins `media`) and no simulation will
    // ever supersede them, so they sit in the pending count for ever, inflating
    // the one number the dashboard asks the user to act on.
    //
    // Applied and failed decisions are left alone here: those are the history.
    report.decisions_removed += sqlx::query(
        "DELETE FROM decisions
          WHERE status IN ('pending', 'skipped')
            AND NOT EXISTS (SELECT 1 FROM media m WHERE m.id = decisions.media_id)
            AND NOT EXISTS (SELECT 1 FROM execution_logs e WHERE e.decision_id = decisions.id)",
    )
    .execute(pool)
    .await?
    .rows_affected();

    // A resolution whose library key names no media any more would keep the
    // cache row it points at alive for ever, so it goes first and the cache
    // purge below follows it.
    report.source_identifiers_removed = prune_resolutions(pool).await?;

    // Metadata for media nobody tracks any more is dead weight.
    report.metadata_cache_removed = sqlx::query(
        // Matched across every identifier namespace: a source keyed on `imdb_id`
        // is as much a reason to keep a row as one keyed on `tmdb_id`, and an
        // id that collides across namespaces only means a row is kept slightly
        // too long — the harmless direction for a cache.
        // One NOT EXISTS per namespace, each an indexed seek on `media`: the same
        // test as a single OR, at 15 ms instead of 65 s on 20 000 titles.
        // A source addressed by search caches under *its own* identifier, which
        // no column of `media` holds: the fourth clause is what keeps AniList's
        // and Jikan's rows, and without it the purge re-fetched the whole anime
        // library every hour and every simulation in between ran blind.
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

    if report.decisions_removed + report.logs_removed + report.jobs_removed > 0 {
        info!(
            "Retention: removed {} decision(s), {} log(s), {} job(s), {} cache entrie(s)",
            report.decisions_removed,
            report.logs_removed,
            report.jobs_removed,
            report.metadata_cache_removed
        );
        // Not vacuuming on purpose: SQLite reuses the freed pages for the next
        // simulation, and a full VACUUM would rewrite the whole file while the
        // scheduler holds the pool.
    }

    Ok(report)
}

/// `&'static str` rather than `&str`, which is the whole guarantee: a caller
/// cannot reach this with a string it assembled at run time, so no audit is
/// needed here and none can be forgotten. The cut-off is bound, never
/// interpolated.
/// What `metadata::local_key_of` reads off a media row.
#[derive(sqlx::FromRow)]
struct LibraryIdentity {
    title: String,
    year: Option<i64>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
}

/// Drop the search resolutions whose library key names no media any more.
///
/// The key is what `metadata::local_key_of` computes from the media row, and
/// its title form has no SQL spelling, so the comparison is made here from the
/// same function rather than approximated in a `DELETE`.
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

    let mut removed = 0;
    let mut tx = pool.begin().await?;
    for (key,) in keys.iter().filter(|(key,)| !live.contains(key)) {
        removed += sqlx::query("DELETE FROM source_identifiers WHERE local_key = ?")
            .bind(key)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(removed)
}

async fn delete_older_than(pool: &SqlitePool, sql: &'static str, days: i64) -> AppResult<u64> {
    Ok(sqlx::query(sql).bind(format!("-{days} days")).execute(pool).await?.rows_affected())
}

#[cfg(test)]
mod tests {

    /// Reverting sets a decision to `skipped` (executor.rs), and a reverted
    /// decision necessarily carries the `move` and `revert` execution logs. The
    /// purge targets `skipped`, `execution_logs.decision_id` has no `ON DELETE`
    /// clause, and `foreign_keys` is ON — so the delete is rejected and the
    /// **whole** maintenance run fails. Every hour, for ever: retention stops
    /// working entirely.
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

    /// The instance and media rows a decision refers to. Minimal on purpose:
    /// these tests care about retention, not about routing.
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
    async fn superseded_pending_decisions_are_purged_immediately() {
        let state = AppState::for_tests().await;
        // A decision always points at a real media row; without one the orphan
        // purge would claim these before the superseded rule is exercised.
        seed_media(&state, "m1").await;

        for (id, superseded, decided_at) in
            [("keep-fresh", 0, "now"), ("drop-superseded", 1, "now")]
        {
            sqlx::query(
                "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
                 target_category, action, status, decided_at, superseded)
                 VALUES (?, 'm1', 'T', 'movie', 'i1', 'standard', 'move', 'pending', datetime(?), ?)",
            )
            .bind(id)
            .bind(decided_at)
            .bind(superseded)
            .execute(&state.pool)
            .await
            .unwrap();
        }

        purge(&state).await.unwrap();

        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT id FROM decisions").fetch_all(&state.pool).await.unwrap();
        assert_eq!(remaining, vec!["keep-fresh"]);
    }

    #[tokio::test]
    async fn applied_decisions_survive_the_pending_purge() {
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
    async fn legacy_plaintext_keys_are_encrypted_on_startup() {
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
        assert!(stored.starts_with("enc:v1:"));
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

    /// A source addressed by search caches under *its own* identifier, which
    /// matches no column of `media`; `source_identifiers` is the only thing
    /// tying that row to the library. Purging it re-searched and re-read the
    /// whole anime library every hour, and every simulation in between ran
    /// without those fields. The orphan beside it is the control: a purge that
    /// keeps everything would pass without it.
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
                "INSERT INTO metadata_cache (source, external_id, media_type, expires_at)
                 VALUES (?, ?, 'movie', '2030-01-01')",
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

    /// A resolution whose library key names no media any more would keep its
    /// cache row alive for ever, so it is pruned first and the cache follows.
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
                "INSERT INTO metadata_cache (source, external_id, media_type, expires_at)
                 VALUES ('anilist', ?, 'movie', '2030-01-01')",
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

    /// Deleting an instance takes its media with it, but `decisions` has no
    /// foreign key — so its unresolved proposals would otherwise sit in the
    /// pending count for ever, un-appliable and never superseded.
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

    /// The other half of the same rule: history survives its media.
    #[tokio::test]
    async fn an_applied_move_survives_the_media_it_moved() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id,
             instance_name, target_category, action, status, decided_at)
             VALUES ('d-applied', 'gone', 'Akira', 'movie', 'gone', 'Radarr', 'anime', 'move',
                     'applied', datetime('now'))",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        purge(&state).await.unwrap();

        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decisions")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(left, 1, "an applied decision is the audit trail");
    }
}
