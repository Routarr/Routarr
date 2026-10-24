//! Exporting and restoring the configuration: rules, folder mappings and the
//! overrides no resync brings back, keyed by meaning, since ids and sealed keys
//! belong to one installation. A full backup is `services::backup`.

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

/// An Arr connection without its key, which is sealed for one installation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// Always false: the import reads no key, and the instance arrives disabled,
    /// named in the report's `needs_key`.
    #[serde(default)]
    pub has_api_key: bool,
    /// Whether automatic application may write to the instance. On when the bundle omits it.
    #[serde(default = "on")]
    pub auto_apply: bool,
}

fn on() -> bool {
    true
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
    // A bundle may carry `locked`: read and dropped, since refusing it fails the import.
    #[serde(rename = "locked", default, skip_serializing)]
    pub _locked: Option<serde::de::IgnoredAny>,
}

pub async fn export(State(state): State<AppState>) -> AppResult<Json<ConfigBundle>> {
    let pool = &state.pool;

    // Sealed values stay behind: nothing elsewhere opens them, yet they would read as set.
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

    let instances: Vec<Instance> = sqlx::query_as::<_, (String, String, String, bool, i64, bool)>(
        "SELECT name, instance_type, base_url, enabled, sync_interval_minutes, auto_apply
           FROM instances ORDER BY name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(name, instance_type, base_url, enabled, sync_interval_minutes, auto_apply)| Instance {
        name,
        instance_type,
        base_url: crate::http::without_credentials(&base_url).into_owned(),
        enabled,
        sync_interval_minutes,
        has_api_key: false,
        auto_apply,
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
    /// Everything that could not be restored, and why, so nothing drops silently.
    pub skipped: Vec<String>,
    /// The instances restored disabled, by name, each waiting for its API key.
    /// Kept out of `skipped`, which the interface reads as what failed.
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
    let localizer = state.localizer().await;
    if bundle.version != BUNDLE_VERSION {
        return Err(AppError::BadRequest(localizer.translate(
            "ErrorBundleVersion",
            &[("version", &bundle.version.to_string()), ("supported", &BUNDLE_VERSION.to_string())],
        )));
    }

    let mut report = ImportReport::default();
    let skip = |key: &str, values: &[(&str, &str)]| localizer.translate(key, values);

    // Settings and mappings may name the bundle's own categories as well as the table's.
    let mut categories = with_bundled(
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?,
        &bundle,
        &localizer,
    );

    // Each source lacking its key here is dropped alone, since no key travels in a bundle.
    let mut keyless = Vec::new();
    for setting in bundle.settings.iter().filter(|s| s.key == "metadata_providers") {
        let empty = std::collections::HashMap::new();
        keyless.extend(
            super::settings::sources_without_their_key(&state, &setting.value, &empty).await,
        );
    }

    // Judged outside the transaction: a one-connection pool cannot serve both at once.
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
                let reason = errors.join(" · ");
                report.skipped.push(skip(
                    "ImportSkippedRule",
                    &[("name", &bundled.rule.name), ("reason", &reason)],
                ));
            }
        }
    }

    let mut tx = crate::db::write_transaction(&state.pool).await?;
    // Read again under the write lock, or a category removed meanwhile gets named below.
    categories = with_bundled(
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&mut *tx).await?,
        &bundle,
        &localizer,
    );

    // Settings first: everything after can depend on them.
    for setting in &bundle.settings {
        let key = setting.key.as_str();

        // A credential in a bundle is refused: sealed elsewhere, or shared in the clear.
        if crate::services::settings::is_secret(key) {
            report.skipped.push(skip("ImportSkippedCredential", &[("key", key)]));
            continue;
        }

        // The gate `PUT /settings` applies: an unknown key would stay in the table for good.
        let checked = crate::api::settings::stored_value(key, &setting.value);
        if let Err(e) = crate::services::settings::check(key, &checked, &categories, &localizer) {
            let reason = e.public_message();
            report.skipped.push(skip("ImportSkippedSetting", &[("key", key), ("reason", &reason)]));
            continue;
        }
        let mut value = checked;
        if key == "metadata_providers" && !keyless.is_empty() {
            for id in &keyless {
                report.skipped.push(skip("ImportSkippedSourceKeyless", &[("source", id)]));
            }
            value = value
                .split(',')
                .map(str::trim)
                .filter(|id| !keyless.iter().any(|keyless| keyless == id))
                .collect::<Vec<_>>()
                .join(",");
        }

        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        // The form the gate checked: the raw `"Anime "` would name no category.
        .bind(&value)
        .execute(&mut *tx)
        .await?;
        report.settings += 1;
    }

    for category in &bundle.categories {
        // The gate `POST /categories` applies, or a bad name lands where `rename` cannot fix it.
        let name = match crate::api::categories::normalise(&category.name, &localizer) {
            Ok(name) => name,
            Err(e) => {
                let reason = e.public_message();
                report.skipped.push(skip(
                    "ImportSkippedCategory",
                    &[("name", &category.name), ("reason", &reason)],
                ));
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

    // By `instances::name_key`, the bundle's own instances added as they are inserted.
    let mut ids = super::instances::ids_by_name(&mut *tx).await?;
    // Disabled, since an instance without its key fails on every scheduler tick.
    for instance in &bundle.instances {
        // The check `POST /instances` applies, or the edit screen could never save the row.
        let checked = super::instances::validate(
            &instance.name,
            &instance.instance_type,
            &instance.base_url,
            &localizer,
        );
        let base_url = match checked {
            Ok(base_url) => base_url,
            Err(e) => {
                let reason = e.public_message();
                report.skipped.push(skip(
                    "ImportSkippedInstance",
                    &[("name", &instance.name), ("reason", &reason)],
                ));
                continue;
            }
        };

        // Stored as `POST /instances` stores it.
        let name = instance.name.trim();
        let key = super::instances::name_key(name);
        if ids.contains_key(&key) {
            report.skipped.push(skip("ImportSkippedInstanceExists", &[("name", &instance.name)]));
            continue;
        }

        // Sealed though empty, so the instance reads as waiting for its key.
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO instances (id, name, name_key, instance_type, base_url, api_key, enabled,
             sync_interval_minutes, webhook_token, auto_apply)
             VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(&key)
        .bind(instance.instance_type.to_lowercase())
        .bind(&base_url)
        .bind(state.secrets.seal("")?)
        .bind(instance.sync_interval_minutes.clamp(1, crate::jobs::MAX_SYNC_INTERVAL_MINUTES))
        .bind(Uuid::new_v4().to_string())
        .bind(instance.auto_apply)
        .execute(&mut *tx)
        .await?;
        ids.insert(key, id);
        report.instances += 1;
        report.needs_key.push(name.to_string());
    }

    // Matched on instance name and path, which mean the same on another machine.
    for mapping in &bundle.root_folders {
        // In the form the category loop writes, or `Anime` names a row nobody holds.
        let category = crate::api::categories::stored_form(&mapping.category);

        // No foreign key guards a category, so a mapping naming a missing one routes nowhere.
        let mapped = [
            ("path", mapping.path.as_str()),
            ("category", mapping.category.as_str()),
            ("instance", mapping.instance_name.as_str()),
        ];
        if !categories.contains(&category) {
            report.skipped.push(skip("ImportSkippedMappingNoCategory", &mapped));
            continue;
        }

        let instance_id = ids.get(&super::instances::name_key(&mapping.instance_name));
        let affected = sqlx::query(
            "UPDATE root_folders SET category = ? WHERE path = ? COLLATE path AND instance_id = ?",
        )
        .bind(&category)
        .bind(&mapping.path)
        .bind(instance_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if affected == 0 {
            // Expected on a fresh install: root folders exist once the instance has synced.
            report.skipped.push(skip("ImportSkippedMappingUnsynced", &mapped));
        } else {
            report.root_folders += 1;
        }
    }

    // Matched on external id within the named instance, or on every copy when none is named.
    for over in &bundle.overrides {
        // A name no instance holds names no copy.
        let instance_id = over
            .instance_name
            .as_deref()
            .map(|name| ids.get(&super::instances::name_key(name)).map_or("", String::as_str));
        let media_ids: Vec<String> = sqlx::query_scalar(
            "SELECT m.id FROM media m
              WHERE m.media_type = ?
                AND ((m.tmdb_id IS NOT NULL AND m.tmdb_id = ?)
                     OR (m.tvdb_id IS NOT NULL AND m.tvdb_id = ?))
                AND (? IS NULL OR m.instance_id = ?)
              ORDER BY m.id",
        )
        .bind(&over.media_type)
        .bind(over.tmdb_id)
        .bind(over.tvdb_id)
        .bind(instance_id)
        .bind(instance_id)
        .fetch_all(&mut *tx)
        .await?;

        let pinned =
            [("title", over.media_title.as_str()), ("category", over.target_category.as_str())];
        if media_ids.is_empty() {
            let skipped = match over.instance_name.as_deref() {
                Some(instance) => {
                    let values = [pinned.as_slice(), &[("instance", instance)]].concat();
                    skip("ImportSkippedExceptionNotOnInstance", &values)
                }
                None => skip("ImportSkippedExceptionNotInLibrary", &pinned),
            };
            report.skipped.push(skipped);
            continue;
        }

        // An override skips the engine, so one on a missing category would route nowhere unseen.
        let category = crate::api::categories::stored_form(&over.target_category);
        if !categories.contains(&category) {
            report.skipped.push(skip("ImportSkippedExceptionNoCategory", &pinned));
            continue;
        }

        if over.instance_name.is_none() && media_ids.len() > 1 {
            let copies = media_ids.len().to_string();
            let values = [pinned.as_slice(), &[("copies", copies.as_str())]].concat();
            report.skipped.push(skip("ImportExceptionOnEveryCopy", &values));
        }
        // As `POST /exceptions` pins: by the importer, withdrawing the proposals it now decides.
        let reason = over.reason.as_deref();
        let by = identity.attribution();
        match super::overrides::pin_in(&mut tx, &media_ids, &category, reason, &by, &localizer)
            .await
        {
            Ok(()) => report.overrides += media_ids.len(),
            Err(AppError::BadRequest(why)) => {
                report.skipped.push(skip(
                    "ImportSkippedException",
                    &[("title", &over.media_title), ("reason", &why)],
                ));
            }
            Err(e) => return Err(e),
        }
    }

    // Rules last, once the instances they are scoped to exist under their bundled names.
    let mut restored = Vec::new();
    for bundled in importable {
        let target = crate::api::categories::stored_form(&bundled.rule.target_category);
        if !categories.contains(&target) {
            report.skipped.push(skip(
                "ImportSkippedRuleCategoryRemoved",
                &[("name", &bundled.rule.name), ("category", &target)],
            ));
            continue;
        }
        // A configuration bundle carries the scope by name.
        match super::rules::scope_here(bundled, true, &ids, &localizer) {
            super::rules::Scoped::Kept(rule, note) => {
                report.adjusted.extend(note);
                restored.push((rule, bundled.marker()));
            }
            super::rules::Scoped::Refused(why) => report.skipped.push(why),
        }
    }
    // A replace with no surviving rule would send the whole library to the fallback category.
    if req.replace_rules && !restored.is_empty() {
        sqlx::query("DELETE FROM rules").execute(&mut *tx).await?;
    } else {
        if req.replace_rules && !bundle.rules.is_empty() {
            report.skipped.push(skip("ImportRulesKept", &[]));
        }
        let present;
        (restored, present) = super::rules::appended(&mut tx, restored, &localizer).await?;
        report.adjusted.extend(present);
    }
    for (rule, marker) in &restored {
        let id = Uuid::new_v4().to_string();
        super::rules::insert_rule(&mut tx, &id, rule, *marker, &localizer).await?;
        report.rules += 1;
    }

    tx.commit().await?;
    // As `PUT /settings` does: archives above a lowered retention go now.
    if bundle.settings.iter().any(|setting| setting.key == "backup_retention_count") {
        crate::services::backup::retention_changed(&state, identity.attribution()).await;
    }
    crate::api::auth::audited(&state, &identity, client, bundled);
    Ok(Json(report))
}

/// The category names in the table and those `bundle` creates, through the
/// gate its loop applies, so a name about to be refused counts for nothing.
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
