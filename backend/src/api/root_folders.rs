//! Root folder discovery and category mapping.

use super::Json;
use axum::extract::State;

use super::Path;
use serde::Serialize;
use sqlx::AssertSqlSafe;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::services::folder_suggestions::{self, Reason};
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<RootFolderWithInstance>>> {
    Ok(Json(
        sqlx::query_as(AssertSqlSafe(format!("{WITH_INSTANCE} ORDER BY i.name, rf.path")))
            .fetch_all(&state.pool)
            .await?,
    ))
}

/// Every folder with its instance's name and kind, as the list and a read of one select it.
const WITH_INSTANCE: &str =
    "SELECT rf.id, rf.instance_id, rf.arr_id, rf.path, rf.free_space, rf.accessible,
     rf.category, rf.last_synced_at, rf.last_accessible_at, rf.origin,
     i.name AS instance_name, i.instance_type
     FROM root_folders rf
     JOIN instances i ON rf.instance_id = i.id";

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<RootFolderWithInstance>> {
    let found: Option<RootFolderWithInstance> =
        sqlx::query_as(AssertSqlSafe(format!("{WITH_INSTANCE} WHERE rf.id = ?")))
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    found.map(Json).ok_or_else(|| AppError::NotFound("No such folder".into()))
}

/// A mapping problem the user should resolve.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MappingConflict {
    /// An open list. `unmapped_category`: an enabled rule targets a category no folder holds on
    /// an instance the rule reaches. `unreachable_root_folder`: the Arr did not reach a folder on
    /// its last look, mapped or not. `orphaned_mapping`: a folder is mapped to a category that no
    /// longer exists.
    pub kind: String,
    /// `warning` for every kind today. An open list.
    pub severity: String,
    /// The instance it is about.
    pub instance_name: Option<String>,
    /// The category it is about, when it is about one.
    pub category: Option<String>,
    /// What it means, in the interface language.
    pub message: String,
}

/// Surface missing and unusable mappings, and any disagreement between Routarr's
/// configuration and what the Arrs report.
pub async fn conflicts(State(state): State<AppState>) -> AppResult<Json<Vec<MappingConflict>>> {
    let pool = &state.pool;
    let localizer = state.localizer().await;
    let mut conflicts = Vec::new();

    // Root folders the Arr did not reach on its last look, said with when: a NAS that spins
    // down is unreachable every night, and only the operator knows a nap from a fault.
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
            // Still mapped and routed to: only a write waits while it sleeps.
            severity: "warning".into(),
            instance_name: Some(instance),
            category: None,
            message: match last_seen.as_deref().and_then(crate::services::routing::parse_timestamp)
            {
                Some(then) => localizer.translate(
                    "ConflictUnreachableFor",
                    &[("path", &path), ("age", &crate::localization::human_age(then, &localizer))],
                ),
                None => localizer.translate("ConflictUnreachableEver", &[("path", &path)]),
            },
        });
    }

    // A rule's category no root folder provides, on the instances the rule can reach only: a
    // `movie` rule never routes into a Sonarr, and a warning nobody can clear stops being read.
    let unmapped: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT r.target_category, i.name FROM rules r
         CROSS JOIN instances i
         WHERE r.enabled = 1 AND i.enabled = 1
           AND (r.media_type = 'both'
                OR (r.media_type = 'movie' AND i.instance_type = 'radarr')
                OR (r.media_type = 'series' AND i.instance_type = 'sonarr'))
           AND (r.instance_ids IS NULL
                OR EXISTS (SELECT 1 FROM json_each(r.instance_ids) j WHERE j.value = i.id))
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

/// Declare a destination the Arr does not report, such as a folder beneath one of its roots.
///
/// The path is checked against the Arr's filesystem, which may be another container's. An Arr
/// that cannot be asked does not block the save, and `verified` says so.
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<DeclareRootFolder>,
) -> AppResult<super::Created<Declared>> {
    let localizer = state.localizer().await;
    let path = destination(&req.path, &localizer)?;

    let instance = state.instance(&req.instance_id).await?;

    // Only for a clear answer: the insert below decides in one statement, since no index on the
    // path would catch two racing declarations.
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM root_folders
          WHERE instance_id = ? AND path = ? COLLATE path)",
    )
    .bind(&req.instance_id)
    .bind(&path)
    .fetch_one(&state.pool)
    .await?;
    if taken {
        return Err(AppError::Conflict {
            reason: "already_listed",
            message: localizer.translate("ErrorDestinationAlreadyListed", &[("path", &path)]),
        });
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

    crate::race::checked("root_folders::declare", &req.instance_id).await;
    let mut connection = state.pool.acquire().await?;
    let declared =
        declare_in(&mut connection, &req.instance_id, &path).await.map_err(|e| match e {
            AppError::Database(e) => gone_instance(e, &req.instance_id),
            other => other,
        })?;
    let Some(id) = declared else {
        return Err(AppError::Conflict {
            reason: "already_listed",
            message: localizer.translate("ErrorDestinationAlreadyListed", &[("path", &path)]),
        });
    };

    let declared = Declared { id: id.clone(), path, verified: seen == Some(true) };
    Ok(super::Created::at(&state, format!("/root-folders/{id}"), declared))
}

/// A destination's path as it is stored, or why it is refused: written as the Arr writes it once
/// trimmed, `/data/movies/4k` for an Arr in a Linux container, `D:\Media\4K` or `\\nas\films`
/// for one on Windows.
pub(crate) fn destination(
    raw: &str,
    localizer: &crate::localization::Localizer,
) -> AppResult<String> {
    // A share to Windows and a plain folder to Linux, so read one way or the
    // other it is somewhere the operator did not mean.
    if raw.trim().starts_with("//") {
        return Err(AppError::BadRequest(localizer.translate("ErrorDestinationTwoSlashes", &[])));
    }
    if !crate::paths::is_absolute(raw) {
        return Err(AppError::BadRequest(localizer.translate("ErrorDestinationAbsolute", &[])));
    }
    let path = crate::paths::trimmed(raw);
    // A path climbing out of a folder would take that folder's free space
    // and reachability, and weigh its moves as renames on that volume.
    if !crate::paths::is_plain(&path) {
        return Err(AppError::BadRequest(
            localizer.translate("ErrorDestinationNotPlain", &[("path", &path)]),
        ));
    }
    Ok(path)
}

/// Declare `path` on the instance, and give it the free space and reachability of the Arr's
/// folder holding it now rather than at the next sync. Its id, or `None` when the instance
/// already lists the folder.
pub(crate) async fn declare_in(
    connection: &mut sqlx::SqliteConnection,
    instance_id: &str,
    path: &str,
) -> AppResult<Option<String>> {
    let id = format!("rf-declared-{}", uuid::Uuid::new_v4());
    // `/x` and `/x/` are one folder, as are `D:\X` and `d:/x/`: the guard compares through the
    // path collation, and the statement is its own check.
    let inserted = sqlx::query(
        "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, origin)
         SELECT ?, ?, NULL, ?, 1, 'declared'
          WHERE NOT EXISTS (SELECT 1 FROM root_folders
                             WHERE instance_id = ? AND path = ? COLLATE path)",
    )
    .bind(&id)
    .bind(instance_id)
    .bind(path)
    .bind(instance_id)
    .bind(path)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(None);
    }
    crate::services::sync::inherit_declared(connection, instance_id).await?;
    Ok(Some(id))
}

/// An instance removed since it was read leaves the insert pointing at
/// nothing, which is a missing instance, not a server error.
fn gone_instance(e: sqlx::Error, instance_id: &str) -> AppError {
    if e.as_database_error().is_some_and(|d| d.is_foreign_key_violation()) {
        AppError::NotFound(format!("Instance {instance_id} not found"))
    } else {
        e.into()
    }
}

/// A category suggested for a folder no category leads to yet, read from its name.
#[derive(Debug, Serialize)]
pub struct FolderSuggestion {
    pub root_folder_id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub path: String,
    /// The category to map, `None` when the name says several and the person picks.
    pub category: Option<String>,
    /// Every category the name says, the suggested one alone when it says one.
    pub choices: Vec<String>,
    /// Why, in the interface language.
    pub reason: String,
    /// The template whose family the name says, while the library holds none of its rules.
    pub template: Option<TemplateOffer>,
    pub dismissed: bool,
}

/// A rule template a suggestion offers, by id and by its name in the interface language.
#[derive(Debug, Serialize)]
pub struct TemplateOffer {
    pub id: &'static str,
    pub name: String,
}

/// What a dismissal answers.
#[derive(Debug, Serialize)]
pub struct Dismissal {
    pub dismissed: bool,
}

/// `GET /root-folders/suggestions`: a category for each unmapped folder of an enabled instance
/// whose name says one, computed at each read. A category the instance already leads to through
/// another folder is never suggested, nor one two of its folders would both take.
pub async fn suggestions(State(state): State<AppState>) -> AppResult<Json<Vec<FolderSuggestion>>> {
    let localizer = state.localizer().await;
    let default = AppState::default_category(&state.pool).await?;
    let known: Vec<String> = sqlx::query_scalar("SELECT name FROM categories ORDER BY name")
        .fetch_all(&state.pool)
        .await?;
    let mapped: std::collections::HashSet<(String, String)> =
        sqlx::query_as("SELECT instance_id, category FROM root_folders WHERE category IS NOT NULL")
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect();
    let templated: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT substr(template, 1, instr(template, '@') - 1) FROM rules
          WHERE template IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    let folders: Vec<(String, String, String, String, String, bool)> = sqlx::query_as(
        "SELECT rf.id, rf.instance_id, i.name, i.instance_type, rf.path,
                EXISTS(SELECT 1 FROM folder_suggestion_dismissals d
                        WHERE d.instance_id = rf.instance_id AND d.path = rf.path COLLATE path)
           FROM root_folders rf JOIN instances i ON i.id = rf.instance_id
          WHERE rf.category IS NULL AND i.enabled = 1
          ORDER BY i.name, rf.path",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut found = Vec::new();
    for (root_folder_id, instance_id, instance_name, kind, path, dismissed) in folders {
        let media_type = if kind == "sonarr" { "series" } else { "movie" };
        let Some(mut suggestion) =
            folder_suggestions::suggest(&path, &known, &localizer, &default, media_type)
        else {
            continue;
        };
        let free = |category: &String| !mapped.contains(&(instance_id.clone(), category.clone()));
        if suggestion.category.as_ref().is_some_and(|category| !free(category)) {
            continue;
        }
        suggestion.choices.retain(free);
        if suggestion.choices.is_empty() {
            continue;
        }
        let reason = match &suggestion.reason {
            Reason::Existing => {
                let category = suggestion.choices[0].as_str();
                localizer.translate("FolderSuggestionExisting", &[("category", category)])
            }
            Reason::Named(word) => localizer.translate("FolderSuggestionNamed", &[("value", word)]),
            Reason::Main => localizer.translate("FolderSuggestionMain", &[("category", &default)]),
            Reason::Ambiguous => localizer.translate("FolderSuggestionAmbiguous", &[]),
        };
        let template = suggestion
            .template
            .filter(|id| !templated.iter().any(|t| t == id))
            .and_then(crate::services::templates::find)
            .map(|template| TemplateOffer {
                id: template.id,
                name: localizer.translate(template.name, &[]),
            });
        found.push(FolderSuggestion {
            root_folder_id,
            instance_id,
            instance_name,
            path,
            category: suggestion.category,
            choices: suggestion.choices,
            reason,
            template,
            dismissed,
        });
    }

    // Two folders of one instance saying one category: the person says which it leads to.
    let mut said: std::collections::HashMap<(String, String), usize> = Default::default();
    for each in &found {
        if let Some(category) = &each.category {
            *said.entry((each.instance_id.clone(), category.clone())).or_default() += 1;
        }
    }
    for each in &mut found {
        let key = each.category.clone().map(|category| (each.instance_id.clone(), category));
        if key.is_some_and(|key| said[&key] > 1) {
            let category = each.category.take().unwrap_or_default();
            each.reason = localizer.translate(
                "FolderSuggestionShared",
                &[("instance", &each.instance_name), ("category", &category)],
            );
        }
    }
    Ok(Json(found))
}

/// `PUT /root-folders/{id}/dismissal`: the folder's suggestion stays hidden, through syncs,
/// until shown again.
pub async fn dismiss(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Dismissal>> {
    // Compared through the collation: an Arr adopting the folder respells it with its separator.
    let stored = sqlx::query(
        "INSERT INTO folder_suggestion_dismissals (instance_id, path)
         SELECT rf.instance_id, rf.path FROM root_folders rf
          WHERE rf.id = ?
            AND NOT EXISTS (SELECT 1 FROM folder_suggestion_dismissals d
                             WHERE d.instance_id = rf.instance_id AND d.path = rf.path COLLATE path)",
    )
    .bind(&id)
    .execute(&state.pool)
    .await?;
    if stored.rows_affected() == 0 && !folder_exists(&state, &id).await? {
        return Err(AppError::NotFound("No such folder".into()));
    }
    Ok(Json(Dismissal { dismissed: true }))
}

/// `DELETE /root-folders/{id}/dismissal`: the folder's suggestion shows again. `deleted` says
/// whether it was dismissed.
pub async fn undismiss(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<super::Deleted>> {
    if !folder_exists(&state, &id).await? {
        return Err(AppError::NotFound("No such folder".into()));
    }
    let lifted = sqlx::query(
        "DELETE FROM folder_suggestion_dismissals
          WHERE instance_id = (SELECT instance_id FROM root_folders WHERE id = ?1)
            AND path = (SELECT path FROM root_folders WHERE id = ?1) COLLATE path",
    )
    .bind(&id)
    .execute(&state.pool)
    .await?;
    Ok(Json(super::Deleted { deleted: lifted.rows_affected() > 0 }))
}

async fn folder_exists(state: &AppState, id: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM root_folders WHERE id = ?)")
        .bind(id)
        .fetch_one(&state.pool)
        .await?)
}

/// A destination as declared.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Declared {
    pub id: String,
    /// As stored: closed by the separator the Arr writes.
    pub path: String,
    /// The Arr reported the folder when it was declared. Unverified, it is
    /// tried at the next sync.
    pub verified: bool,
}

/// The category a folder now leads to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Mapped {
    pub updated: bool,
    /// `null` once the folder leads to no category.
    pub category: Option<String>,
}

/// Remove a destination Routarr declared. One an Arr reports is not ours to
/// delete: it would come back on the next sync, without the category.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<super::Deleted>> {
    crate::race::checked("root_folders::delete", &id).await;
    // The statement is its own check: a sync may promote the folder to the
    // Arr's own at any moment, and that one, with its category, stays.
    let deleted: Option<String> = sqlx::query_scalar(
        "DELETE FROM root_folders WHERE id = ? AND origin = 'declared' RETURNING instance_id",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(instance_id) = &deleted {
        folder_suggestions::forget_dismissals_without_folder(&state.pool, instance_id).await?;
    }
    let deleted = u64::from(deleted.is_some());
    if deleted == 0 {
        let still_there: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM root_folders WHERE id = ?)")
                .bind(&id)
                .fetch_one(&state.pool)
                .await?;
        return Err(if still_there {
            AppError::Conflict {
                reason: "not_declared",
                message: state.localizer().await.translate("ErrorDestinationNotOurs", &[]),
            }
        } else {
            AppError::NotFound("No such folder".into())
        });
    }
    Ok(Json(super::Deleted { deleted: true }))
}

pub async fn update_category(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRootFolderCategory>,
) -> AppResult<Json<Mapped>> {
    let category =
        req.category.map(|c| super::categories::stored_form(&c)).filter(|c| !c.is_empty());

    // The checks and the write under one write lock: a category removed, or
    // mapped on another folder of the instance, between a check and the write
    // would otherwise be written over.
    let localizer = state.localizer().await;
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    if let Some(ref cat) = category {
        super::categories::ensure_exists(&mut tx, cat, &localizer).await?;

        // A second folder for a category only when asked: written by mistake, it would share
        // the category's new titles without a word.
        let taken: Option<String> = sqlx::query_scalar(
            "SELECT rf.path FROM root_folders rf
             WHERE rf.category = ?
               AND rf.instance_id = (SELECT instance_id FROM root_folders WHERE id = ?)
               AND rf.id != ?",
        )
        .bind(cat)
        .bind(&id)
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(path) = taken.filter(|_| !req.add_to_set) {
            return Err(AppError::Conflict {
                reason: "category_mapped",
                message: localizer
                    .translate("ErrorCategoryAlreadyMapped", &[("category", cat), ("path", &path)]),
            });
        }
        crate::race::checked("root_folders::map", cat).await;
    }

    let result = sqlx::query("UPDATE root_folders SET category = ? WHERE id = ?")
        .bind(&category)
        .bind(&id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Root folder {id} not found")));
    }
    tx.commit().await?;

    Ok(Json(Mapped { updated: true, category }))
}
