//! Root folder discovery and category mapping.

use super::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::state::AppState;

type RootFolderRow = (
    String,
    String,
    Option<i64>,
    String,
    Option<i64>,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<RootFolderWithInstance>>> {
    let rows: Vec<RootFolderRow> = sqlx::query_as(
        "SELECT rf.id, rf.instance_id, rf.arr_id, rf.path, rf.free_space, rf.accessible,
         rf.category, rf.last_synced_at, rf.last_accessible_at, rf.origin, i.name, i.instance_type
         FROM root_folders rf
         JOIN instances i ON rf.instance_id = i.id
         ORDER BY i.name, rf.path",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| RootFolderWithInstance {
                root_folder: RootFolder {
                    id: r.0,
                    instance_id: r.1,
                    arr_id: r.2,
                    path: r.3,
                    free_space: r.4,
                    accessible: r.5,
                    category: r.6,
                    last_synced_at: r.7,
                    last_accessible_at: r.8,
                    origin: r.9,
                },
                instance_name: r.10,
                instance_type: r.11,
            })
            .collect(),
    ))
}

/// A mapping problem the user should resolve.
#[derive(Debug, Serialize)]
pub struct MappingConflict {
    pub kind: String,
    pub severity: String,
    pub instance_name: Option<String>,
    pub category: Option<String>,
    pub message: String,
}

/// Surface duplicate, missing and unusable mappings, and any disagreement
/// between Routarr's configuration and what the Arrs report.
pub async fn conflicts(State(state): State<AppState>) -> AppResult<Json<Vec<MappingConflict>>> {
    let pool = &state.pool;
    let localizer = state.localizer().await;
    let mut conflicts = Vec::new();

    // Two root folders on the same instance claiming the same category: the
    // routing engine would pick one arbitrarily.
    let duplicates: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT i.name, rf.category, COUNT(*) FROM root_folders rf
         JOIN instances i ON i.id = rf.instance_id
         WHERE rf.category IS NOT NULL AND rf.category != ''
         GROUP BY rf.instance_id, rf.category HAVING COUNT(*) > 1",
    )
    .fetch_all(pool)
    .await?;

    for (instance, category, count) in duplicates {
        conflicts.push(MappingConflict {
            kind: "duplicate_mapping".into(),
            severity: "error".into(),
            instance_name: Some(instance.clone()),
            category: Some(category.clone()),
            message: localizer.translate(
                "ConflictDuplicateMapping",
                &[("count", &count.to_string()), ("instance", &instance), ("category", &category)],
            ),
        });
    }

    // Root folders the Arr did not reach on its last look.
    //
    // Stated as what it is and when, rather than as a mapping conflict: a NAS
    // that spins down is reported inaccessible every night, and an error badge
    // for a disk that is merely asleep is how a diagnostic gets ignored. No
    // threshold decides which is which — "twenty minutes ago" reads as a nap
    // and "three days ago" as a fault, and the operator is the one who knows
    // their hardware.
    let unreachable: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT i.name, rf.path, rf.last_accessible_at FROM root_folders rf
         JOIN instances i ON i.id = rf.instance_id
         WHERE rf.accessible = 0",
    )
    .fetch_all(pool)
    .await?;

    for (instance, path, last_seen) in unreachable {
        conflicts.push(MappingConflict {
            kind: "unreachable_root_folder".into(),
            // A warning, not an error: the folder is still mapped and still
            // routed to, and the only thing that will not happen is a write
            // while it stays quiet.
            severity: "warning".into(),
            instance_name: Some(instance),
            category: None,
            message: match last_seen {
                Some(when) => localizer
                    .translate("ConflictUnreachableSince", &[("path", &path), ("since", &when)]),
                None => localizer.translate("ConflictUnreachableEver", &[("path", &path)]),
            },
        });
    }

    // A rule targets a category that no root folder provides on some instance.
    //
    // Only instances the rule could actually reach: a `movie` rule can never
    // route anything into a Sonarr, so telling the user that Sonarr lacks a
    // folder for `concerts` is a warning they cannot act on. An ordinary setup
    // — movie categories on Radarr, series categories on Sonarr — otherwise
    // fills this page with warnings that never clear, which is exactly how a
    // warning stops being read.
    let unmapped: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT r.target_category, i.name FROM rules r
         CROSS JOIN instances i
         WHERE r.enabled = 1 AND i.enabled = 1
           AND (r.media_type = 'both'
                OR (r.media_type = 'movie' AND i.instance_type = 'radarr')
                OR (r.media_type = 'series' AND i.instance_type = 'sonarr'))
           AND NOT EXISTS (
             SELECT 1 FROM root_folders rf
             WHERE rf.instance_id = i.id AND rf.category = r.target_category
           )",
    )
    .fetch_all(pool)
    .await?;

    for (category, instance) in unmapped {
        conflicts.push(MappingConflict {
            kind: "unmapped_category".into(),
            severity: "warning".into(),
            instance_name: Some(instance.clone()),
            category: Some(category.clone()),
            message: localizer
                .translate("ConflictUnmapped", &[("instance", &instance), ("category", &category)]),
        });
    }

    // A mapping pointing at a category that was deleted since.
    let orphaned: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.name, rf.category FROM root_folders rf
         JOIN instances i ON i.id = rf.instance_id
         WHERE rf.category IS NOT NULL AND rf.category != ''
           AND NOT EXISTS (SELECT 1 FROM categories c WHERE c.name = rf.category)",
    )
    .fetch_all(pool)
    .await?;

    for (instance, category) in orphaned {
        conflicts.push(MappingConflict {
            kind: "orphaned_mapping".into(),
            severity: "warning".into(),
            instance_name: Some(instance),
            category: Some(category.clone()),
            message: localizer.translate("ConflictOrphaned", &[("category", &category)]),
        });
    }

    Ok(Json(conflicts))
}

/// Declare a destination the Arr does not report.
///
/// A target used to have to be a root folder in Radarr or Sonarr already, so
/// routing into `/media/movies/anime` meant declaring it *there* first. What an
/// operator wants is one root folder per Arr and the targets beneath it
/// declared here.
///
/// The path is checked against the **Arr's** filesystem, not against Routarr's:
/// the two run in different containers as often as not, and a path that exists
/// here says nothing about the process that will do the writing. An Arr that
/// cannot be asked does not block the save — refusing on an unavailable probe
/// would lock the operator out of configuring at the worst moment — but the
/// answer is reported.
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<DeclareRootFolder>,
) -> AppResult<Json<serde_json::Value>> {
    let localizer = state.localizer().await;
    let path = req.path.trim().trim_end_matches('/').to_string();
    if path.is_empty() || !path.starts_with('/') {
        return Err(AppError::BadRequest(localizer.translate("ErrorDestinationAbsolute", &[])));
    }

    let instance = state.instance(&req.instance_id).await?;

    // Read here only to answer well; the insert below is what decides, in one
    // statement. Asked and then inserted, two declarations racing each other
    // both passed the question and both landed — and since 019 no index is
    // left to catch the second.
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM root_folders
          WHERE instance_id = ? AND rtrim(path, '/') = ?)",
    )
    .bind(&req.instance_id)
    .bind(&path)
    .fetch_one(&state.pool)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            localizer.translate("ErrorDestinationAlreadyListed", &[("path", &path)]),
        ));
    }

    let seen = match state.adapter(&instance) {
        Ok(adapter) => adapter.directory_exists(&path).await.ok(),
        // Unreachable or misconfigured: the save is not the place to find out.
        Err(_) => None,
    };
    if seen == Some(false) {
        return Err(AppError::BadRequest(
            localizer.translate("ErrorDestinationUnseen", &[("path", &path)]),
        ));
    }

    let id = format!("rf-declared-{}", uuid::Uuid::new_v4());
    // `/x` and `/x/` are one folder, so the guard is on the trimmed path and
    // the statement is its own check: nothing between the two can slip in.
    let inserted = sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, origin)
         SELECT ?, ?, NULL, ?, 1, 'declared'
          WHERE NOT EXISTS (SELECT 1 FROM root_folders
                             WHERE instance_id = ? AND rtrim(path, '/') = ?)",
    )
    .bind(&id)
    .bind(&req.instance_id)
    .bind(&path)
    .bind(&req.instance_id)
    .bind(&path)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(AppError::Conflict(
            localizer.translate("ErrorDestinationAlreadyListed", &[("path", &path)]),
        ));
    }

    // Straight away, not at the next pass: a destination that sits under a
    // synced root already knows its free space and its reachability, and
    // waiting for the sync would show a new row with no figures for as long as
    // that instance's interval.
    crate::services::sync::inherit_declared(&state.pool, &req.instance_id).await?;

    Ok(Json(serde_json::json!({ "id": id, "path": path, "verified": seen == Some(true) })))
}

/// Remove a destination Routarr declared. One an Arr reports is not ours to
/// delete: it would come back on the next sync, without the category.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let origin: Option<String> = sqlx::query_scalar("SELECT origin FROM root_folders WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;

    match origin.as_deref() {
        None => Err(AppError::NotFound("No such folder".into())),
        Some("arr") => Err(AppError::Conflict(
            state.localizer().await.translate("ErrorDestinationNotOurs", &[]),
        )),
        _ => {
            sqlx::query("DELETE FROM root_folders WHERE id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await?;
            Ok(Json(serde_json::json!({ "deleted": id })))
        }
    }
}

pub async fn update_category(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRootFolderCategory>,
) -> AppResult<Json<serde_json::Value>> {
    let category = req.category.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty());

    if let Some(ref cat) = category {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = ?)")
                .bind(cat)
                .fetch_one(&state.pool)
                .await?;

        if !exists {
            return Err(AppError::BadRequest(
                state.localizer().await.translate("ErrorCategoryUnknown", &[("category", cat)]),
            ));
        }

        // Two folders on one instance answering to the same category makes the
        // target ambiguous, so refuse it up front rather than routing at random.
        let taken: Option<String> = sqlx::query_scalar(
            "SELECT rf.path FROM root_folders rf
             WHERE rf.category = ?
               AND rf.instance_id = (SELECT instance_id FROM root_folders WHERE id = ?)
               AND rf.id != ?",
        )
        .bind(cat)
        .bind(&id)
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(path) = taken {
            return Err(AppError::Conflict(
                state
                    .localizer()
                    .await
                    .translate("ErrorCategoryAlreadyMapped", &[("category", cat), ("path", &path)]),
            ));
        }
    }

    let result = sqlx::query("UPDATE root_folders SET category = ? WHERE id = ?")
        .bind(&category)
        .bind(&id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Root folder {id} not found")));
    }

    Ok(Json(serde_json::json!({ "updated": true, "category": category })))
}
