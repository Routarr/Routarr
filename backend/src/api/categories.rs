//! User-defined functional categories.

use super::Json;
use axum::extract::State;

use super::Path;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::localization::Localizer;
use crate::models::*;
use crate::state::AppState;

/// `is_default` comes from `AppState::default_category`, the answer `services::routing`
/// reads, never from a stored flag.
type CategoryRow = (String, String, Option<String>, bool, i64, String, i64, i64, i64, i64);

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<CategoryWithUsage>>> {
    Ok(Json(with_usage(&state, None).await?))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<CategoryWithUsage>> {
    let found = with_usage(&state, Some(&id)).await?.pop();
    found.map(Json).ok_or_else(|| AppError::NotFound(format!("Category {id} not found")))
}

/// Every category with what holds it, or the one `id` names.
async fn with_usage(state: &AppState, id: Option<&str>) -> AppResult<Vec<CategoryWithUsage>> {
    let fallback = AppState::default_category(&state.pool).await?;
    let rows: Vec<CategoryRow> = sqlx::query_as(
        "SELECT c.id, c.name, c.description, c.name = ?1,
                c.display_order, c.created_at,
                (SELECT COUNT(*) FROM rules r WHERE r.target_category = c.name),
                (SELECT COUNT(*) FROM root_folders rf WHERE rf.category = c.name),
                (SELECT COUNT(*) FROM overrides o WHERE o.target_category = c.name),
                (SELECT COUNT(*) FROM rule_tests t WHERE t.expected_category = c.name)
         FROM categories c
         WHERE ?2 IS NULL OR c.id = ?2
         ORDER BY c.display_order, c.name",
    )
    .bind(&fallback)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
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
            exception_count: r.8,
            rule_test_count: r.9,
        })
        .collect())
}

/// A unique-constraint failure on the name is a conflict, not a server error. The constraint
/// is the only check, since a `SELECT EXISTS` first would race a second request.
fn name_conflict(error: sqlx::Error, name: &str, localizer: &Localizer) -> AppError {
    match &error {
        sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Conflict {
            reason: "name_taken",
            message: localizer.translate("ErrorCategoryNameTaken", &[("name", name)]),
        },
        _ => AppError::from(error),
    }
}

/// A category name is a folder mapping, a badge and a filter value, and a
/// paragraph in any of them breaks the screen that shows it.
const MAX_CATEGORY_NAME_LENGTH: usize = 64;

/// The one place a category name is cleaned and judged, run by every writer of `categories`:
/// `create`, `rename`, `POST /config/import` and `POST /rules/import`. Its refusal is read under
/// the field the name was typed in, so it is translated.
pub fn normalise(raw: &str, localizer: &Localizer) -> AppResult<String> {
    let name = stored_form(raw);
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

/// The one form a name is stored in: trimmed, lower-cased and composed, so an
/// accent written apart from its letter names what the letter with its accent
/// names (Unicode normalisation form C).
pub fn stored_form(raw: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    raw.trim().to_lowercase().nfc().collect()
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
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Json(req): Json<CreateCategoryRequest>,
) -> AppResult<super::Created<Category>> {
    // The fallback is a setting, which only the owner reaches.
    if req.is_default && !identity.is_owner() {
        let refusal = state.localizer().await.translate("ErrorFallbackOwnerOnly", &[]);
        return Err(AppError::Forbidden(refusal));
    }
    let localizer = state.localizer().await;
    let name = normalise(&req.name, &localizer)?;

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
    .map_err(|e| name_conflict(e, &name, &localizer))?;

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

    let location = format!("/categories/{id}");
    let created = Category {
        id,
        name,
        description: req.description,
        is_default: req.is_default,
        display_order: req.display_order,
        created_at: crate::services::routing::format_timestamp(chrono::Utc::now()),
    };
    Ok(super::Created::at(&state, location, created))
}

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

/// Rename a category, and carry every reference to it along: every column naming it, the
/// decisions and the `default_category` setting, since nothing cascades. A rendered
/// justification keeps the old word until the next simulation.
pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RenameCategoryRequest>,
) -> AppResult<Json<Category>> {
    let localizer = state.localizer().await;
    let name = normalise(&req.name, &localizer)?;

    // Read in the transaction that writes, so the name renamed is the name
    // stored when the references move, whatever another writer did first.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let fallback = AppState::default_category(&mut *tx).await?;
    let row: Option<(String, String, Option<String>, bool, i64, String)> = sqlx::query_as(
        "SELECT id, name, description, name = ?, display_order, created_at
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

    move_references(&mut tx, &previous, &name, &localizer).await?;
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

/// Rename a category and every row naming it, in `tx`.
pub(crate) async fn move_references(
    tx: &mut sqlx::SqliteConnection,
    previous: &str,
    name: &str,
    localizer: &Localizer,
) -> AppResult<()> {
    for statement in [
        "UPDATE categories SET name = ? WHERE name = ?",
        "UPDATE rules SET target_category = ? WHERE target_category = ?",
        "UPDATE root_folders SET category = ? WHERE category = ?",
        "UPDATE overrides SET target_category = ? WHERE target_category = ?",
        "UPDATE retired_overrides SET target_category = ? WHERE target_category = ?",
        "UPDATE decisions SET target_category = ? WHERE target_category = ?",
        "UPDATE media_routing SET category = ? WHERE category = ?",
        // A case left on the old name would report a mismatch nobody caused.
        "UPDATE rule_tests SET expected_category = ? WHERE expected_category = ?",
        "UPDATE settings SET value = ?, updated_at = datetime('now')
         WHERE key = 'default_category' AND value = ?",
    ] {
        sqlx::query(statement)
            .bind(name)
            .bind(previous)
            .execute(&mut *tx)
            .await
            .map_err(|e| name_conflict(e, name, localizer))?;
    }
    Ok(())
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<super::Deleted>> {
    // Every check and the delete in one transaction holding the write lock: a
    // rule, a mapping, a pin or a case naming the category cannot land between
    // the count that found none and the delete.
    let localizer = state.localizer().await;
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
        return Err(AppError::Conflict {
            reason: "fallback_category",
            message: localizer.translate("ErrorCategoryIsFallback", &[("name", &name)]),
        });
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
        return Err(AppError::Conflict {
            reason: "in_use",
            message: localizer.translate(
                "ErrorCategoryInUse",
                &[
                    ("name", &name),
                    ("rules", &rules.to_string()),
                    ("folders", &folders.to_string()),
                    ("overrides", &overrides.to_string()),
                    ("tests", &tests.to_string()),
                ],
            ),
        });
    }

    crate::race::checked("categories::remove", &name).await;
    // An exception waiting for its title is no use once its category is gone.
    sqlx::query("DELETE FROM retired_overrides WHERE target_category = ?")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM categories WHERE id = ?").bind(&id).execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(super::Deleted { deleted: true }))
}

/// Give each category name the one form names are stored in, and every row
/// naming it with it. A name whose stored form another category holds is left,
/// and said: merging two categories is the operator's to decide.
pub async fn converge_names(state: &AppState) -> AppResult<()> {
    let localizer = state.localizer().await;
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?;
    for previous in names {
        let name = stored_form(&previous);
        if name == previous {
            continue;
        }
        if names_held(state, &name).await? {
            tracing::warn!(
                "The category '{previous}' reads as '{name}', which another category holds"
            );
            continue;
        }
        let mut tx = crate::db::write_transaction(&state.pool).await?;
        move_references(&mut tx, &previous, &name, &localizer).await?;
        tx.commit().await?;
        tracing::info!("The category '{previous}' is stored as '{name}'");
    }
    Ok(())
}

async fn names_held(state: &AppState, name: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE name = ?)")
        .bind(name)
        .fetch_one(&state.pool)
        .await?)
}
