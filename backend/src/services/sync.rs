//! Pull media and root folders from the configured Arr instances.

use futures::StreamExt;
use sqlx::{AssertSqlSafe, SqlitePool};
use tracing::{error, info};

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::JobKind;
use crate::models::Instance;
use crate::services::notify;
use crate::state::AppState;

/// Result of syncing one instance.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncReport {
    pub instance_id: String,
    pub instance_name: String,
    pub root_folders: usize,
    pub media: usize,
    /// Media rows that disappeared upstream and were removed locally.
    pub removed: u64,
    /// Set when the sync failed — the caller of sync-all still gets one entry
    /// per instance instead of the failure silently vanishing from the report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How many instances are synced at once.
///
/// Bounded, and bounded *below* the connection pool's eight. What overlaps
/// here is the waiting on someone else's network, but each sync still ends by
/// holding a connection for its write transaction, and a pass that took every
/// connection would stall the interface polling `/status` behind a 15-second
/// acquire timeout — on the screen someone is watching precisely because a
/// sync is running.
const SYNC_CONCURRENCY: usize = 4;

/// Synchronize every enabled instance, isolating per-instance failures.
pub async fn sync_all_instances(state: &AppState, trigger: &str) -> AppResult<Vec<SyncReport>> {
    let instances = state.instances(true).await?;
    info!("Syncing {} enabled instance(s)", instances.len());

    // Concurrently. `do_sync` fetches root folders, media and tags *before* it
    // opens its transaction, so what overlaps is the network wait and not the
    // writing — the transactions still land one at a time under SQLite's single
    // writer. `buffered` rather than `buffer_unordered`: the reports are what
    // the API returns and what the interface lists, and rows that shuffle
    // between two runs are a table nobody can read.

    let reports = futures::stream::iter(instances.into_iter().map(|instance| async move {
        match sync_instance_inner(state, &instance, trigger).await {
            Ok(report) => report,
            Err(e) => {
                // One unreachable Radarr must not stop the Sonarr sync — but the
                // caller still hears about it: last_sync_status alone only helps
                // someone already looking at the instance list.
                error!("Failed to sync instance '{}': {e}", instance.name);
                SyncReport {
                    instance_id: instance.id,
                    instance_name: instance.name,
                    error: Some(e.to_string()),
                    ..Default::default()
                }
            }
        }
    }))
    .buffered(SYNC_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    Ok(reports)
}

/// Synchronize a single instance by id.
pub async fn sync_instance(
    state: &AppState,
    instance_id: &str,
    trigger: &str,
) -> AppResult<SyncReport> {
    let instance = state.instance(instance_id).await?;
    sync_instance_inner(state, &instance, trigger).await
}

async fn sync_instance_inner(
    state: &AppState,
    instance: &Instance,
    trigger: &str,
) -> AppResult<SyncReport> {
    // Refuse to pile up concurrent syncs of the same instance — the scheduler and
    // a user clicking "Sync" would otherwise fight over the same rows.
    let Some(_lock) = state.jobs.try_lock(&format!("sync:{}", instance.id)) else {
        return Err(AppError::Conflict(format!(
            "A sync is already running for instance '{}'",
            instance.name
        )));
    };

    let job = state
        .jobs
        .start(JobKind::Sync, trigger, Some(&instance.id), &format!("Syncing {}", instance.name))
        .await?;

    // Read before writing: notifications fire on the transition, not on the
    // state. An Arr that has been down for a week must not produce a message on
    // every scheduler tick — that is how a useful alert becomes noise the
    // operator mutes, which is worse than having none.
    let was_failing = instance.last_sync_status.as_deref().is_some_and(|s| s.starts_with("error"));

    let outcome = do_sync(state, instance).await;

    match &outcome {
        Ok(report) => {
            update_sync_status(&state.pool, &instance.id, "success").await;
            job.succeed(&format!("{} media, {} root folders", report.media, report.root_folders))
                .await;
            if was_failing {
                notify::send(
                    state,
                    notify::Event::InstanceRecovered { instance: instance.name.clone() },
                )
                .await;
            }
        }
        Err(e) => {
            update_sync_status(&state.pool, &instance.id, &format!("error: {e}")).await;
            job.fail(&e.to_string()).await;
            if !was_failing {
                notify::send(
                    state,
                    notify::Event::InstanceUnreachable {
                        instance: instance.name.clone(),
                        error: e.to_string(),
                    },
                )
                .await;
            }
        }
    }

    outcome
}

async fn do_sync(state: &AppState, instance: &Instance) -> AppResult<SyncReport> {
    let adapter = state.adapter(instance)?;

    // Before the first request, not after: everything below describes the Arr as
    // it was at *this* instant, and an apply that lands while we are reading
    // makes what we are about to write stale. `upsert_media` compares this
    // against `moved_at` and declines to put an old path back — migration 007.
    let read_at = crate::services::routing::format_timestamp(chrono::Utc::now());

    let root_folders = adapter.get_root_folders().await?;
    let media = adapter.get_media().await?;

    // Tags are one signal among many: an Arr too old to expose the endpoint, or
    // one that errors on it, must not take the whole sync down with it.
    let tags = match adapter.get_tags().await {
        Ok(tags) => tags,
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the tag catalogue: {e}");
            Vec::new()
        }
    };
    let tag_labels: std::collections::HashMap<i64, String> =
        tags.iter().map(|t| (t.arr_id, t.label.clone())).collect();

    info!(
        instance = %instance.name,
        root_folders = root_folders.len(),
        media = media.len(),
        "Fetched from Arr"
    );

    // Every row written by this run is stamped with the same token, so the
    // orphan cleanup below is exact rather than time-window based — a sync that
    // takes longer than the window would otherwise delete its own early rows.
    let sync_token = crate::services::routing::format_timestamp(chrono::Utc::now());

    // One transaction for the whole instance: a partial sync would make the
    // orphan cleanup delete rows that were simply not written yet.
    let mut tx = state.pool.begin().await?;

    for rf in &root_folders {
        // The same path, declared here first and adopted by the Arr since, is
        // one folder and not two. Promoting it keeps the category mapped onto
        // it; without this the upsert would hit the path index and fail the
        // whole instance's sync.
        sqlx::query(
            "UPDATE root_folders
                SET origin = 'arr', arr_id = ?
              WHERE instance_id = ? AND origin = 'declared'
                AND rtrim(path, '/') = rtrim(?, '/')",
        )
        .bind(rf.arr_id)
        .bind(&instance.id)
        .bind(&rf.path)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            // `last_accessible_at` moves only when the folder answered.
            // `last_synced_at` is stamped on every row this pass writes,
            // including one just reported unreachable, so it says "seen in a
            // pass" — a NAS asleep for three days would read "just now".
            "INSERT INTO root_folders
                (id, instance_id, arr_id, path, free_space, accessible,
                 last_synced_at, last_accessible_at, origin)
             VALUES (?, ?, ?, ?, ?, ?, ?, CASE WHEN ? THEN ? END, 'arr')
             ON CONFLICT(instance_id, arr_id) DO UPDATE SET
                path = excluded.path,
                free_space = excluded.free_space,
                accessible = excluded.accessible,
                last_synced_at = excluded.last_synced_at,
                last_accessible_at =
                    COALESCE(excluded.last_accessible_at, root_folders.last_accessible_at)",
        )
        .bind(format!("rf-{}-{}", instance.id, rf.arr_id))
        .bind(&instance.id)
        .bind(rf.arr_id)
        .bind(&rf.path)
        .bind(rf.free_space)
        .bind(rf.accessible)
        .bind(&sync_token)
        .bind(rf.accessible)
        .bind(&sync_token)
        .execute(&mut *tx)
        .await?;
    }

    inherit_declared(&mut *tx, &instance.id).await?;

    // Replaced wholesale: a tag renamed or deleted upstream must not linger.
    sqlx::query("DELETE FROM arr_tags WHERE instance_id = ?")
        .bind(&instance.id)
        .execute(&mut *tx)
        .await?;
    for tag in &tags {
        sqlx::query("INSERT INTO arr_tags (instance_id, arr_id, label) VALUES (?, ?, ?)")
            .bind(&instance.id)
            .bind(tag.arr_id)
            .bind(&tag.label)
            .execute(&mut *tx)
            .await?;
    }

    for item in &media {
        upsert_media(&mut *tx, &instance.id, item, &tag_labels, &sync_token, &read_at).await?;
    }

    // Drop rows removed upstream so the library view and the counters do not
    // drift. Deleting media cascades to its overrides, so an Arr that answers
    // with an empty list (misconfiguration, restore in progress) must never be
    // taken as "the user deleted everything".
    let (removed, stale_folders) = if media.is_empty() || root_folders.is_empty() {
        tracing::warn!(
            instance = %instance.name,
            "Arr returned an empty media or root-folder list — skipping orphan cleanup"
        );
        (0, 0)
    } else {
        let gone: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM media WHERE instance_id = ? AND last_synced_at IS NOT ?",
        )
        .bind(&instance.id)
        .bind(&sync_token)
        .fetch_all(&mut *tx)
        .await?;
        let removed = retire_media(&mut tx, &gone).await?;

        // Only what the Arr owns. A declared destination is the operator's, and
        // deleting it would take the category mapped onto it with it — on the
        // first pass after it was typed.
        let stale_folders = sqlx::query(
            "DELETE FROM root_folders
             WHERE instance_id = ? AND last_synced_at IS NOT ? AND origin = 'arr'",
        )
        .bind(&instance.id)
        .bind(&sync_token)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        (removed, stale_folders)
    };

    tx.commit().await?;

    if removed > 0 || stale_folders > 0 {
        info!(
            instance = %instance.name,
            "Removed {removed} stale media row(s) and {stale_folders} stale root folder(s)"
        );
    }

    Ok(SyncReport {
        instance_id: instance.id.clone(),
        instance_name: instance.name.clone(),
        root_folders: root_folders.len(),
        media: media.len(),
        removed,
        error: None,
    })
}

/// Sync a single media item after a webhook, without walking the whole library.
pub async fn sync_single_media(
    state: &AppState,
    instance: &Instance,
    arr_id: i64,
) -> AppResult<Option<String>> {
    let adapter: ArrAdapter = state.adapter(instance)?;
    // Before the first request, as `do_sync` does: what the Arr answers is as
    // old as the moment it was asked, and a move applied while the read was
    // in flight has to win over it — see `upsert_media`'s `moved_at` gate.
    let read_at = crate::services::routing::format_timestamp(chrono::Utc::now());
    let Some(item) = adapter.get_media_one(arr_id).await? else {
        return Ok(None);
    };

    // The tag catalogue too: a rule on "tag is anime" must be able to match the
    // moment the item is added, which is the whole point of the webhook path.
    let tag_labels: std::collections::HashMap<i64, String> = adapter
        .get_tags()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| (t.arr_id, t.label))
        .collect();

    upsert_media(&state.pool, &instance.id, &item, &tag_labels, &read_at, &read_at).await?;

    Ok(Some(media_row_id(&instance.id, item.arr_id)))
}

/// The `media.id` of an Arr item: one instance, one Arr id, one row.
pub fn media_row_id(instance_id: &str, arr_id: i64) -> String {
    format!("m-{instance_id}-{arr_id}")
}

/// Remove media rows and retire what pointed at them.
///
/// Overrides go with the row (`ON DELETE CASCADE`); decisions do not reference
/// it, so a pending proposal for an item the Arr no longer has would stay in
/// the list for ever — shown, and never applicable, since the executor joins
/// `media`. Superseded here, which is the state the list already hides. The
/// one writer for both paths that lose a row: the full sync and a delete
/// event.
pub async fn retire_media(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[String],
) -> AppResult<u64> {
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    crate::services::routing::supersede_pending(tx, &refs).await?;

    let mut removed = 0;
    // Chunked like the statement above; a full sync can retire a whole
    // instance's worth of rows in one pass.
    for chunk in ids.chunks(crate::services::routing::BIND_CHUNK) {
        let placeholders = crate::db::placeholders(chunk.len());
        let delete = format!("DELETE FROM media WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(delete.as_str()));
        for id in chunk {
            query = query.bind(id);
        }
        removed += query.execute(&mut **tx).await?.rows_affected();
    }
    Ok(removed)
}

/// Write one media row, creating or updating it.
///
/// Shared by the full sync and the single-item webhook path. Two copies of this
/// column list means adding a column updates one of them, and a rule on an Arr
/// tag then silently cannot match a freshly added item — the exact case
/// automatic application exists for. One writer, one list.
async fn upsert_media<'e, E>(
    executor: E,
    instance_id: &str,
    item: &crate::integrations::adapter::ArrMedia,
    tag_labels: &std::collections::HashMap<i64, String>,
    sync_token: &str,
    // When the caller started reading the Arr. Anything moved *after* this
    // instant is newer than what we are holding, so its path is kept.
    read_at: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, sort_title, year,
         tmdb_id, tvdb_id, imdb_id, current_path, current_root_folder, monitored, has_files,
         status, added_at, series_type, size_on_disk, season_count, tags, genres,
         original_language, certification, last_synced_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(instance_id, arr_id) DO UPDATE SET
            title = excluded.title,
            sort_title = excluded.sort_title,
            year = excluded.year,
            tmdb_id = excluded.tmdb_id,
            tvdb_id = excluded.tvdb_id,
            imdb_id = excluded.imdb_id,
            -- The two columns an apply also writes. Everything else above and
            -- below is upstream's to state, but a path we read before a move
            -- landed is older than the move, and writing it would repropose a
            -- move that already happened.
            current_path = CASE
                WHEN media.moved_at IS NULL OR media.moved_at < ?
                THEN excluded.current_path ELSE media.current_path END,
            current_root_folder = CASE
                WHEN media.moved_at IS NULL OR media.moved_at < ?
                THEN excluded.current_root_folder ELSE media.current_root_folder END,
            monitored = excluded.monitored,
            has_files = excluded.has_files,
            status = excluded.status,
            series_type = excluded.series_type,
            size_on_disk = excluded.size_on_disk,
            season_count = excluded.season_count,
            tags = excluded.tags,
            genres = excluded.genres,
            original_language = excluded.original_language,
            certification = excluded.certification,
            last_synced_at = excluded.last_synced_at",
    )
    .bind(media_row_id(instance_id, item.arr_id))
    .bind(instance_id)
    .bind(item.arr_id)
    .bind(item.media_type)
    .bind(&item.title)
    .bind(&item.sort_title)
    .bind(item.year)
    .bind(item.tmdb_id)
    .bind(item.tvdb_id)
    .bind(&item.imdb_id)
    .bind(&item.path)
    .bind(&item.root_folder_path)
    .bind(item.monitored)
    .bind(item.has_files)
    .bind(&item.status)
    .bind(&item.added)
    .bind(&item.series_type)
    .bind(item.size_on_disk)
    .bind(item.season_count)
    // Labels rather than ids: a rule must be written against "anime", not
    // against tag 7, and the ids are not stable across instances.
    .bind(serde_json::to_string(
        &item.tag_ids.iter().filter_map(|id| tag_labels.get(id).cloned()).collect::<Vec<_>>(),
    )?)
    .bind(serde_json::to_string(&item.genres)?)
    .bind(&item.original_language)
    .bind(&item.certification)
    .bind(sync_token)
    // Twice, because the guard appears once per path column, and sqlx binds by
    // the order the placeholders appear in the statement — the two in the
    // ON CONFLICT clause come after every one in VALUES.
    .bind(read_at)
    .bind(read_at)
    .execute(executor)
    .await?;

    Ok(())
}

/// Record what this pass did.
///
/// The attempt is always stamped; the success only on success. Writing
/// `last_sync_at` in both branches would let a failure refresh it exactly as a
/// success does, and the instance list would read "synchronised 2 minutes ago"
/// beside an error badge, describing data two days old. One column says the
/// scheduler is running, the other says the library is current, and they are
/// not the same question.
async fn update_sync_status(pool: &SqlitePool, instance_id: &str, status: &str) {
    let succeeded = status == "success";
    let _ = sqlx::query(
        "UPDATE instances
         SET last_sync_attempt_at = datetime('now'),
             last_sync_at = CASE WHEN ? THEN datetime('now') ELSE last_sync_at END,
             last_sync_status = ?,
             updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(succeeded)
    .bind(status)
    .bind(instance_id)
    .execute(pool)
    .await;
}

/// A declared destination inherits from the folder it sits under.
///
/// It is the whole point of allowing one: a path beneath a synced root is on
/// that root's volume, so its free space and its reachability are known — which
/// is what keeps the capacity and reachability guards meaningful for a folder
/// no Arr reports. The deepest matching parent wins, since /media/movies/anime
/// belongs to /media/movies rather than to /media.
///
/// Compared by prefix arithmetic rather than with LIKE: a path holding `_` or
/// `%` would otherwise match folders it has nothing to do with.
///
/// Run by the sync and again the moment one is declared — waiting for the next
/// pass would show a new destination with no figures for as long as the
/// instance's sync interval.
pub(crate) async fn inherit_declared<'e, E>(executor: E, instance_id: &str) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "UPDATE root_folders AS d
            SET free_space = (
                    SELECT p.free_space FROM root_folders p
                     WHERE p.instance_id = d.instance_id AND p.origin = 'arr'
                       AND substr(rtrim(d.path, '/'), 1, length(rtrim(p.path, '/')) + 1)
                           = rtrim(p.path, '/') || '/'
                     ORDER BY length(rtrim(p.path, '/')) DESC LIMIT 1),
                accessible = COALESCE((
                    SELECT p.accessible FROM root_folders p
                     WHERE p.instance_id = d.instance_id AND p.origin = 'arr'
                       AND substr(rtrim(d.path, '/'), 1, length(rtrim(p.path, '/')) + 1)
                           = rtrim(p.path, '/') || '/'
                     ORDER BY length(rtrim(p.path, '/')) DESC LIMIT 1), 1),
                last_accessible_at = (
                    SELECT p.last_accessible_at FROM root_folders p
                     WHERE p.instance_id = d.instance_id AND p.origin = 'arr'
                       AND substr(rtrim(d.path, '/'), 1, length(rtrim(p.path, '/')) + 1)
                           = rtrim(p.path, '/') || '/'
                     ORDER BY length(rtrim(p.path, '/')) DESC LIMIT 1)
          WHERE d.instance_id = ? AND d.origin = 'declared'",
    )
    .bind(instance_id)
    .execute(executor)
    .await?;
    Ok(())
}
