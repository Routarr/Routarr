//! User-defined functional categories.

use super::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::state::AppState;

type CategoryRow = (String, String, Option<String>, bool, i64, String, i64, i64);

/// The category an unmatched media item falls back to.
///
/// One source, and it is the setting, because the setting is what
/// `services::routing` reads. An `is_default` flag on the row as well would be
/// a second answer nothing keeps in step, and the guard below would end up
/// protecting the flagged category rather than the one actually in use.
async fn default_category(pool: &sqlx::SqlitePool) -> Result<String, sqlx::Error> {
    // Delegates rather than repeating the query: a second spelling here that
    // fell back to the empty string where the engine falls back to `standard`
    // would leave the guard below unable to fire with no setting row, and the
    // category routing actually lands in would be deletable.
    Ok(AppState::default_category(pool).await)
}

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<CategoryWithUsage>>> {
    let rows: Vec<CategoryRow> = sqlx::query_as(
        "SELECT c.id, c.name, c.description,
                c.name = (SELECT value FROM settings WHERE key = 'default_category'),
                c.display_order, c.created_at,
                (SELECT COUNT(*) FROM rules r WHERE r.target_category = c.name),
                (SELECT COUNT(*) FROM root_folders rf WHERE rf.category = c.name)
         FROM categories c ORDER BY c.display_order, c.name",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| CategoryWithUsage {
                category: Category {
                    id: r.0,
                    name: r.1,
                    description: r.2,
                    is_default: r.3,
                    display_order: r.4,
                    created_at: r.5,
                },
                rule_count: r.6,
                root_folder_count: r.7,
            })
            .collect(),
    ))
}

/// A unique-constraint failure on the name is a conflict, not a server error.
///
/// A `SELECT EXISTS` before the write would leave a window between the two:
/// nothing stops a second request passing the same check and then losing to the
/// constraint, and losing to the constraint means a raw sqlx error and a 500.
/// The database is the arbiter either way, so it is the only one, and its
/// refusal is translated rather than leaked.
fn name_conflict(error: sqlx::Error, name: &str) -> AppError {
    match &error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("Category '{name}' already exists"))
        }
        _ => AppError::from(error),
    }
}

/// A category name is a folder mapping, a badge and a filter value; a
/// paragraph in any of them breaks the screen that shows it.
const MAX_CATEGORY_NAME_LENGTH: usize = 64;

/// The one place a category name is cleaned and judged.
///
/// Every writer of the `categories` table runs it: `create`, `rename`,
/// `POST /config/import` and `POST /rules/import`. A name that skipped it
/// lands in the table and can never be renamed back, since `rename` runs the
/// gate the writer did not — and it reaches paths, rule payloads and query
/// strings on the way.
pub fn normalise(raw: &str) -> AppResult<String> {
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Err(AppError::BadRequest("Category name cannot be empty".into()));
    }
    if name.chars().count() > MAX_CATEGORY_NAME_LENGTH {
        return Err(AppError::BadRequest(format!(
            "Category names are limited to {MAX_CATEGORY_NAME_LENGTH} characters"
        )));
    }
    // The name ends up in paths, rule payloads and query strings; keep it boring.
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::BadRequest(
            "Category names may only contain letters, digits, '-' and '_'".into(),
        ));
    }
    Ok(name)
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateCategoryRequest>,
) -> AppResult<Json<Category>> {
    let name = normalise(&req.name)?;

    let id = format!("cat-{}", Uuid::new_v4());
    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO categories (id, name, description, display_order) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(&req.description)
    .bind(req.display_order)
    .execute(&mut *tx)
    .await
    .map_err(|e| name_conflict(e, &name))?;

    if req.is_default {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ('default_category', ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Json(Category {
        id,
        name,
        description: req.description,
        is_default: req.is_default,
        display_order: req.display_order,
        created_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
    }))
}

/// Rename a category, and carry every reference to it along.
///
/// The name is the join key — there is no foreign key to cascade — so it lives
/// in six places: this row, the four columns that name a category, and the
/// `default_category` setting. Missing that last one leaves an installation
/// holding a setting its own validator rejects the next time anything is saved.
///
/// The decision history follows too. A rename is not a deletion: the category
/// is the same thing under a new name, and leaving old rows pointing at a name
/// that no longer exists would put ghosts in the history screen and break its
/// category filter. Justifications already rendered keep the old word, as they
/// keep the language they were written in; the next simulation replaces them.
pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RenameCategoryRequest>,
) -> AppResult<Json<Category>> {
    let name = normalise(&req.name)?;

    let row: Option<CategoryRow> = sqlx::query_as(
        "SELECT id, name, description,
                name = (SELECT value FROM settings WHERE key = 'default_category'),
                display_order, created_at, 0, 0
         FROM categories WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;

    let Some(existing) = row else {
        return Err(AppError::NotFound(format!("Category {id} not found")));
    };
    let previous = existing.1;

    if previous == name {
        return Ok(Json(Category {
            id,
            name,
            description: existing.2,
            is_default: existing.3,
            display_order: existing.4,
            created_at: existing.5,
        }));
    }

    let mut tx = state.pool.begin().await?;
    for statement in [
        "UPDATE categories SET name = ? WHERE name = ?",
        "UPDATE rules SET target_category = ? WHERE target_category = ?",
        "UPDATE root_folders SET category = ? WHERE category = ?",
        "UPDATE overrides SET target_category = ? WHERE target_category = ?",
        "UPDATE decisions SET target_category = ? WHERE target_category = ?",
        // Pinned expectations too. A case left pointing at the old name fails
        // for a reason that has nothing to do with the rules — and it is the
        // one place a stale name is *silent*, since the case simply starts
        // reporting a mismatch nobody caused.
        "UPDATE rule_tests SET expected_category = ? WHERE expected_category = ?",
        "UPDATE settings SET value = ?, updated_at = datetime('now')
         WHERE key = 'default_category' AND value = ?",
    ] {
        sqlx::query(statement)
            .bind(&name)
            .bind(&previous)
            .execute(&mut *tx)
            .await
            .map_err(|e| name_conflict(e, &name))?;
    }
    tx.commit().await?;

    Ok(Json(Category {
        id,
        name,
        description: existing.2,
        is_default: existing.3,
        display_order: existing.4,
        created_at: existing.5,
    }))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM categories WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;

    let Some(name) = name else {
        return Err(AppError::NotFound(format!("Category {id} not found")));
    };
    // Read from the setting, not from a flag on the row: the guard has to
    // protect the category the engine actually falls back to.
    if name == default_category(&state.pool).await? {
        return Err(AppError::BadRequest("Cannot delete the default category".into()));
    }

    // Categories are referenced by name with no foreign key, so deleting one in
    // use would leave rules pointing at nothing and silently routing to the
    // default. Refuse and tell the user what still depends on it.
    let rules: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rules WHERE target_category = ?")
        .bind(&name)
        .fetch_one(&state.pool)
        .await?;
    let folders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM root_folders WHERE category = ?")
        .bind(&name)
        .fetch_one(&state.pool)
        .await?;
    let overrides: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM overrides WHERE target_category = ?")
            .bind(&name)
            .fetch_one(&state.pool)
            .await?;

    if rules + folders + overrides > 0 {
        return Err(AppError::Conflict(format!(
            "Category '{name}' is still in use — rules: {rules}, root folder mappings: {folders}, overrides: {overrides}"
        )));
    }

    sqlx::query("DELETE FROM categories WHERE id = ?").bind(&id).execute(&state.pool).await?;

    Ok(Json(serde_json::json!({ "deleted": true })))
}
