//! User-defined functional categories.

use super::Json;
use axum::extract::State;

use super::Path;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::localization::Localizer;
use crate::models::*;
use crate::state::AppState;

/// `is_default` is computed from `AppState::default_category`, the one answer
/// `services::routing` reads: a flag stored on the row, or the setting read
/// here without its fallback, would be a second answer that disagrees with the
/// engine once the setting row is gone.
type CategoryRow = (String, String, Option<String>, bool, i64, String, i64, i64);

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<CategoryWithUsage>>> {
    let fallback = AppState::default_category(&state.pool).await?;
    let rows: Vec<CategoryRow> = sqlx::query_as(
        "SELECT c.id, c.name, c.description, c.name = ?,
                c.display_order, c.created_at,
                (SELECT COUNT(*) FROM rules r WHERE r.target_category = c.name),
                (SELECT COUNT(*) FROM root_folders rf WHERE rf.category = c.name)
         FROM categories c ORDER BY c.display_order, c.name",
    )
    .bind(&fallback)
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

/// A category name is a folder mapping, a badge and a filter value, and a
/// paragraph in any of them breaks the screen that shows it.
const MAX_CATEGORY_NAME_LENGTH: usize = 64;

/// The one place a category name is cleaned and judged.
///
/// Every writer of the `categories` table runs it: `create`, `rename`,
/// `POST /config/import` and `POST /rules/import`. A name that skipped it
/// lands in the table and can never be renamed back, since `rename` runs the
/// gate the writer did not, and it reaches rule payloads and query strings on
/// the way. The refusal is read under the field the name was typed in, so it
/// speaks the interface's language.
pub fn normalise(raw: &str, localizer: &Localizer) -> AppResult<String> {
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Err(AppError::BadRequest(localizer.translate("CategoryNameEmpty", &[])));
    }
    if name.chars().count() > MAX_CATEGORY_NAME_LENGTH {
        let max = MAX_CATEGORY_NAME_LENGTH.to_string();
        return Err(AppError::BadRequest(
            localizer.translate("CategoryNameTooLong", &[("max", &max)]),
        ));
    }
    // Letters of any script, as the reader's language writes them: no space,
    // slash or quote, which a filter value or a rule payload would have to
    // escape.
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || is_mark(c)) {
        return Err(AppError::BadRequest(localizer.translate("CategoryNameRefused", &[])));
    }
    Ok(name)
}

/// A combining mark, which some letters are written with: lower-cased, the
/// Turkish `İ` is `i` followed by U+0307, and a decomposed `é` is `e` and
/// U+0301. Neither is alphanumeric in its own right.
fn is_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateCategoryRequest>,
) -> AppResult<Json<Category>> {
    let name = normalise(&req.name, &state.localizer().await)?;

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
/// The name is the join key, with no foreign key to cascade, so it lives in
/// this row, in every column that names a category, and in the
/// `default_category` setting. Missing that last one leaves an installation
/// holding a setting its own validator rejects the next time anything is saved.
///
/// The decision history follows too. A rename is not a deletion: the category
/// is the same thing under a new name, and leaving old rows pointing at a name
/// that no longer exists would put ghosts in the history screen and break its
/// category filter. Justifications already rendered keep the old word, as they
/// keep the language they were written in, and the next simulation replaces
/// them.
/// Refuse `name` unless a category holds it, read on `connection`: the write
/// transaction of the row about to name it, so no removal lands between the
/// check and the write (`db::write_transaction`).
pub(crate) async fn ensure_exists(
    connection: &mut sqlx::SqliteConnection,
    name: &str,
    localizer: &Localizer,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = ?)")
        .bind(name)
        .fetch_one(connection)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            localizer.translate("ErrorCategoryUnknown", &[("category", name)]),
        ))
    }
}

pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RenameCategoryRequest>,
) -> AppResult<Json<Category>> {
    let name = normalise(&req.name, &state.localizer().await)?;

    // Read in the transaction that writes, so the name renamed is the name
    // stored when the references move, whatever another writer did first.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let fallback = AppState::default_category(&mut *tx).await?;
    let row: Option<CategoryRow> = sqlx::query_as(
        "SELECT id, name, description, name = ?, display_order, created_at, 0, 0
         FROM categories WHERE id = ?",
    )
    .bind(&fallback)
    .bind(&id)
    .fetch_optional(&mut *tx)
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

    for statement in [
        "UPDATE categories SET name = ? WHERE name = ?",
        "UPDATE rules SET target_category = ? WHERE target_category = ?",
        "UPDATE root_folders SET category = ? WHERE category = ?",
        "UPDATE overrides SET target_category = ? WHERE target_category = ?",
        "UPDATE decisions SET target_category = ? WHERE target_category = ?",
        "UPDATE media_routing SET category = ? WHERE category = ?",
        // Pinned expectations too. A case left pointing at the old name fails
        // for a reason that has nothing to do with the rules, and it is the
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
) -> AppResult<Json<super::Deleted>> {
    // Every check and the delete in one transaction holding the write lock: a
    // rule, a mapping, a pin or a case naming the category cannot land between
    // the count that found none and the delete.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM categories WHERE id = ?")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;

    let Some(name) = name else {
        return Err(AppError::NotFound(format!("Category {id} not found")));
    };
    // Read from the setting, not from a flag on the row: the guard has to
    // protect the category the engine actually falls back to.
    if name == AppState::default_category(&mut *tx).await? {
        return Err(AppError::BadRequest("Cannot delete the default category".into()));
    }

    // Categories are referenced by name with no foreign key, so deleting one in
    // use would leave rules pointing at nothing and silently routing to the
    // default. Refuse and tell the user what still depends on it.
    let rules: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rules WHERE target_category = ?")
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
    let folders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM root_folders WHERE category = ?")
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
    let overrides: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM overrides WHERE target_category = ?")
            .bind(&name)
            .fetch_one(&mut *tx)
            .await?;
    // A case expecting the category could only fail once it is gone.
    let tests: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rule_tests WHERE expected_category = ?")
            .bind(&name)
            .fetch_one(&mut *tx)
            .await?;

    if rules + folders + overrides + tests > 0 {
        return Err(AppError::Conflict(format!(
            "Category '{name}' is still in use. Rules: {rules}, root folder mappings: {folders}, overrides: {overrides}, rule tests: {tests}"
        )));
    }

    crate::race::checked("categories::remove", &name).await;
    sqlx::query("DELETE FROM categories WHERE id = ?").bind(&id).execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(super::Deleted { deleted: true }))
}
