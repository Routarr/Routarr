//! Exporting and restoring the configuration.
//!
//! What this is for: rebuilding an installation, or moving it to another
//! machine, without redoing by hand the work that cannot be regenerated —
//! the rules, the folder mappings and above all the manual overrides, which are
//! human decisions no amount of resyncing brings back.
//!
//! What it deliberately is not: a database backup. The library, the decision
//! history and the metadata cache are all reproducible from the Arrs and TMDb,
//! so they are left out; a bundle stays small enough to read and to diff. For a
//! true backup, `data/routarr.db` and `data/routarr.key` must be copied
//! **together** — without the key the encrypted API keys are lost.
//!
//! Nothing host-specific crosses the boundary. Ids are generated per install,
//! and API keys are sealed with a master key that exists on one machine only:
//! both are exported by *meaning* (an instance's name, a root folder's path, a
//! media's external id) or not at all.

use super::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::services::routing;
use crate::state::AppState;

/// The version this build writes and is able to read.
const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigBundle {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
    #[serde(default)]
    pub settings: Vec<Setting>,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub root_folders: Vec<RootFolderMapping>,
    #[serde(default)]
    pub overrides: Vec<Override>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An Arr connection, minus the one thing that cannot travel.
#[derive(Debug, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// Always false today, and present so the restorer knows to ask.
    ///
    /// API keys are sealed with a master key held by one installation. Exporting
    /// them would either leak them in plaintext or produce ciphertext the
    /// destination cannot open — so they are simply absent, and the instance
    /// arrives disabled until someone supplies one.
    #[serde(default)]
    pub has_api_key: bool,
}

/// A folder mapping, keyed by what means the same thing on another machine.
#[derive(Debug, Serialize, Deserialize)]
pub struct RootFolderMapping {
    pub instance_name: String,
    pub path: String,
    pub category: String,
}

/// A human decision about one media, keyed by its external identity.
#[derive(Debug, Serialize, Deserialize)]
pub struct Override {
    /// Kept for the human reading the bundle; matching goes by external id.
    pub media_title: String,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    pub target_category: String,
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub async fn export(State(state): State<AppState>) -> AppResult<Json<ConfigBundle>> {
    let pool = &state.pool;

    // Sealed values are left behind. They are ciphertext under a master key
    // this installation holds alone, so carrying them would put an opaque blob
    // in the destination's table that nothing there can ever open — while
    // `<key>_configured` reported the source as ready. The same reasoning that
    // keeps an Arr's key out of a bundle applies to a metadata source's.
    let settings: Vec<Setting> =
        sqlx::query_as::<_, (String, String)>("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter(|(key, _)| !crate::api::settings::is_secret(key))
            .map(|(key, value)| Setting { key, value })
            .collect();

    let categories: Vec<Category> = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT name, description FROM categories ORDER BY name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(name, description)| Category { name, description })
    .collect();

    let instances: Vec<Instance> = sqlx::query_as::<_, (String, String, String, bool, i64)>(
        "SELECT name, instance_type, base_url, enabled, sync_interval_minutes
           FROM instances ORDER BY name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(name, instance_type, base_url, enabled, sync_interval_minutes)| Instance {
        name,
        instance_type,
        base_url,
        enabled,
        sync_interval_minutes,
        has_api_key: false,
    })
    .collect();

    let root_folders: Vec<RootFolderMapping> = sqlx::query_as::<_, (String, String, String)>(
        "SELECT i.name, rf.path, rf.category
           FROM root_folders rf
           JOIN instances i ON i.id = rf.instance_id
          WHERE rf.category IS NOT NULL AND rf.category != ''
          ORDER BY i.name, rf.path",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(instance_name, path, category)| RootFolderMapping { instance_name, path, category })
    .collect();

    let overrides: Vec<Override> = sqlx::query_as::<
        _,
        (String, String, Option<i64>, Option<i64>, String, bool, Option<String>),
    >(
        "SELECT m.title, m.media_type, m.tmdb_id, m.tvdb_id,
                    o.target_category, o.locked, o.reason
               FROM overrides o
               JOIN media m ON m.id = o.media_id
              ORDER BY m.title",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(media_title, media_type, tmdb_id, tvdb_id, target_category, locked, reason)| Override {
        media_title,
        media_type,
        tmdb_id,
        tvdb_id,
        target_category,
        locked,
        reason,
    })
    .collect();

    Ok(Json(ConfigBundle {
        version: BUNDLE_VERSION,
        exported_at: Some(routing::format_timestamp(chrono::Utc::now())),
        settings,
        categories,
        instances,
        root_folders,
        overrides,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub bundle: ConfigBundle,
}

/// What the import managed to restore, and what it could not.
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub settings: usize,
    pub categories: usize,
    pub instances: usize,
    pub root_folders: usize,
    pub overrides: usize,
    /// Everything that could not be restored, and why. Never silent: a backup
    /// that quietly drops half its contents is worse than none.
    pub skipped: Vec<String>,
    /// The instances restored disabled, by name: no export carries an API key,
    /// so each needs one entered before it can be enabled. Restored, so not in
    /// `skipped`, which the interface reads as what failed.
    pub needs_key: Vec<String>,
}

pub async fn import(
    State(state): State<AppState>,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportReport>> {
    let bundle = req.bundle;
    if bundle.version != BUNDLE_VERSION {
        return Err(AppError::BadRequest(format!(
            "Unsupported bundle version {}. This Routarr understands version {BUNDLE_VERSION}",
            bundle.version
        )));
    }

    let mut report = ImportReport::default();

    // What `default_category` and a folder mapping are allowed to name: the
    // categories already here *plus* the ones this bundle is about to create.
    // Settings are written first because everything after can depend on them,
    // so validating against the table alone would reject every bundle that
    // brings its own categories — which is every bundle from another
    // installation.
    let mut categories: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?;
    // Through the same gate the loop below uses, or a name it is about to
    // refuse would still count as something `default_category` may point at.
    for category in &bundle.categories {
        if let Ok(name) = crate::api::categories::normalise(&category.name)
            && !categories.contains(&name)
        {
            categories.push(name);
        }
    }

    let mut tx = state.pool.begin().await?;

    // Settings first: everything after can depend on them.
    for setting in &bundle.settings {
        // `tmdb_cache_ttl_days` is the former name of `metadata_cache_ttl_days`,
        // from when the cache was TMDb's alone. A bundle exported under that
        // name carries a perfectly valid value; refusing it would lose the one
        // setting the user had bothered to change.
        let key = match setting.key.as_str() {
            "tmdb_cache_ttl_days" => "metadata_cache_ttl_days",
            other => other,
        };

        // Sealed elsewhere, meaningless here. An older bundle can still carry
        // one, since the export only stopped emitting them in this version.
        if crate::api::settings::is_secret(key) {
            report
                .skipped
                .push(format!("setting '{key}' is sealed by another installation. Set it again"));
            continue;
        }

        // The same gate `PUT /settings` applies. A key this build does not know
        // would sit in the table for ever, unreachable and unremovable through
        // the API; a value it does know but refuses is worse, because it looks
        // applied. Reported rather than fatal: a bundle is restored as far as it
        // can be, and `skipped` is what says how far.
        if let Err(e) = crate::api::settings::check(key, &setting.value, &categories) {
            report.skipped.push(format!("setting {:?}: {e}", setting.key));
            continue;
        }

        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        // Trimmed, as `PUT /settings` stores it. The gate above validates the
        // trimmed value, so binding the raw one let `"anime "` pass a check
        // that `"anime"` had answered and land as a name no category holds.
        .bind(setting.value.trim())
        .execute(&mut *tx)
        .await?;
        report.settings += 1;
    }

    for category in &bundle.categories {
        // The same gate `POST /categories` applies. Trimming and lowercasing
        // was only half of it: a name with a space or an ampersand, or one five
        // hundred characters long, was written and then unreachable — `rename`
        // runs this check, so it could never be corrected through the API.
        let name = match crate::api::categories::normalise(&category.name) {
            Ok(name) => name,
            Err(e) => {
                report.skipped.push(format!("category {:?}: {e}", category.name));
                continue;
            }
        };
        sqlx::query(
            "INSERT INTO categories (id, name, description) VALUES (?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET description = excluded.description",
        )
        .bind(format!("cat-{}", Uuid::new_v4()))
        .bind(&name)
        .bind(&category.description)
        .execute(&mut *tx)
        .await?;
        report.categories += 1;
    }

    // Instances arrive without their API key, so they arrive disabled: an
    // instance that looks connected but cannot authenticate would fail on every
    // scheduler tick and fill the log with noise the user did not ask for.
    for instance in &bundle.instances {
        // The same three checks `POST /instances` applies. Written into the
        // table unchecked, a type no adapter knows or a URL with no scheme
        // produces a row the create endpoint would have refused — and one the
        // edit screen cannot save without fixing first.
        if instance.instance_type.parse::<crate::models::InstanceType>().is_err() {
            report.skipped.push(format!(
                "instance '{}': '{}' is not a known instance type",
                instance.name, instance.instance_type
            ));
            continue;
        }
        let base_url = instance.base_url.trim().trim_end_matches('/');
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            report.skipped.push(format!(
                "instance '{}': base_url must start with http:// or https://",
                instance.name
            ));
            continue;
        }
        if instance.name.trim().is_empty() {
            report.skipped.push("an instance with no name was left out".to_string());
            continue;
        }

        // Stored as `POST /instances` stores it: the name trimmed, the interval
        // within the day the scheduler clamps it to when it reads it — so the
        // number on screen is the one running. Matched trimmed on both sides,
        // or a stray space — in the bundle, or in a row an earlier build wrote
        // as it came — is a second instance the check cannot see.
        let name = instance.name.trim();
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM instances WHERE TRIM(name) = ?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;

        if existing.is_some() {
            report
                .skipped
                .push(format!("instance '{}' already exists and was left alone", instance.name));
            continue;
        }

        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled,
             sync_interval_minutes, webhook_token)
             VALUES (?, ?, ?, ?, '', 0, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(name)
        .bind(instance.instance_type.to_lowercase())
        .bind(base_url)
        .bind(instance.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *tx)
        .await?;
        report.instances += 1;
        report.needs_key.push(name.to_string());
    }

    // Mappings are matched on (instance name, path). Both are meaningful on the
    // destination; ids are not.
    for mapping in &bundle.root_folders {
        // Lowercased like the category rows themselves, which the loop above
        // writes that way: a bundle carrying `Anime` would otherwise create
        // `anime` and point the folder at a name no row holds.
        let category = mapping.category.trim().to_lowercase();

        // Categories are joined by value with no foreign key, so nothing in the
        // database would stop a mapping naming one that does not exist — it
        // would simply route nowhere, and the unmapped-category warning could
        // not fire either, since there is no category row to count.
        if !categories.contains(&category) {
            report.skipped.push(format!(
                "mapping '{}' → '{}' on '{}': that category does not exist",
                mapping.path, mapping.category, mapping.instance_name
            ));
            continue;
        }

        let affected = sqlx::query(
            "UPDATE root_folders SET category = ?
              WHERE path = ?
                AND instance_id = (SELECT id FROM instances WHERE name = ?)",
        )
        .bind(&category)
        .bind(&mapping.path)
        .bind(&mapping.instance_name)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if affected == 0 {
            // Expected on a fresh install: root folders only exist once the
            // instance has been synced.
            report.skipped.push(format!(
                "mapping '{}' → '{}' on '{}': sync that instance, then import again",
                mapping.path, mapping.category, mapping.instance_name
            ));
        } else {
            report.root_folders += 1;
        }
    }

    // Overrides are matched on external id, which is the only identity a media
    // keeps across installations.
    for over in &bundle.overrides {
        let media_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM media
              WHERE media_type = ?
                AND ((tmdb_id IS NOT NULL AND tmdb_id = ?) OR (tvdb_id IS NOT NULL AND tvdb_id = ?))
              LIMIT 1",
        )
        .bind(&over.media_type)
        .bind(over.tmdb_id)
        .bind(over.tvdb_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(media_id) = media_id else {
            report.skipped.push(format!(
                "override for '{}' → '{}': that media is not in the library yet",
                over.media_title, over.target_category
            ));
            continue;
        };

        // The same two checks `POST /overrides` applies. An override
        // short-circuits the engine entirely, so one naming a category this
        // installation does not have routes its item nowhere — silently, and
        // without appearing in `skipped`, which is the one place a partial
        // restore is supposed to be visible.
        let category = over.target_category.trim().to_lowercase();
        if !categories.contains(&category) {
            report.skipped.push(format!(
                "override for '{}' → '{}': that category does not exist",
                over.media_title, over.target_category
            ));
            continue;
        }

        sqlx::query(
            "INSERT INTO overrides (id, media_id, target_category, reason, locked)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(media_id) DO UPDATE SET
                target_category = excluded.target_category,
                reason = excluded.reason,
                locked = excluded.locked",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&media_id)
        .bind(&category)
        .bind(&over.reason)
        .bind(over.locked)
        .execute(&mut *tx)
        .await?;
        report.overrides += 1;
    }

    tx.commit().await?;
    Ok(Json(report))
}
