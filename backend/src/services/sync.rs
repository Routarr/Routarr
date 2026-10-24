//! Pull media and root folders from the configured Arr instances.

use futures::StreamExt;
use sqlx::{AssertSqlSafe, SqlitePool};
use tracing::{error, info, warn};

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::jobs::{Attribution, Detail, JobKind};
use crate::localization::Told;
use crate::models::Instance;
use crate::models::instance::SYNC_FAILED;
use crate::services::connection::{self, Cause, Said};
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
    /// Set when the sync failed: a sync of every instance answers one entry per instance.
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
        match sync_instance_inner(state, &instance, by, Removal::Bounded, localizer).await {
            Ok(report) => report,
            Err(failed) => {
                error!("Failed to sync instance '{}': {}", instance.name, failed.error);
                SyncReport {
                    instance_id: instance.id,
                    instance_name: instance.name,
                    error: Some(failed.said),
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
    let localizer = state.localizer().await;
    sync_instance_inner(state, &instance, by, Removal::Bounded, &localizer)
        .await
        .map_err(|failed| failed.error)
}

/// [`sync_instance`] for a person waiting on it, a failure answered as what to change.
pub async fn sync_instance_explained(
    state: &AppState,
    instance_id: &str,
    by: &Attribution,
) -> AppResult<SyncReport> {
    let instance = state.instance(instance_id).await?;
    let localizer = state.localizer().await;
    sync_instance_inner(state, &instance, by, Removal::Bounded, &localizer)
        .await
        .map_err(|failed| failed.told())
}

/// Synchronize one instance and remove every title its Arr stopped reporting, however many, a
/// failure answered as what to change.
pub async fn sync_removing_missing(
    state: &AppState,
    instance_id: &str,
    by: &Attribution,
) -> AppResult<SyncReport> {
    let instance = state.instance(instance_id).await?;
    let localizer = state.localizer().await;
    sync_instance_inner(state, &instance, by, Removal::Asked, &localizer)
        .await
        .map_err(|failed| failed.told())
}

/// A sync that failed: the error as it came, which the log keeps, and what a person reads of it.
struct Failed {
    error: AppError,
    said: String,
    /// The refusal a caller reads, when the address, the key or a limit explains the failure.
    refusal: Option<AppError>,
}

impl Failed {
    fn told(self: Box<Self>) -> AppError {
        self.refusal.unwrap_or(self.error)
    }
}

/// What a failed sync says to change, as a row keeps it, and whether a retry may clear it: the
/// address or the key that explains `error`, or the variable to raise when the Arr answers its
/// status but its listing timed out, outgrew its cap or did not decode. `None` when nothing does.
async fn explanation(
    state: &AppState,
    instance: &Instance,
    error: &AppError,
) -> Option<(Told, bool)> {
    let kind = instance.instance_type.as_str();
    let explained = |cause: Cause| {
        let outage = cause.is_outage();
        (connection::told(&cause, kind, &instance.base_url), outage)
    };
    let cause = connection::cause_of(error)?;
    if !matches!(cause, Cause::TimedOut | Cause::TooLarge | Cause::NotTheApi) {
        return Some(explained(cause));
    }
    // A probe of the address tells a wrong type or port from a listing the sync could not read.
    let adapter = state.adapter(instance).ok()?;
    let status = match adapter.test_connection().await {
        Ok(status) => status,
        Err(failed) => return connection::cause_of(&failed).map(explained),
    };
    if let Some(cause) = connection::wrong_app(kind, status.app_name.as_deref()) {
        return Some(explained(cause));
    }
    if let Err(failed) = adapter.get_root_folders().await {
        return connection::cause_of(&failed).map(explained);
    }
    let service = connection::service_name(kind);
    let told = match cause {
        Cause::TimedOut => Told::new("InstanceSyncTimedOut")
            .with("service", service)
            .with("variable", "ROUTARR_LIBRARY_TIMEOUT_SECS"),
        Cause::TooLarge => Told::new("InstanceSyncTooLarge")
            .with("service", service)
            .with("variable", "ROUTARR_MAX_LIBRARY_MIB"),
        _ => Told::new("InstanceSyncUnreadable").with("service", service),
    };
    Some((told, true))
}

/// Whether a sync may remove most of an instance's library on one read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Removal {
    Bounded,
    Asked,
}

/// A sync dropping more titles than this, and more than half of the instance, holds them.
const HELD_PAST: usize = 10;

async fn sync_instance_inner(
    state: &AppState,
    instance: &Instance,
    by: &Attribution,
    removal: Removal,
    localizer: &crate::localization::Localizer,
) -> Result<SyncReport, Box<Failed>> {
    let unexplained = |error: AppError| {
        let said = Said::of(&error, Some(instance), localizer).said;
        Box::new(Failed { error, said, refusal: None })
    };
    // The scheduler and a click on Sync must not fight over the same rows.
    let Some(_lock) = state.jobs.try_lock(&format!("sync:{}", instance.id)) else {
        let message = localizer.translate("ErrorSyncInProgress", &[("name", &instance.name)]);
        return Err(unexplained(AppError::InProgress { reason: "sync_running", message }));
    };

    let mut job = state
        .jobs
        .start(
            JobKind::Sync,
            by,
            Some(&instance.id),
            Detail::new("JobDetailSyncing").with("instance", &instance.name),
        )
        .await
        .map_err(unexplained)?;

    // Read before writing: a notification fires on the transition, never on every failing tick.
    let was_failing =
        instance.last_sync_status.as_deref().is_some_and(|s| s.starts_with(SYNC_FAILED));

    let list_folders = lists_folders(state, instance, by).await.unwrap_or(true);
    let outcome = do_sync(state, instance, list_folders, asked(by), removal).await;
    if outcome.is_ok()
        && let Err(e) = crate::services::executor::settle_requested(state, instance, by).await
    {
        warn!(instance = %instance.name, "The moves left requested could not be settled: {e}");
    }

    let mut failed = None;
    // On their own task: awaited here, a slow receiver would hold the sync's answer and lock.
    match &outcome {
        Ok(report) => {
            update_sync_status(&state.pool, instance, "success").await;
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
            let explained = explanation(state, instance, e).await;
            let said = match &explained {
                Some((told, _)) => Said::told(told, localizer),
                None => Said::of(e, Some(instance), localizer),
            };
            update_sync_status(&state.pool, instance, &format!("{SYNC_FAILED}{}", said.stored))
                .await;
            notify::send_later(
                state,
                notify::Event::SyncFailed {
                    instance_id: instance.id.clone(),
                    instance: instance.name.clone(),
                    error: said.stored.clone(),
                },
            );
            let (task, told) = (job.id.clone(), said.said.clone());
            job.fail(e).await;
            let refusal = explained.map(|(_, outage)| {
                if outage {
                    AppError::UpstreamDown(told.clone())
                } else {
                    AppError::BadRequest(told.clone())
                }
            });
            // The task says what the caller reads, its log line the error as it came.
            if let Some(refusal) = &refusal
                && let Err(e) = sqlx::query(
                    "UPDATE jobs SET error_message = ?, error_code = ?
                      WHERE id = ? AND status = 'failed'",
                )
                .bind(refusal.public_message())
                .bind(refusal.code())
                .bind(&task)
                .execute(&state.pool)
                .await
            {
                let name = &instance.name;
                warn!(instance = %name, "Could not explain the failed sync on its task: {e}");
            }
            failed = Some((told, refusal));
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

    outcome.map_err(|error| match failed {
        Some((said, refusal)) => Box::new(Failed { error, said, refusal }),
        None => unexplained(error),
    })
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
    removal: Removal,
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

    // Stamps what this run writes: a time window would let a long sync delete its own folders.
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
    let known = known_folders(&mut tx, &instance.id).await?;
    for item in &media {
        let item = &item.in_folder_of(&known);
        upsert_media(&mut *tx, &instance.id, item, &names, &read_at).await?;
    }
    restore_overrides(&mut tx, &instance.id).await?;

    // An empty answer is never read as everything deleted: a deletion cascades to overrides.
    let (removed, stale_folders) =
        if media.is_empty() || matches!(&folders, Folders::Listed(listed) if listed.is_empty()) {
            tracing::warn!(
                instance = %instance.name,
                "Arr returned an empty media or root-folder list, skipping orphan cleanup"
            );
            (0, 0)
        } else {
            // Only rows written before this read: a title the webhook added since is not gone.
            let listed: Vec<i64> = media.iter().map(|item| item.arr_id).collect();
            let gone: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM media WHERE instance_id = ?
                   AND arr_id NOT IN (SELECT value FROM json_each(?))
                   AND (last_synced_at IS NULL OR last_synced_at < ?)",
            )
            .bind(&instance.id)
            .bind(serde_json::to_string(&listed)?)
            .bind(&read_at)
            .fetch_all(&mut *tx)
            .await?;
            let held = removal == Removal::Bounded
                && gone.len() > HELD_PAST
                && gone.len() * 2 > gone.len() + media.len();
            let removed = if held {
                warn!(
                    instance = %instance.name,
                    count = gone.len(),
                    "The Arr no longer reports most of the library: its titles are kept until \
                     they are removed from the Instances screen"
                );
                0
            } else {
                retire_media(&mut tx, &gone).await?
            };
            sqlx::query("UPDATE instances SET removal_held = ? WHERE id = ?")
                .bind(if held { gone.len() as i64 } else { 0 })
                .bind(&instance.id)
                .execute(&mut *tx)
                .await?;

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
            if matches!(folders, Folders::Listed(_)) {
                crate::services::folder_suggestions::forget_dismissals_without_folder(
                    &mut *tx,
                    &instance.id,
                )
                .await?;
            }

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
    let item = item.in_folder_of(&known_folders(&mut tx, &instance.id).await?);
    upsert_media(&mut *tx, &instance.id, &item, &names, &read_at).await?;
    restore_overrides(&mut tx, &instance.id).await?;
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

/// Remove media rows and retire what pointed at them, the one writer for the full sync and a
/// delete event. A pending proposal is superseded, an exception waits for the title's own id,
/// and the history moves to an id no row takes: a reused Arr id revives no revert of the old title.
pub async fn retire_media(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[String],
) -> AppResult<u64> {
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    crate::services::routing::supersede_pending(tx, &refs).await?;

    let history = format!("retired-{}:", uuid::Uuid::new_v4());
    let mut removed = 0;
    // Chunked: a full sync can retire a whole instance's rows in one pass.
    for chunk in ids.chunks(crate::services::routing::BIND_CHUNK) {
        let placeholders = crate::db::placeholders(chunk.len());
        let keep = format!(
            "INSERT OR REPLACE INTO retired_overrides
                    (instance_id, media_type, work_id, target_category, reason, created_at,
                     subject, subject_key)
             SELECT m.instance_id, m.media_type,
                    CASE m.media_type WHEN 'movie' THEN m.tmdb_id ELSE m.tvdb_id END,
                    o.target_category, o.reason, o.created_at, o.subject, o.subject_key
               FROM overrides o JOIN media m ON m.id = o.media_id
              WHERE o.media_id IN ({placeholders})
                AND CASE m.media_type WHEN 'movie' THEN m.tmdb_id ELSE m.tvdb_id END > 0"
        );
        let detach = [
            format!(
                "UPDATE decisions SET media_id = ? || media_id
                  WHERE media_id IN ({placeholders}) AND status <> 'pending'"
            ),
            format!(
                "UPDATE execution_logs SET media_id = ? || media_id
                  WHERE media_id IN ({placeholders})"
            ),
        ];
        let delete = format!("DELETE FROM media WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(keep.as_str()));
        for id in chunk {
            query = query.bind(id);
        }
        query.execute(&mut **tx).await?;
        for statement in &detach {
            let mut query = sqlx::query(AssertSqlSafe(statement.as_str())).bind(&history);
            for id in chunk {
                query = query.bind(id);
            }
            query.execute(&mut **tx).await?;
        }
        let mut query = sqlx::query(AssertSqlSafe(delete.as_str()));
        for id in chunk {
            query = query.bind(id);
        }
        removed += query.execute(&mut **tx).await?.rows_affected();
    }
    Ok(removed)
}

/// Give the exceptions back to the titles of `instance_id` reported again by their own id.
async fn restore_overrides(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
) -> AppResult<()> {
    let returned = "FROM retired_overrides r JOIN media m
                      ON m.instance_id = r.instance_id AND m.media_type = r.media_type
                     AND r.work_id = CASE m.media_type WHEN 'movie' THEN m.tmdb_id ELSE m.tvdb_id END
                   WHERE r.instance_id = ?
                     AND NOT EXISTS (SELECT 1 FROM overrides o WHERE o.media_id = m.id)
                     AND EXISTS (SELECT 1 FROM categories c WHERE c.name = r.target_category)";
    sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO overrides (id, media_id, target_category, reason, created_at, subject,
                                subject_key)
         SELECT lower(hex(randomblob(16))), m.id, r.target_category, r.reason, r.created_at,
                r.subject, r.subject_key
         {returned}"
    )))
    .bind(instance_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM retired_overrides
          WHERE instance_id = ? AND EXISTS (
                SELECT 1 FROM media m
                 WHERE m.instance_id = retired_overrides.instance_id
                   AND m.media_type = retired_overrides.media_type
                   AND retired_overrides.work_id =
                       CASE m.media_type WHEN 'movie' THEN m.tmdb_id ELSE m.tvdb_id END)",
    )
    .bind(instance_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Every folder of `instance` Routarr knows, reported or declared.
async fn known_folders(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT path FROM root_folders WHERE instance_id = ?")
        .bind(instance_id)
        .fetch_all(&mut **tx)
        .await?)
}

/// Write one media row, creating it or updating what changed. The one writer of its column list,
/// for the full sync and the webhook alike, so a new column cannot reach only one of them.
async fn upsert_media<'e, E>(
    executor: E,
    instance_id: &str,
    item: &crate::integrations::adapter::ArrMedia,
    names: &Names,
    // When the caller began reading the Arr, stamped on the row: a move or a read after it wins.
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
            -- The two columns an apply also writes: a path read before a move landed is older than
            -- the move, and writing it would propose that move again.
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
            last_synced_at = excluded.last_synced_at
         -- A row read after this read began is newer in every column.
         WHERE (media.last_synced_at IS NULL OR media.last_synced_at <= ?)
           -- An unchanged row is not rewritten: a sync of a quiet library writes nothing.
           AND (((media.moved_at IS NULL OR media.moved_at < ?)
                AND (media.current_path IS NOT excluded.current_path
                     OR media.current_root_folder IS NOT excluded.current_root_folder))
            OR media.title IS NOT excluded.title
            OR media.sort_title IS NOT excluded.sort_title
            OR media.year IS NOT excluded.year
            OR media.tmdb_id IS NOT excluded.tmdb_id
            OR media.tvdb_id IS NOT excluded.tvdb_id
            OR media.imdb_id IS NOT excluded.imdb_id
            OR media.monitored IS NOT excluded.monitored
            OR media.has_files IS NOT excluded.has_files
            OR media.status IS NOT excluded.status
            OR media.series_type IS NOT excluded.series_type
            OR media.size_on_disk IS NOT excluded.size_on_disk
            OR media.season_count IS NOT excluded.season_count
            OR media.tags IS NOT excluded.tags
            OR media.genres IS NOT excluded.genres
            OR media.original_language IS NOT excluded.original_language
            OR media.certification IS NOT excluded.certification
            OR media.quality_profile IS NOT excluded.quality_profile
            OR media.runtime IS NOT excluded.runtime
            OR media.studio IS NOT excluded.studio
            OR media.network IS NOT excluded.network
            OR media.collection IS NOT excluded.collection
            OR media.collection_tmdb_id IS NOT excluded.collection_tmdb_id
            OR media.ratings IS NOT excluded.ratings)",
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
    .bind(read_at)
    // For the guard of each path column, then the two of the `WHERE`, after every VALUES one.
    .bind(read_at)
    .bind(read_at)
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

/// How long a refused outcome waits before it is written again.
const STATUS_RETRY: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(20)
} else {
    std::time::Duration::from_secs(1)
};

/// Record what this pass did: the attempt always, the success only on success, so a failure
/// never reads as a library just synchronised. Tried three times, since a busy database would
/// otherwise leave the Instances screen on the pass before.
async fn update_sync_status(pool: &SqlitePool, instance: &Instance, status: &str) {
    let succeeded = status == "success";
    let mut refused = None;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(STATUS_RETRY).await;
        }
        let written = sqlx::query(
            "UPDATE instances
             SET last_sync_attempt_at = datetime('now'),
                 last_sync_at = CASE WHEN ? THEN datetime('now') ELSE last_sync_at END,
                 last_sync_status = ?,
                 updated_at = datetime('now')
             WHERE id = ?",
        )
        .bind(succeeded)
        .bind(status)
        .bind(&instance.id)
        .execute(pool)
        .await;
        match written {
            Ok(_) => return,
            Err(e) => refused = Some(e),
        }
    }
    if let Some(e) = refused {
        warn!(instance = %instance.name, "The outcome of a sync could not be recorded: {e}");
    }
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

    /// A file Radarr imported while a long sync read the library reaches the row through the
    /// webhook, and the sync's older reading of every column is not laid over it.
    #[tokio::test]
    async fn a_full_sync_keeps_a_row_the_webhook_read_after_it() {
        let pool = crate::db::test_pool().await;
        sqlx::query(crate::tests::AN_INSTANCE).execute(&pool).await.unwrap();
        let film = |has_files: bool| crate::integrations::adapter::ArrMedia {
            arr_id: 10,
            media_type: "movie",
            title: "My Neighbor Totoro".into(),
            sort_title: None,
            year: Some(1988),
            tmdb_id: Some(8392),
            tvdb_id: None,
            imdb_id: None,
            path: Some("/movies/standard/My Neighbor Totoro (1988)".into()),
            root_folder_path: Some("/movies/standard".into()),
            monitored: true,
            has_files,
            status: Some("released".into()),
            added: None,
            series_type: None,
            size_on_disk: Some(if has_files { 8_589_934_592 } else { 0 }),
            season_count: None,
            tag_ids: Vec::new(),
            genres: Vec::new(),
            original_language: None,
            certification: None,
            quality_profile_id: None,
            runtime: None,
            studio: None,
            network: None,
            collection: None,
            ratings: Default::default(),
        };
        let names = super::Names { tags: Default::default(), profiles: Default::default() };
        let (webhook, sync) = ("2026-10-22 12:00:10", "2026-10-22 12:00:00");

        super::upsert_media(&pool, "inst-1", &film(true), &names, webhook).await.unwrap();
        super::upsert_media(&pool, "inst-1", &film(false), &names, sync).await.unwrap();

        let row: (bool, String) = sqlx::query_as("SELECT has_files, last_synced_at FROM media")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row, (true, webhook.to_string()));
    }
}
