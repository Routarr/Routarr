//! Pull media and root folders from the configured Arr instances.

use futures::StreamExt;
use sqlx::{AssertSqlSafe, SqlitePool};
use tracing::{error, info, warn};

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::{Attribution, Detail, JobKind};
use crate::models::Instance;
use crate::models::instance::SYNC_FAILED;
use crate::services::connection::Said;
use crate::services::{notify, webhook_check};
use crate::state::AppState;

/// Result of syncing one instance.
#[derive(Debug, Default, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct SyncReport {
    pub instance_id: String,
    pub instance_name: String,
    pub root_folders: usize,
    pub media: usize,
    /// Media rows that disappeared upstream and were removed locally.
    pub removed: u64,
    /// Set when the sync failed, so the caller of sync-all still gets one entry
    /// per instance instead of the failure silently vanishing from the report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How many instances are synced at once, below the connection pool's eight: each sync ends
/// holding a connection for its write, and a pass taking every one would stall the interface
/// polling `/status`.
pub(crate) const SYNC_CONCURRENCY: usize = 4;

/// Synchronize every enabled instance, isolating per-instance failures.
pub async fn sync_all_instances(state: &AppState, by: &Attribution) -> AppResult<Vec<SyncReport>> {
    let instances = state.instances(true).await?;
    info!("Syncing {} enabled instance(s)", instances.len());

    let localizer = &state.localizer().await;
    let reports = futures::stream::iter(instances.into_iter().map(|instance| async move {
        match sync_instance_inner(state, &instance, by).await {
            Ok(report) => report,
            Err(e) => {
                // One Arr out of reach must not stop the others, nor go unreported.
                error!("Failed to sync instance '{}': {e}", instance.name);
                let said = Said::of(&e, Some(&instance), localizer).said;
                SyncReport {
                    instance_id: instance.id,
                    instance_name: instance.name,
                    error: Some(said),
                    ..Default::default()
                }
            }
        }
    }))
    // `buffered`, not `buffer_unordered`: the interface lists the reports in a stable order.
    .buffered(SYNC_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    Ok(reports)
}

/// Synchronize a single instance by id.
pub async fn sync_instance(
    state: &AppState,
    instance_id: &str,
    by: &Attribution,
) -> AppResult<SyncReport> {
    let instance = state.instance(instance_id).await?;
    sync_instance_inner(state, &instance, by).await
}

async fn sync_instance_inner(
    state: &AppState,
    instance: &Instance,
    by: &Attribution,
) -> AppResult<SyncReport> {
    // The scheduler and a click on Sync must not fight over the same rows.
    let localizer = state.localizer().await;
    let Some(_lock) = state.jobs.try_lock(&format!("sync:{}", instance.id)) else {
        let message = localizer.translate("ErrorSyncInProgress", &[("name", &instance.name)]);
        return Err(AppError::InProgress { reason: "sync_running", message });
    };

    let mut job = state
        .jobs
        .start(
            JobKind::Sync,
            by,
            Some(&instance.id),
            Detail::new("JobDetailSyncing").with("instance", &instance.name),
        )
        .await?;

    // Read before writing: a notification fires on the transition, never on every failing tick.
    let was_failing =
        instance.last_sync_status.as_deref().is_some_and(|s| s.starts_with(SYNC_FAILED));

    let list_folders = lists_folders(state, instance, by).await.unwrap_or(true);
    let outcome = do_sync(state, instance, list_folders, asked(by)).await;
    if outcome.is_ok()
        && let Err(e) = crate::services::executor::settle_requested(state, instance, by).await
    {
        warn!(instance = %instance.name, "The moves left requested could not be settled: {e}");
    }

    // On their own task: awaited here, a slow receiver would hold the sync's answer and lock.
    match &outcome {
        Ok(report) => {
            update_sync_status(&state.pool, &instance.id, "success").await;
            job.report(report);
            job.succeed(
                Detail::new("JobDetailSynced")
                    .with("media", report.media)
                    .with("folders", report.root_folders),
            )
            .await;
            if was_failing {
                notify::send_later(
                    state,
                    notify::Event::InstanceRecovered { instance: instance.name.clone() },
                );
            }
        }
        Err(e) => {
            let said = Said::of(e, Some(instance), &localizer);
            update_sync_status(&state.pool, &instance.id, &format!("{SYNC_FAILED}{}", said.stored))
                .await;
            notify::send_later(
                state,
                notify::Event::SyncFailed {
                    instance_id: instance.id.clone(),
                    instance: instance.name.clone(),
                    error: said.stored.clone(),
                },
            );
            job.fail(e).await;
            // A database refusal or a key that cannot be opened is no Arr out of reach.
            let unreachable = matches!(e, AppError::ExternalApi { .. } | AppError::UpstreamDown(_));
            if !was_failing && unreachable {
                notify::send_later(
                    state,
                    notify::Event::InstanceUnreachable {
                        instance: instance.name.clone(),
                        error: said.stored,
                    },
                );
            }
        }
    }

    outcome
}

/// Whether somebody asked for this sync, through the interface or the API,
/// rather than the schedule, a webhook or the automation.
fn asked(by: &Attribution) -> bool {
    matches!(by.trigger.as_str(), crate::jobs::TRIGGER_MANUAL | crate::jobs::TRIGGER_API)
}

/// How long a scheduled sync goes without listing the root folders.
const FOLDERS_LISTED_EVERY: chrono::Duration = chrono::Duration::hours(24);

/// Whether this sync lists the Arr's root folders, which makes the Arr walk
/// every folder inside each: when somebody asked for it, and otherwise once a
/// day. In between the free space of the mounts is read instead.
async fn lists_folders(state: &AppState, instance: &Instance, by: &Attribution) -> AppResult<bool> {
    if asked(by) {
        return Ok(true);
    }
    let listed: Option<String> =
        sqlx::query_scalar("SELECT root_folders_read_at FROM instances WHERE id = ?")
            .bind(&instance.id)
            .fetch_one(&state.pool)
            .await?;
    let due = chrono::Utc::now() - FOLDERS_LISTED_EVERY;
    Ok(listed
        .and_then(|at| crate::services::routing::parse_timestamp(&at))
        .is_none_or(|at| at < due))
}

/// The root folders as a sync read them: listed whole, or only the free space
/// of the mounts they sit on.
enum Folders {
    Listed(Vec<crate::integrations::adapter::ArrRootFolder>),
    Mounts(Vec<crate::integrations::adapter::ArrDiskSpace>),
}

async fn do_sync(
    state: &AppState,
    instance: &Instance,
    list_folders: bool,
    asked: bool,
) -> AppResult<SyncReport> {
    let adapter = state.adapter(instance)?;

    // Before the first request: `upsert_media` keeps a path moved after this instant.
    let read_at = crate::services::routing::format_timestamp(chrono::Utc::now());

    // The release the warnings hold against the oldest supported, kept when the Arr is silent.
    let version = match adapter.test_connection().await {
        Ok(status) => Some(status.version),
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the Arr's version: {e}");
            None
        }
    };
    let folders = if list_folders {
        Folders::Listed(adapter.get_root_folders().await?)
    } else {
        match adapter.get_disk_space().await {
            Ok(mounts) => Folders::Mounts(mounts),
            Err(e) => {
                tracing::warn!(instance = %instance.name, "Could not read the free space: {e}");
                Folders::Mounts(Vec::new())
            }
        }
    };
    let root_folders = match &folders {
        Folders::Listed(listed) => listed.as_slice(),
        Folders::Mounts(_) => &[],
    };
    let media = adapter.get_media().await?;

    // An Arr that cannot list its tags must not take the whole sync down.
    let fresh_tags = match adapter.get_tags().await {
        Ok(tags) => Some(tags),
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the tag catalogue: {e}");
            None
        }
    };
    let fresh_profiles = match adapter.get_quality_profiles().await {
        Ok(profiles) => Some(profiles),
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the quality profiles: {e}");
            None
        }
    };
    let names = Names {
        tags: match &fresh_tags {
            Some(tags) => tags.iter().map(|t| (t.arr_id, t.label.clone())).collect(),
            None => stored_tag_labels(&state.pool, &instance.id).await?,
        },
        profiles: match &fresh_profiles {
            Some(profiles) => profiles.iter().map(|p| (p.arr_id, p.name.clone())).collect(),
            None => stored_profile_names(&state.pool, &instance.id).await?,
        },
    };
    // The system the Arr's ratings belong to, kept as last read when the Arr cannot say.
    let certification_country = match adapter.certification_country().await {
        Ok(country) => country,
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the rating country: {e}");
            None
        }
    };

    let webhook = match webhook_check::read(state, instance, &adapter, Some(&names.tags)).await {
        Ok(check) => Some(check),
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the webhook connections: {e}");
            None
        }
    };

    info!(
        instance = %instance.name,
        root_folders = root_folders.len(),
        media = media.len(),
        "Fetched from Arr"
    );

    // One token stamps this run's rows: a time window would let a long sync delete its own.
    let sync_token = crate::services::routing::format_timestamp(chrono::Utc::now());

    // One transaction per instance, or the orphan cleanup deletes rows not written yet.
    let mut tx = crate::db::write_transaction(&state.pool).await?;

    // A changed country is dated for the warning, which a sync somebody asks for clears.
    if let Some(country) = &certification_country {
        sqlx::query(
            "UPDATE instances SET
                certification_country_changed_at = CASE
                    WHEN certification_country IS NOT NULL AND certification_country != ?1
                        THEN datetime('now')
                    WHEN ?3 THEN NULL
                    ELSE certification_country_changed_at
                END,
                certification_country = ?1
              WHERE id = ?2",
        )
        .bind(country)
        .bind(&instance.id)
        .bind(asked)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(check) = &webhook {
        webhook_check::store(&mut *tx, instance, check).await?;
    }
    if let Some(version) = &version {
        sqlx::query("UPDATE instances SET arr_version = ? WHERE id = ?")
            .bind(version)
            .bind(&instance.id)
            .execute(&mut *tx)
            .await?;
    }
    match &folders {
        Folders::Listed(_) => {
            sqlx::query("UPDATE instances SET root_folders_read_at = ? WHERE id = ?")
                .bind(&sync_token)
                .bind(&instance.id)
                .execute(&mut *tx)
                .await?;
        }
        Folders::Mounts(mounts) => free_space_of_mounts(&mut tx, &instance.id, mounts).await?,
    }

    // A category follows its path, never an id a rebuilt Arr hands out again.
    let mapped: std::collections::HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT path, category FROM root_folders WHERE instance_id = ? AND category IS NOT NULL",
    )
    .bind(&instance.id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(path, category)| (crate::paths::key(&path), category))
    .collect();

    // Every statement here dropping what the Arr stopped reporting keeps to `origin = 'arr'`.
    for rf in root_folders {
        // Kept, a row holding this id for another path would take this path's place.
        let freed: Option<(String, Option<String>)> = sqlx::query_as(
            "DELETE FROM root_folders
              WHERE instance_id = ? AND arr_id = ? AND origin = 'arr'
                AND path <> ? COLLATE path
             RETURNING path, category",
        )
        .bind(&instance.id)
        .bind(rf.arr_id)
        .bind(&rf.path)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((previous, Some(category))) = freed {
            warn!(
                instance = %instance.name,
                arr_id = rf.arr_id,
                from = %previous,
                to = %rf.path,
                category = %category,
                "The Arr gives this folder id to another path: the category stays with the path \
                 it was set on"
            );
        }

        // A declared path the Arr adopted is promoted, keeping one row and its category.
        sqlx::query(
            "UPDATE root_folders
                SET origin = 'arr', arr_id = ?
              WHERE instance_id = ? AND origin = 'declared'
                AND path = ? COLLATE path",
        )
        .bind(rf.arr_id)
        .bind(&instance.id)
        .bind(&rf.path)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            // Only a folder that answered moves `last_accessible_at`, unlike `last_synced_at`.
            "INSERT INTO root_folders
                (id, instance_id, arr_id, path, free_space, accessible,
                 last_synced_at, last_accessible_at, origin, category)
             VALUES (?, ?, ?, ?, ?, ?, ?, CASE WHEN ? THEN ? END, 'arr', ?)
             ON CONFLICT(instance_id, arr_id) DO UPDATE SET
                path = excluded.path,
                free_space = excluded.free_space,
                accessible = excluded.accessible,
                last_synced_at = excluded.last_synced_at,
                last_accessible_at =
                    COALESCE(excluded.last_accessible_at, root_folders.last_accessible_at),
                category = excluded.category",
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
        .bind(mapped.get(&crate::paths::key(&rf.path)))
        .execute(&mut *tx)
        .await?;
    }

    inherit_declared(&mut tx, &instance.id).await?;

    // Replaced wholesale: a tag renamed or deleted upstream must not linger.
    if let Some(tags) = &fresh_tags {
        sqlx::query("DELETE FROM arr_tags WHERE instance_id = ?")
            .bind(&instance.id)
            .execute(&mut *tx)
            .await?;
        for tag in tags {
            sqlx::query("INSERT INTO arr_tags (instance_id, arr_id, label) VALUES (?, ?, ?)")
                .bind(&instance.id)
                .bind(tag.arr_id)
                .bind(&tag.label)
                .execute(&mut *tx)
                .await?;
        }
    }
    if let Some(profiles) = &fresh_profiles {
        store_profiles(&mut tx, &instance.id, profiles).await?;
    }

    let reassigned = reassigned(&mut tx, &instance.id, &media).await?;
    if !reassigned.is_empty() {
        warn!(
            instance = %instance.name,
            count = reassigned.len(),
            "Arr ids now name other titles: their exceptions and proposals were dropped"
        );
        retire_media(&mut tx, &reassigned).await?;
    }
    for item in &media {
        upsert_media(&mut *tx, &instance.id, item, &names, &sync_token, &read_at).await?;
    }

    // An empty answer is never read as everything deleted: a deletion cascades to overrides.
    let (removed, stale_folders) =
        if media.is_empty() || matches!(&folders, Folders::Listed(listed) if listed.is_empty()) {
            tracing::warn!(
                instance = %instance.name,
                "Arr returned an empty media or root-folder list, skipping orphan cleanup"
            );
            (0, 0)
        } else {
            // Only rows seen before this read: a title the webhook added since is not gone.
            let gone: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM media WHERE instance_id = ? AND last_synced_at IS NOT ?
               AND (last_synced_at IS NULL OR last_synced_at < ?)",
            )
            .bind(&instance.id)
            .bind(&sync_token)
            .bind(&read_at)
            .fetch_all(&mut *tx)
            .await?;
            let removed = retire_media(&mut tx, &gone).await?;

            // Only what the Arr owns: a declared destination is the operator's.
            let stale_folders = match &folders {
                Folders::Listed(_) => sqlx::query(
                    "DELETE FROM root_folders
                 WHERE instance_id = ? AND last_synced_at IS NOT ? AND origin = 'arr'",
                )
                .bind(&instance.id)
                .bind(&sync_token)
                .execute(&mut *tx)
                .await?
                .rows_affected(),
                Folders::Mounts(_) => 0,
            };

            (removed, stale_folders)
        };

    tx.commit().await?;

    if removed > 0 || stale_folders > 0 {
        info!(
            instance = %instance.name,
            "Removed {removed} stale media row(s) and {stale_folders} stale root folder(s)"
        );
    }

    let root_folders: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_folders WHERE instance_id = ? AND origin = 'arr'",
    )
    .bind(&instance.id)
    .fetch_one(&state.pool)
    .await?;

    Ok(SyncReport {
        instance_id: instance.id.clone(),
        instance_name: instance.name.clone(),
        root_folders: usize::try_from(root_folders).unwrap_or_default(),
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
    // Before the first request, as `do_sync` reads it: a move applied meanwhile wins.
    let read_at = crate::services::routing::format_timestamp(chrono::Utc::now());
    let Some(item) = adapter.get_media_one(arr_id).await? else {
        return Ok(None);
    };

    // The tags too: a rule on a tag must match the moment the item is added.
    let tags = match adapter.get_tags().await {
        Ok(tags) => tags.into_iter().map(|t| (t.arr_id, t.label)).collect(),
        Err(e) => {
            tracing::warn!(instance = %instance.name, "Could not read the tag catalogue: {e}");
            stored_tag_labels(&state.pool, &instance.id).await?
        }
    };
    // The stored profiles, read again only for one they do not name, as a new 4K profile.
    let mut profiles = stored_profile_names(&state.pool, &instance.id).await?;
    let mut fresh_profiles = None;
    if item.quality_profile_id.is_some_and(|id| !profiles.contains_key(&id)) {
        match adapter.get_quality_profiles().await {
            Ok(read) => {
                profiles = read.iter().map(|p| (p.arr_id, p.name.clone())).collect();
                fresh_profiles = Some(read);
            }
            Err(e) => {
                tracing::warn!(instance = %instance.name, "Could not read the quality profiles: {e}");
            }
        }
    }
    let names = Names { tags, profiles };

    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let reassigned = reassigned(&mut tx, &instance.id, std::slice::from_ref(&item)).await?;
    retire_media(&mut tx, &reassigned).await?;
    if let Some(read) = &fresh_profiles {
        store_profiles(&mut tx, &instance.id, read).await?;
    }
    upsert_media(&mut *tx, &instance.id, &item, &names, &read_at, &read_at).await?;
    tx.commit().await?;

    Ok(Some(media_row_id(&instance.id, item.arr_id)))
}

/// The `media.id` of an Arr item: one instance, one Arr id, one row.
pub fn media_row_id(instance_id: &str, arr_id: i64) -> String {
    format!("m-{instance_id}-{arr_id}")
}

/// The rows whose Arr id now names another title: a rebuilt Arr hands its ids out again.
/// Told apart by type and by the one external id the Arr holds unique, TMDB for Radarr and
/// TheTVDB for Sonarr: any other id changes when a source corrects it.
async fn reassigned(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
    items: &[crate::integrations::adapter::ArrMedia],
) -> AppResult<Vec<String>> {
    type Known = (i64, String, Option<i64>, Option<i64>);
    // The ids asked about when they fit one statement, the whole instance for a full sync.
    let rows = if items.len() > crate::services::routing::BIND_CHUNK {
        sqlx::query_as::<_, Known>(
            "SELECT arr_id, media_type, tmdb_id, tvdb_id FROM media WHERE instance_id = ?",
        )
        .bind(instance_id)
        .fetch_all(&mut **tx)
        .await?
    } else if items.is_empty() {
        Vec::new()
    } else {
        let sql = known_by_arr_id(items.len());
        let mut query = sqlx::query_as::<_, Known>(AssertSqlSafe(sql.as_str())).bind(instance_id);
        for item in items {
            query = query.bind(item.arr_id);
        }
        query.fetch_all(&mut **tx).await?
    };
    let known: std::collections::HashMap<i64, Known> =
        rows.into_iter().map(|row| (row.0, row)).collect();
    let differs =
        |held: &Option<i64>, now: &Option<i64>| matches!((held, now), (Some(a), Some(b)) if a != b);
    Ok(items
        .iter()
        .filter(|item| {
            known.get(&item.arr_id).is_some_and(|(_, kind, tmdb, tvdb)| {
                kind != item.media_type
                    || match item.media_type {
                        crate::integrations::adapter::MOVIE => differs(tmdb, &item.tmdb_id),
                        _ => differs(tvdb, &item.tvdb_id),
                    }
            })
        })
        .map(|item| media_row_id(instance_id, item.arr_id))
        .collect())
}

/// The rows of `count` Arr ids of one instance, read through their unique
/// index: the webhook checks one title on a library of any size.
fn known_by_arr_id(count: usize) -> String {
    format!(
        "SELECT arr_id, media_type, tmdb_id, tvdb_id FROM media
          WHERE instance_id = ? AND arr_id IN ({})",
        crate::db::placeholders(count)
    )
}

/// Remove media rows and retire what pointed at them: overrides cascade, and a pending
/// proposal, which does not reference the row, is superseded or it stays listed and never
/// applies. The one writer for the full sync and a delete event.
pub async fn retire_media(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[String],
) -> AppResult<u64> {
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    crate::services::routing::supersede_pending(tx, &refs).await?;

    let mut removed = 0;
    // Chunked: a full sync can retire a whole instance's rows in one pass.
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

/// Write one media row, creating or updating it. The one writer of its column list, for the
/// full sync and the webhook alike, so a new column cannot reach only one of them.
async fn upsert_media<'e, E>(
    executor: E,
    instance_id: &str,
    item: &crate::integrations::adapter::ArrMedia,
    names: &Names,
    sync_token: &str,
    // When the caller started reading the Arr: a path moved after it is kept.
    read_at: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO media (id, instance_id, arr_id, media_type, title, sort_title, year,
         tmdb_id, tvdb_id, imdb_id, current_path, current_root_folder, monitored, has_files,
         status, added_at, series_type, size_on_disk, season_count, tags, genres,
         original_language, certification, quality_profile, runtime, studio, network, collection,
         collection_tmdb_id, ratings, last_synced_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                 ?, ?, ?)
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
            quality_profile = excluded.quality_profile,
            runtime = excluded.runtime,
            studio = excluded.studio,
            network = excluded.network,
            collection = excluded.collection,
            collection_tmdb_id = excluded.collection_tmdb_id,
            ratings = excluded.ratings,
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
    .bind(item.added.as_deref().map(stored_shape))
    .bind(&item.series_type)
    .bind(item.size_on_disk)
    .bind(item.season_count)
    // Labels rather than ids: a rule is written against "anime", and ids differ by instance.
    .bind(serde_json::to_string(
        &item.tag_ids.iter().filter_map(|id| names.tags.get(id).cloned()).collect::<Vec<_>>(),
    )?)
    .bind(serde_json::to_string(&item.genres)?)
    .bind(&item.original_language)
    .bind(&item.certification)
    .bind(item.quality_profile_id.and_then(|id| names.profiles.get(&id)))
    .bind(item.runtime)
    .bind(&item.studio)
    .bind(&item.network)
    .bind(item.collection.as_ref().and_then(|c| c.title.as_deref()))
    .bind(item.collection.as_ref().and_then(|c| c.tmdb_id))
    .bind(serde_json::to_string(&item.ratings)?)
    .bind(sync_token)
    // Twice, for the guard of each path column, bound after every VALUES placeholder.
    .bind(read_at)
    .bind(read_at)
    .execute(executor)
    .await?;

    Ok(())
}

/// A date as every stored timestamp is shaped, where the Arr wrote it in its
/// own, and as it came when it cannot be read.
fn stored_shape(raw: &str) -> String {
    crate::services::routing::parse_timestamp(raw)
        .map_or_else(|| raw.to_string(), crate::services::routing::format_timestamp)
}

/// Record what this pass did: the attempt always, the success only on success, so a failure
/// never reads as a library just synchronised.
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

/// What an instance's ids name, which a media row stores by name: ids are not stable across
/// instances, and a rule is written against "anime" or "Ultra-HD".
struct Names {
    tags: std::collections::HashMap<i64, String>,
    profiles: std::collections::HashMap<i64, String>,
}

/// The quality profiles the last good read stored, for when the Arr does not answer.
async fn stored_profile_names(
    pool: &SqlitePool,
    instance_id: &str,
) -> AppResult<std::collections::HashMap<i64, String>> {
    let stored: Vec<(i64, String)> =
        sqlx::query_as("SELECT arr_id, name FROM arr_quality_profiles WHERE instance_id = ?")
            .bind(instance_id)
            .fetch_all(pool)
            .await?;
    Ok(stored.into_iter().collect())
}

/// Replace an instance's stored quality profiles with a fresh read: one renamed or deleted
/// upstream must not linger.
async fn store_profiles(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
    profiles: &[crate::integrations::adapter::ArrQualityProfile],
) -> AppResult<()> {
    sqlx::query("DELETE FROM arr_quality_profiles WHERE instance_id = ?")
        .bind(instance_id)
        .execute(&mut **tx)
        .await?;
    for profile in profiles {
        sqlx::query(
            "INSERT INTO arr_quality_profiles (instance_id, arr_id, name) VALUES (?, ?, ?)",
        )
        .bind(instance_id)
        .bind(profile.arr_id)
        .bind(&profile.name)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// The tag catalogue the last good pass stored, for when the Arr fails to answer its own:
/// resolved against nothing, every `tag_in` rule would stop matching.
pub(crate) async fn stored_tag_labels(
    pool: &SqlitePool,
    instance_id: &str,
) -> AppResult<std::collections::HashMap<i64, String>> {
    let stored: Vec<(i64, String)> =
        sqlx::query_as("SELECT arr_id, label FROM arr_tags WHERE instance_id = ?")
            .bind(instance_id)
            .fetch_all(pool)
            .await?;
    Ok(stored.into_iter().collect())
}

/// Read whether the Arr's root folders answer, and their free space, for the
/// folders already known: what an apply asks about a folder stored as asleep.
/// Adding and removing folders is a full sync's.
pub async fn refresh_root_folders(state: &AppState, instance_id: &str) -> AppResult<()> {
    let instance = state.instance(instance_id).await?;
    let listed = state.adapter(&instance)?.get_root_folders().await?;
    let now = crate::services::routing::format_timestamp(chrono::Utc::now());
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    for folder in &listed {
        sqlx::query(
            "UPDATE root_folders
                SET accessible = ?, free_space = ?,
                    last_accessible_at = CASE WHEN ? THEN ? ELSE last_accessible_at END
              WHERE instance_id = ? AND arr_id = ? AND origin = 'arr'",
        )
        .bind(folder.accessible)
        .bind(folder.free_space)
        .bind(folder.accessible)
        .bind(&now)
        .bind(instance_id)
        .bind(folder.arr_id)
        .execute(&mut *tx)
        .await?;
    }
    inherit_declared(&mut tx, instance_id).await?;
    tx.commit().await?;
    Ok(())
}

/// The free space of each folder the Arr reported, taken from the deepest
/// mount it sits on. A folder on no mount the Arr lists keeps its figure.
async fn free_space_of_mounts(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
    mounts: &[crate::integrations::adapter::ArrDiskSpace],
) -> AppResult<()> {
    let folders: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, path FROM root_folders WHERE instance_id = ? AND origin = 'arr'",
    )
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await?;
    for (id, path) in folders {
        let mount = mounts
            .iter()
            .filter(|mount| crate::paths::within(&path, &mount.path))
            .max_by_key(|mount| crate::paths::key(&mount.path).len());
        if let Some(free) = mount.and_then(|mount| mount.free_space) {
            sqlx::query("UPDATE root_folders SET free_space = ? WHERE id = ?")
                .bind(free)
                .bind(&id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// A declared destination inherits the free space and reachability of the deepest synced
/// folder it sits under, matched by [`crate::paths::within`]: a SQL prefix or `LIKE` would
/// match a sibling. Run by the sync and again the moment one is declared.
pub(crate) async fn inherit_declared(
    connection: &mut sqlx::SqliteConnection,
    instance_id: &str,
) -> AppResult<()> {
    #[derive(sqlx::FromRow)]
    struct Folder {
        id: String,
        path: String,
        origin: String,
        free_space: Option<i64>,
        accessible: bool,
        last_accessible_at: Option<String>,
    }

    let folders: Vec<Folder> = sqlx::query_as(
        "SELECT id, path, origin, free_space, accessible, last_accessible_at
           FROM root_folders WHERE instance_id = ?",
    )
    .bind(instance_id)
    .fetch_all(&mut *connection)
    .await?;
    let (synced, declared): (Vec<Folder>, Vec<Folder>) =
        folders.into_iter().partition(|folder| folder.origin == "arr");

    for folder in &declared {
        let parent = synced
            .iter()
            .filter(|parent| crate::paths::within(&folder.path, &parent.path))
            .max_by_key(|parent| crate::paths::key(&parent.path).len());
        sqlx::query(
            "UPDATE root_folders SET free_space = ?, accessible = ?, last_accessible_at = ?
              WHERE id = ?",
        )
        .bind(parent.and_then(|parent| parent.free_space))
        .bind(parent.is_none_or(|parent| parent.accessible))
        .bind(parent.and_then(|parent| parent.last_accessible_at.clone()))
        .bind(&folder.id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// One title's check reads its own row, not the instance's whole library.
    #[tokio::test]
    async fn one_title_is_checked_through_the_index() {
        let pool = crate::db::test_pool().await;
        let sql = format!("EXPLAIN QUERY PLAN {}", super::known_by_arr_id(1));
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind("inst-1")
            .bind(10)
            .fetch_all(&pool)
            .await
            .unwrap();
        let steps: Vec<&str> = plan.iter().map(|step| step.3.as_str()).collect();
        let by_arr_id = |step: &&str| step.starts_with("SEARCH media") && step.contains("arr_id=?");
        assert!(steps.iter().any(by_arr_id), "{steps:?}");
    }
}
