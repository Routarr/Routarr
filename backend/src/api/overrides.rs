//! Manual overrides: the human veto over the rule engine.

use super::{Deleted, Json, Query};
use axum::extract::State;

use super::Path;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::media::ExternalTitle;
use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::state::AppState;

type OverrideRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
);

/// A page of the exceptions, the newest first: a script pins a whole library
/// through `PUT /overrides/external`, and one answer holding every pin would
/// grow with it.
pub async fn list(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Query(query): Query<super::PageQuery>,
) -> AppResult<Json<super::Page<OverrideWithMedia>>> {
    let (page, per_page, offset) = super::paginate(query.page, query.per_page);
    let rows: Vec<OverrideRow> = sqlx::query_as(
        "SELECT o.id, o.media_id, o.target_category, o.reason, o.created_at, o.subject,
         m.title, m.media_type, i.name, o.subject_key
         FROM overrides o
         JOIN media m ON o.media_id = m.id
         JOIN instances i ON m.instance_id = i.id
         ORDER BY o.created_at DESC, o.id DESC LIMIT ? OFFSET ?",
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    // Counted as listed: an exception whose title went with a sync is in neither.
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM overrides o
         JOIN media m ON o.media_id = m.id
         JOIN instances i ON m.instance_id = i.id",
    )
    .fetch_one(&state.pool)
    .await?;

    let entries = rows
        .into_iter()
        .map(|r| OverrideWithMedia {
            override_entry: OverrideEntry {
                id: r.0,
                media_id: r.1,
                target_category: r.2,
                reason: r.3,
                created_at: r.4,
                subject: identity.shown_subject(r.5, r.9.as_deref()),
            },
            media_title: r.6,
            media_type: r.7,
            instance_name: r.8,
        })
        .collect();
    Ok(Json(super::Page::new(entries, page, per_page, total)))
}

pub async fn create(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Json(req): Json<CreateOverrideRequest>,
) -> AppResult<super::Created<OverrideEntry>> {
    crate::race::checked("overrides::create", &req.media_id).await;
    let pinned = pin(
        &state,
        std::slice::from_ref(&req.media_id),
        &req.target_category,
        req.reason.as_deref(),
        &identity,
    )
    .await?;
    let pinned = pinned
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Internal("a pin went unwritten".into()))?;
    Ok(super::Created::at(&state, format!("/overrides/{}", pinned.id), pinned))
}

/// What keeping a title where it is takes.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KeepRequest {
    pub reason: Option<String>,
}

/// The category the root folder `folder` leads to on `instance_id`, if one does.
pub(crate) async fn folder_category<'e, E>(
    executor: E,
    instance_id: &str,
    folder: &str,
) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_scalar(
        "SELECT category FROM root_folders
          WHERE instance_id = ? AND path = ? COLLATE path
            AND category IS NOT NULL AND category != ''",
    )
    .bind(instance_id)
    .bind(folder)
    .fetch_optional(executor)
    .await?)
}

/// Pin a title to the category of the root folder it is in, so no run proposes to move it, and
/// decide it again at once.
pub async fn keep(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Path(id): Path<String>,
    Json(req): Json<KeepRequest>,
) -> AppResult<super::Created<OverrideEntry>> {
    let localizer = state.localizer().await;
    let refused = |reason: &'static str, key: &str, params: &[(&str, &str)]| AppError::Conflict {
        reason,
        message: localizer.translate(key, params),
    };
    // Read under the pin's lock: a mapping changed meanwhile would pin a category that moves it.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let held: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT instance_id, current_root_folder FROM media WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((instance_id, folder)) = held else {
        return Err(AppError::NotFound(format!("Media {id} not found")));
    };
    let Some(folder) = folder.filter(|folder| !folder.trim().is_empty()) else {
        return Err(refused("no_current_folder", "ErrorKeepNoFolder", &[]));
    };
    let Some(category) = folder_category(&mut *tx, &instance_id, &folder).await? else {
        return Err(refused("folder_unmapped", "ErrorKeepFolderUnmapped", &[("folder", &folder)]));
    };
    crate::race::checked("overrides::keep", &id).await;
    let by = identity.attribution();
    pin_in(&mut tx, std::slice::from_ref(&id), &category, req.reason.as_deref(), &by, &localizer)
        .await?;
    let row: (String, String, String, Option<String>, String, Option<String>) = sqlx::query_as(
        "SELECT id, media_id, target_category, reason, created_at, subject
           FROM overrides WHERE media_id = ?",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    // The webhook's one-title pass, with no library permit. The pin stands whatever it answers.
    let decided = crate::services::routing::run_simulation(
        &state.pool,
        &state.library_passes,
        crate::services::routing::SimulationOptions {
            trigger: by.trigger.clone(),
            subject: by.subject.clone(),
            subject_key: by.key.clone(),
            instance_ids: vec![instance_id],
            media_ids: Some(vec![id.clone()]),
            persist: true,
            max_returned: Some(1),
            language: state.language().await,
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = decided {
        tracing::warn!(media = %id, "A title kept where it is was not decided again: {e}");
    }

    let entry = OverrideEntry {
        id: row.0,
        media_id: row.1,
        target_category: row.2,
        reason: row.3,
        created_at: row.4,
        subject: row.5,
    };
    Ok(super::Created::at(&state, format!("/overrides/{}", entry.id), entry))
}

/// The category to force on a title, and why.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PinRequest {
    /// The name of an existing category.
    pub target_category: String,
    pub reason: Option<String>,
}

/// Pin every copy of a title another service names, or the one on `instance`.
pub async fn pin_external(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Query(title): Query<ExternalTitle>,
    Json(req): Json<PinRequest>,
) -> AppResult<Json<Vec<OverrideEntry>>> {
    let copies = copies_of(&state, &title).await?;
    let pinned =
        pin(&state, &copies, &req.target_category, req.reason.as_deref(), &identity).await?;
    Ok(Json(pinned))
}

/// Remove the pins on every copy of a title another service names, or on the
/// one on `instance`. A title with none answers `deleted: false`.
pub async fn unpin_external(
    State(state): State<AppState>,
    Query(title): Query<ExternalTitle>,
) -> AppResult<Json<Deleted>> {
    let copies = copies_of(&state, &title).await?;
    let mut tx = state.pool.begin().await?;
    // Only a copy whose pin went loses its proposals: one with none keeps those under review.
    let mut unpinned = Vec::new();
    for media_id in &copies {
        let removed = sqlx::query("DELETE FROM overrides WHERE media_id = ?")
            .bind(media_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if removed > 0 {
            unpinned.push(media_id.as_str());
        }
    }
    crate::services::routing::supersede_pending(&mut tx, &unpinned).await?;
    tx.commit().await?;
    Ok(Json(Deleted { deleted: !unpinned.is_empty() }))
}

/// The library rows of the title `title` names.
async fn copies_of(state: &AppState, title: &ExternalTitle) -> AppResult<Vec<String>> {
    let (media_type, id) = title.named()?;
    let instance = title.instance.as_deref();
    let copies =
        crate::services::routing::media_by_external_id(&state.pool, media_type, &id, instance)
            .await?;
    if copies.is_empty() {
        return Err(AppError::NotFound(format!(
            "No {media_type} in the library has {} {}.",
            id.column(),
            id.value()
        )));
    }
    Ok(copies.into_iter().map(|media| media.id).collect())
}

/// Longest reason a pin keeps. It is a line on the exceptions screen, and
/// the body limit alone would let one hold two megabytes.
const REASON_MAX: usize = 500;

/// Pin each title to `category`, as `asker` asked, and answer the pins as they stand.
async fn pin(
    state: &AppState,
    media_ids: &[String],
    category: &str,
    reason: Option<&str>,
    asker: &crate::api::auth::Identity,
) -> AppResult<Vec<OverrideEntry>> {
    let localizer = state.localizer().await;
    let category = super::categories::normalise(category, &localizer)?;
    // Checked under the pins' write lock, so the category cannot be removed before they land.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    super::categories::ensure_exists(&mut tx, &category, &localizer).await?;
    crate::race::checked("overrides::pin", &category).await;
    pin_in(&mut tx, media_ids, &category, reason, &asker.attribution(), &localizer).await?;

    // Read back before committing: an upsert keeps the stored id, and no later pin can slip in.
    let mut pinned = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let row: (String, String, String, Option<String>, String, Option<String>) = sqlx::query_as(
            "SELECT id, media_id, target_category, reason, created_at, subject
                 FROM overrides WHERE media_id = ?",
        )
        .bind(media_id)
        .fetch_one(&mut *tx)
        .await?;
        pinned.push(OverrideEntry {
            id: row.0,
            media_id: row.1,
            target_category: row.2,
            reason: row.3,
            created_at: row.4,
            subject: row.5,
        });
    }
    tx.commit().await?;
    Ok(pinned)
}

/// Pin each title to `category`, an existing one, as `by`, and withdraw the pending proposals of
/// each whose category changed: a pin repeating a category keeps what it produced. Every writer
/// of a pin goes through here.
pub(crate) async fn pin_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    media_ids: &[String],
    category: &str,
    reason: Option<&str>,
    by: &crate::jobs::Attribution,
    localizer: &crate::localization::Localizer,
) -> AppResult<()> {
    if reason.is_some_and(|reason| reason.chars().count() > REASON_MAX) {
        let refusal =
            localizer.translate("ErrorReasonTooLong", &[("max", &REASON_MAX.to_string())]);
        return Err(AppError::BadRequest(refusal));
    }
    let mut changed = Vec::new();
    for media_id in media_ids {
        // Under the pin's lock: a title a sync removed since it was named has nothing to pin.
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media WHERE id = ?)")
            .bind(media_id)
            .fetch_one(&mut **tx)
            .await?;
        if !exists {
            return Err(AppError::NotFound(format!("Media {media_id} not found")));
        }
        let held: Option<String> =
            sqlx::query_scalar("SELECT target_category FROM overrides WHERE media_id = ?")
                .bind(media_id)
                .fetch_optional(&mut **tx)
                .await?;
        if held.as_deref() != Some(category) {
            changed.push(media_id.as_str());
        }
        sqlx::query(
            "INSERT INTO overrides (id, media_id, target_category, reason, subject, subject_key)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(media_id) DO UPDATE SET
                target_category = excluded.target_category,
                reason = excluded.reason,
                subject = excluded.subject,
                subject_key = excluded.subject_key",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(media_id)
        .bind(category)
        .bind(reason)
        .bind(&by.subject)
        .bind(&by.key)
        .execute(&mut **tx)
        .await?;
    }
    crate::services::routing::supersede_pending(tx, &changed).await
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Deleted>> {
    let media_id: Option<String> =
        sqlx::query_scalar("SELECT media_id FROM overrides WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;

    let Some(media_id) = media_id else {
        return Err(AppError::NotFound(format!("Override {id} not found")));
    };

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM overrides WHERE id = ?").bind(&id).execute(&mut *tx).await?;
    crate::services::routing::supersede_pending(&mut tx, &[&media_id]).await?;
    tx.commit().await?;

    Ok(Json(Deleted { deleted: true }))
}
