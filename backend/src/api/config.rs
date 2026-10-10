//! Exporting and restoring the configuration.
//!
//! What this is for: rebuilding an installation, or moving it to another
//! machine, without redoing by hand the work that cannot be regenerated: the
//! rules, the folder mappings and above all the manual overrides, which are
//! human decisions no amount of resyncing brings back.
//!
//! What it deliberately is not: a database backup. The library, the decision
//! history and the metadata cache are all reproducible from the Arrs and the
//! metadata sources, so they are left out, and a bundle stays small enough to
//! read and to diff. A backup is `services::backup`, which archives the
//! database with the master key and the API key.
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
use crate::state::AppState;

/// The version this build writes and is able to read.
const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub rules: Vec<crate::models::BundledRule>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An Arr connection, minus the one thing that cannot travel.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// Always false, and present so the restorer knows to ask.
    ///
    /// API keys are sealed with a master key held by one installation. Exporting
    /// them would either leak them in plaintext or produce ciphertext the
    /// destination cannot open, so they are simply absent, and the instance
    /// arrives disabled until someone supplies one.
    #[serde(default)]
    pub has_api_key: bool,
}

/// A folder mapping, keyed by what means the same thing on another machine.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootFolderMapping {
    pub instance_name: String,
    pub path: String,
    pub category: String,
}

/// A human decision about one media, keyed by its external identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// Kept for the human reading the bundle: matching goes by external id.
    pub media_title: String,
    /// The instance holding the copy the exception was pinned on: one film
    /// held by two instances is two titles, each with its own exception.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    pub target_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // A bundle may hold a lock on an exception. It is read and dropped, since
    // an exception pins its title whatever it holds: refused, the whole bundle
    // would not import.
    #[serde(rename = "locked", default, skip_serializing)]
    pub _locked: Option<serde::de::IgnoredAny>,
}

pub async fn export(State(state): State<AppState>) -> AppResult<Json<ConfigBundle>> {
    let pool = &state.pool;

    // Sealed values are left behind. They are ciphertext under a master key
    // this installation holds alone, so carrying them would put an opaque blob
    // in the destination's table that nothing there can ever open, while
    // `<key>_configured` would report the source as ready. The same reasoning
    // that keeps an Arr's key out of a bundle applies to a metadata source's.
    let settings: Vec<Setting> =
        sqlx::query_as::<_, (String, String)>("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(pool)
            .await?
            .into_iter()
            .filter(|(key, _)| !crate::services::settings::is_secret(key))
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
        base_url: crate::http::without_credentials(&base_url).into_owned(),
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

    type OverrideRow = (String, String, String, Option<i64>, Option<i64>, String, Option<String>);
    let overrides: Vec<Override> = sqlx::query_as::<_, OverrideRow>(
        "SELECT m.title, i.name, m.media_type, m.tmdb_id, m.tvdb_id, o.target_category, o.reason
           FROM overrides o
           JOIN media m ON m.id = o.media_id
           JOIN instances i ON i.id = m.instance_id
          ORDER BY m.title, i.name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(media_title, instance, media_type, tmdb_id, tvdb_id, target_category, reason)| {
        Override {
            media_title,
            instance_name: Some(instance),
            media_type,
            tmdb_id,
            tvdb_id,
            target_category,
            reason,
            _locked: None,
        }
    })
    .collect();

    let rules = super::rules::bundled_rules(pool).await?;

    Ok(Json(ConfigBundle {
        version: BUNDLE_VERSION,
        exported_at: Some(super::rules::exported_now()),
        settings,
        categories,
        instances,
        root_folders,
        overrides,
        rules,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub bundle: ConfigBundle,
    /// Whether the bundle's rules replace the rules in place, rather than
    /// joining them, as `POST /rules/import` asks.
    #[serde(default)]
    pub replace_rules: bool,
}

/// What the import managed to restore, and what it could not.
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub settings: usize,
    pub categories: usize,
    pub instances: usize,
    pub root_folders: usize,
    pub overrides: usize,
    pub rules: usize,
    /// Everything that could not be restored, and why. Never silent: a backup
    /// that quietly drops half its contents is worse than none.
    pub skipped: Vec<String>,
    /// The instances restored disabled, by name: no export carries an API key,
    /// so each needs one entered before it can be enabled. Restored, so not in
    /// `skipped`, which the interface reads as what failed.
    pub needs_key: Vec<String>,
    /// Each rule restored otherwise than the bundle has it, with how.
    pub adjusted: Vec<String>,
}

pub async fn import(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    crate::api::auth::Client(client): crate::api::auth::Client,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportReport>> {
    let bundled = crate::api::auth::allowed(
        crate::services::audit::Kind::Configuration,
        "AuditConfigurationImported",
    )
    .with("settings", req.bundle.settings.len())
    .with("instances", req.bundle.instances.len())
    .with("rules", req.bundle.rules.len());
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
    // brings its own categories, which is every bundle from another
    // installation.
    let localizer = state.localizer().await;
    let mut categories = with_bundled(
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?,
        &bundle,
        &localizer,
    );

    // A source the bundle enables that needs a key this installation lacks is
    // left out of the list, and said, as `PUT /settings` refuses it. Keys never
    // travel, so refusing the whole list would lose it on every restore. Read
    // before the transaction opens, for the reason the rules below are judged
    // there.
    let mut keyless = Vec::new();
    for setting in bundle.settings.iter().filter(|s| s.key == "metadata_providers") {
        let empty = std::collections::HashMap::new();
        keyless.extend(
            super::settings::sources_without_their_key(&state, &setting.value, &empty).await,
        );
    }

    // Judged before the transaction opens, against the categories as they will
    // be once it commits: a pool of one connection, which the tests run on,
    // cannot serve the reads judging needs while a transaction holds it.
    let mut importable = Vec::new();
    if !bundle.rules.is_empty() {
        let mut env = super::rules::environment(&state).await?;
        env.known.extend(categories.iter().cloned());
        for bundled in &bundle.rules {
            let errors: Vec<String> = super::rules::judge(&env, &bundled.rule)
                .into_iter()
                .filter(crate::models::ValidationIssue::is_error)
                .map(|issue| issue.message)
                .collect();
            if errors.is_empty() {
                importable.push(bundled);
            } else {
                report.skipped.push(format!(
                    "rule '{}': {}",
                    bundled.rule.name,
                    errors.join(" · ")
                ));
            }
        }
    }

    let mut tx = crate::db::write_transaction(&state.pool).await?;
    // Read again under the write lock: a category removed since the read
    // above would otherwise be named by a setting, a mapping, a pin or a rule
    // written below.
    categories = with_bundled(
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&mut *tx).await?,
        &bundle,
        &localizer,
    );

    // Settings first: everything after can depend on them.
    for setting in &bundle.settings {
        let key = setting.key.as_str();

        // A credential never arrives through a bundle: sealed elsewhere it
        // opens with nothing here, and in the clear it came through a file
        // people share. The export writes none, but a bundle edited by hand, or
        // exported by a build that kept the setting in the clear, can carry one.
        if crate::services::settings::is_secret(key) {
            report.skipped.push(format!(
                "setting '{key}' is a credential a bundle does not carry. Set it again"
            ));
            continue;
        }

        // The same gate `PUT /settings` applies. A key this build does not know
        // would sit in the table for ever, unreachable and unremovable through
        // the API, and a value it does know but refuses is worse, because it
        // looks applied. Reported rather than fatal: a bundle is restored as far as it
        // can be, and `skipped` is what says how far.
        let checked = crate::api::settings::stored_value(key, &setting.value);
        if let Err(e) = crate::services::settings::check(key, &checked, &categories, &localizer) {
            report.skipped.push(format!("setting {:?}: {e}", setting.key));
            continue;
        }
        let mut value = checked;
        if key == "metadata_providers" && !keyless.is_empty() {
            for id in &keyless {
                report.skipped.push(format!(
                    "source '{id}' has no key here: set its key, then enable the source"
                ));
            }
            value = value
                .split(',')
                .map(str::trim)
                .filter(|id| !keyless.iter().any(|keyless| keyless == id))
                .collect::<Vec<_>>()
                .join(",");
        }

        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        // In the form `PUT /settings` stores it. The gate above validates that
        // form, so binding the raw one would let `"Anime "` pass a check that
        // `"anime"` answered and land as a name no category holds.
        .bind(&value)
        .execute(&mut *tx)
        .await?;
        report.settings += 1;
    }

    for category in &bundle.categories {
        // The same gate `POST /categories` applies. Trimming and lowercasing
        // is only half of it: a name with a space or an ampersand, or one five
        // hundred characters long, would be written and then unreachable, since
        // `rename` runs this check and it could never be corrected through the
        // API.
        let name = match crate::api::categories::normalise(&category.name, &localizer) {
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
        // produces a row the create endpoint would have refused, and one the
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
        // within the day the scheduler clamps it to when it reads it, so the
        // number on screen is the one running. Matched trimmed on both sides,
        // or a stray space (in the bundle, or in a row stored untrimmed) is a
        // second instance the check cannot see.
        let name = instance.name.trim();
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM instances WHERE lower(trim(name)) = lower(?)")
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
    // destination, and ids are not.
    for mapping in &bundle.root_folders {
        // Lowercased like the category rows themselves, which the loop above
        // writes that way: a bundle carrying `Anime` would otherwise create
        // `anime` and point the folder at a name no row holds.
        let category = mapping.category.trim().to_lowercase();

        // Categories are joined by value with no foreign key, so nothing in the
        // database would stop a mapping naming one that does not exist. It
        // would simply route nowhere, and the unmapped-category warning could
        // not fire either, since there is no category row to count.
        if !categories.contains(&category) {
            report.skipped.push(format!(
                "mapping '{}' → '{}' on '{}': that category does not exist",
                mapping.path, mapping.category, mapping.instance_name
            ));
            continue;
        }

        // One category, one folder per instance, as the mapping route insists:
        // two folders answering to one category leave the target ambiguous.
        let taken: Option<String> = sqlx::query_scalar(
            "SELECT path FROM root_folders
              WHERE category = ?
                AND instance_id = (SELECT id FROM instances WHERE TRIM(name) = TRIM(?))
                AND path <> ? COLLATE path
              LIMIT 1",
        )
        .bind(&category)
        .bind(&mapping.instance_name)
        .bind(&mapping.path)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(other) = taken {
            report.skipped.push(format!(
                "mapping '{}' → '{}' on '{}': that category already maps '{other}' there",
                mapping.path, mapping.category, mapping.instance_name
            ));
            continue;
        }

        let affected = sqlx::query(
            "UPDATE root_folders SET category = ?
              WHERE path = ? COLLATE path
                AND instance_id = (SELECT id FROM instances WHERE TRIM(name) = TRIM(?))",
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
    // keeps across installations, on the instance of the name they were pinned
    // under. A bundle naming none cannot tell two copies apart, so it pins
    // every copy, and says so.
    for over in &bundle.overrides {
        let media_ids: Vec<String> = sqlx::query_scalar(
            "SELECT m.id FROM media m JOIN instances i ON i.id = m.instance_id
              WHERE m.media_type = ?
                AND ((m.tmdb_id IS NOT NULL AND m.tmdb_id = ?)
                     OR (m.tvdb_id IS NOT NULL AND m.tvdb_id = ?))
                AND (? IS NULL OR TRIM(i.name) = TRIM(?))
              ORDER BY m.id",
        )
        .bind(&over.media_type)
        .bind(over.tmdb_id)
        .bind(over.tvdb_id)
        .bind(&over.instance_name)
        .bind(&over.instance_name)
        .fetch_all(&mut *tx)
        .await?;

        if media_ids.is_empty() {
            let place = over.instance_name.as_deref().map_or_else(
                || "the library".to_string(),
                |instance| format!("the library of '{instance}'"),
            );
            report.skipped.push(format!(
                "override for '{}' → '{}': that media is not in {place} yet",
                over.media_title, over.target_category
            ));
            continue;
        }

        // The same two checks `POST /overrides` applies. An override
        // short-circuits the engine entirely, so one naming a category this
        // installation does not have routes its item nowhere, silently, and
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

        if over.instance_name.is_none() && media_ids.len() > 1 {
            report.skipped.push(format!(
                "override for '{}' → '{}': the bundle names no instance, so it is pinned on \
                 every copy ({})",
                over.media_title,
                over.target_category,
                media_ids.len()
            ));
        }
        for media_id in &media_ids {
            sqlx::query(
                "INSERT INTO overrides (id, media_id, target_category, reason)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(media_id) DO UPDATE SET
                    target_category = excluded.target_category,
                    reason = excluded.reason",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(media_id)
            .bind(&category)
            .bind(&over.reason)
            .execute(&mut *tx)
            .await?;
            report.overrides += 1;
        }
    }

    // Rules last, once the instances they are scoped to exist under the names
    // the bundle gives them.
    let ids: std::collections::HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT name, id FROM instances")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let mut restored = Vec::new();
    for bundled in importable {
        let target = bundled.rule.target_category.trim().to_lowercase();
        if !categories.contains(&target) {
            report.skipped.push(format!(
                "rule '{}': category '{target}' was removed meanwhile",
                bundled.rule.name
            ));
            continue;
        }
        // A configuration bundle has always carried the scope by name.
        match super::rules::scope_here(bundled, true, &ids, &localizer) {
            super::rules::Scoped::Kept(rule, note) => {
                report.adjusted.extend(note);
                restored.push(rule);
            }
            super::rules::Scoped::Refused(why) => report.skipped.push(why),
        }
    }
    // A replace in which no rule survives would delete every rule and add
    // none, and the next pass would route the library to the fallback
    // category, as `POST /rules/import` refuses to.
    if req.replace_rules && !restored.is_empty() {
        sqlx::query("DELETE FROM rules").execute(&mut *tx).await?;
    } else if req.replace_rules && !bundle.rules.is_empty() {
        report.skipped.push("rules: none could be restored, so the rules in place are kept".into());
    }
    for rule in &restored {
        super::rules::insert_rule(&mut tx, &Uuid::new_v4().to_string(), rule, &localizer).await?;
        report.rules += 1;
    }

    tx.commit().await?;
    crate::api::auth::audited(&state, &identity, client, bundled);
    Ok(Json(report))
}

/// The category names in the table and those `bundle` creates, through the
/// same gate its loop applies: a name about to be refused does not count as
/// something `default_category`, a mapping or a rule may point at.
fn with_bundled(
    mut names: Vec<String>,
    bundle: &ConfigBundle,
    localizer: &crate::localization::Localizer,
) -> Vec<String> {
    for category in &bundle.categories {
        if let Ok(name) = crate::api::categories::normalise(&category.name, localizer)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}
