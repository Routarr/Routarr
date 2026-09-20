//! Global settings.

use super::Json;
use axum::extract::State;
use serde::Deserialize;
use std::collections::HashMap;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The most backups that may be kept.
///
/// A ceiling rather than a matter of taste: this is the length of a list the
/// interface renders in full, and the number of archives a disk has to hold.
const MAX_BACKUPS_KEPT: i64 = 50;

/// Settings Routarr knows about, with the validation each one requires.
///
/// An unknown key is rejected rather than silently stored: a typo like
/// `global_dryrun` would otherwise look saved while dry-run stayed on.
const KNOWN: &[(&str, Kind)] = &[
    ("default_category", Kind::Category),
    ("global_dry_run", Kind::Bool),
    ("auto_sync_enabled", Kind::Bool),
    ("auto_simulate_enabled", Kind::Bool),
    ("auto_apply_enabled", Kind::Bool),
    ("refresh_after_move", Kind::Bool),
    ("move_files_default", Kind::Bool),
    // Every count is bounded on both sides. `RetentionCount` was the only one
    // that was, and its reason — a number that decides how long a list gets and
    // how much disk it costs — is not special to backups. Unbounded, each of
    // these has a value that reads as "off" while the interface reports it as
    // set: an interval of a million hours is a backup that never runs.
    ("batch_limit", Kind::Bounded(1, 1_000)),
    ("confirmation_threshold", Kind::Bounded(1, 10_000)),
    // Ten years. Past that the intent is "never expire", which should be said
    // rather than approximated with a big number.
    ("metadata_cache_ttl_days", Kind::Bounded(1, 3_650)),
    ("metadata_providers", Kind::ProviderList),
    ("backup_enabled", Kind::Bool),
    // A week, which is what `jobs::scheduler` clamps this to when it reads it.
    // Bounded at a year, the interface accepted a number that was silently not
    // the one running — the same defect the line below was written to fix.
    ("backup_interval_hours", Kind::Bounded(1, 24 * 7)),
    ("backup_retention_count", Kind::Retention(1, MAX_BACKUPS_KEPT)),
    ("decision_retention_days", Kind::NonNegativeInt),
    ("log_retention_days", Kind::NonNegativeInt),
    // The same bounds the scheduler already clamps to when it reads this. The
    // clamp stays — it is the guard — but refusing here means the number on
    // screen is the number that runs, instead of one silently ignored.
    ("scheduler_interval_minutes", Kind::Bounded(1, 24 * 60)),
    ("certification_regions", Kind::CountryList),
    ("notification_webhook_url", Kind::WebhookUrl),
    ("ui_language", Kind::Language),
    ("ui_theme", Kind::Theme),
    // The metadata credentials. Sealed on the way in and never returned, the
    // same posture as an Arr's key — which is the *more* dangerous of the two,
    // since it writes to the library while these only read.
    ("tmdb_api_key", Kind::Secret),
    ("omdb_api_key", Kind::Secret),
    ("tvdb_api_key", Kind::Secret),
];

/// What a setting's value has to be, and therefore how it is validated.
///
/// One variant per shape rather than a free-form validator per key: the table
/// above then reads as a list of settings, and the reason for a bound sits
/// beside the setting it bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Bool,
    /// Stored sealed and never read back out. An empty value clears it, which
    /// is how a source is turned off from the interface.
    Secret,
    WebhookUrl,
    /// A whole number within an inclusive range, both ends stated at the table
    /// above so the reason for each ceiling sits beside the setting it bounds.
    /// Converged at startup when stored outside the range — see
    /// `maintenance::converge_setting_bounds`.
    Bounded(i64, i64),
    /// A count of things kept: bounded on save like `Bounded`, raised to its
    /// floor at startup like `Bounded`, and never lowered by one. Lowering it
    /// removes what is beyond it, so nothing but the operator's own save may;
    /// `offline_warnings` names a stored value above the ceiling until then.
    Retention(i64, i64),
    NonNegativeInt,
    Category,
    CountryList,
    Language,
    Theme,
    ProviderList,
}

impl Kind {
    /// The inclusive range a value must sit in to be saved, if it is a number
    /// with one. The one place the two ranged kinds are read alike.
    fn range(self) -> Option<(i64, i64)> {
        match self {
            Kind::Bounded(min, max) | Kind::Retention(min, max) => Some((min, max)),
            _ => None,
        }
    }

    /// Whether a start may *lower* a stored value into its range. Raising is
    /// always safe — a floor of one keeps more, not less — while lowering a
    /// retention count removes what is beyond it.
    fn lowered_at_startup(self) -> bool {
        matches!(self, Kind::Bounded(..))
    }
}

pub async fn get_all(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&state.pool)
            .await?;

    let mut settings = serde_json::Map::new();
    for (key, value) in rows {
        // A sealed value never leaves the process. The screen needs to know
        // whether one is set, not what it is — so it gets a boolean under a
        // separate key and the value itself reads empty, which is also what
        // makes the field safe to leave untouched on the next save.
        let secret = KNOWN.iter().any(|(k, kind)| *k == key && *kind == Kind::Secret);
        if secret {
            let configured = !value.trim().is_empty();
            settings.insert(format!("{key}_configured"), serde_json::Value::Bool(configured));
            settings.insert(key, serde_json::Value::String(String::new()));
            continue;
        }
        settings.insert(key, serde_json::Value::String(value));
    }

    // Surface defaults for keys the database has not seen yet, so the Settings
    // screen can render every knob without guessing.
    for (key, _) in KNOWN {
        settings.entry(key.to_string()).or_insert(serde_json::Value::String(String::new()));
    }

    Ok(Json(serde_json::Value::Object(settings)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub settings: HashMap<String, String>,
}

/// Whether a value may be stored under this key.
///
/// Public because the settings table has a second writer: `POST /config/import`
/// writes it from a bundle. Both go through this, or the bundle becomes a way
/// to store what the API refuses — a `default_category` naming a category that
/// does not exist, a source list without `arr`, a theme nothing renders.
pub fn check(key: &str, value: &str, categories: &[String]) -> AppResult<()> {
    let kind = KNOWN
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, kind)| *kind)
        .ok_or_else(|| AppError::BadRequest(format!("Unknown setting '{key}'")))?;
    validate(key, value, kind, categories)
}

/// The range a `Bounded` key is converged into, if it is one.
///
/// Read by the startup sweep that converges values stored before a bound
/// existed: `PUT /settings` validates the whole payload and the Settings screen
/// always sends every field, so one out-of-range value left in the table blocks
/// *every* save — with an error naming a field in a tab the operator never
/// opened. A `Retention` count answers its floor and no ceiling: raised like
/// any other, never lowered by a start.
pub fn bounds(key: &str) -> Option<(i64, i64)> {
    KNOWN.iter().find(|(k, _)| *k == key).and_then(|(_, kind)| {
        let (min, max) = kind.range()?;
        Some((min, if kind.lowered_at_startup() { max } else { i64::MAX }))
    })
}

/// Every retention count with its ceiling, for the warning that names a
/// stored value above one.
pub fn retention_counts() -> impl Iterator<Item = (&'static str, i64)> {
    KNOWN.iter().filter_map(|(k, kind)| match kind {
        Kind::Retention(_, max) => Some((*k, *max)),
        _ => None,
    })
}

/// Every ranged key: its save range, and whether a start may lower it. Only a
/// test asks, so the matrix test reads the table as `bounds` does rather than
/// naming the keys it happens to know.
#[cfg(test)]
pub fn ranged_keys() -> Vec<(&'static str, i64, i64, bool)> {
    KNOWN
        .iter()
        .filter_map(|(k, kind)| {
            kind.range().map(|(min, max)| (*k, min, max, kind.lowered_at_startup()))
        })
        .collect()
}

/// Whether this key holds a sealed value.
///
/// Sealed with a master key one installation holds, so it is meaningless
/// anywhere else: a bundle must neither carry it nor accept it.
pub fn is_secret(key: &str) -> bool {
    KNOWN.iter().any(|(k, kind)| *k == key && *kind == Kind::Secret)
}

pub async fn update(
    State(state): State<AppState>,
    Json(req): Json<UpdateSettingsRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let categories: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?;

    // Validate everything before writing anything: a half-applied settings save
    // is worse than a rejected one.
    for (key, value) in &req.settings {
        check(key, value, &categories)?;
    }

    let mut tx = state.pool.begin().await?;
    for (key, value) in &req.settings {
        let kind = KNOWN.iter().find(|(k, _)| k == key).map(|(_, kind)| *kind);
        let stored = match kind {
            // Sealed here rather than in the client, so a value reaching the
            // table in plaintext is impossible whatever the caller sent. An
            // empty value stays empty: it is how the source is switched off,
            // and sealing nothing would store an opaque blob meaning "unset".
            Some(Kind::Secret) if !value.trim().is_empty() => state.secrets.seal(value.trim())?,
            _ => value.trim().to_string(),
        };
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(&stored)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // Pruning only after a backup is taken would leave every archive above the
    // new retention on disk — and on screen — until the next scheduled run.
    // The number takes effect when it is set.
    if req.settings.contains_key("backup_retention_count")
        && let Err(e) = crate::services::backup::prune(&state).await
    {
        tracing::warn!("Could not prune backups after the retention changed: {e}");
    }

    Ok(Json(serde_json::json!({ "updated": req.settings.len() })))
}

fn validate(key: &str, value: &str, kind: Kind, categories: &[String]) -> AppResult<()> {
    let value = value.trim();
    let bad = |message: String| AppError::BadRequest(message);

    match kind {
        // Nothing to check beyond a shape nobody can predict: TMDb accepts a v3
        // key or a v4 token, OMDb an eight-character string, TheTVDB a UUID.
        // A pattern guessed here would reject a credential the provider accepts,
        // which is worse than letting the source report that it cannot connect —
        // and the diagnostics screen already probes each one.
        Kind::Secret => {}
        Kind::WebhookUrl => {
            // Empty means "no notifications", which is the default and not an
            // error. Anything else has to be a URL we could actually POST to:
            // a typo here fails silently in the background, where nobody sees
            // it — which is precisely what this feature exists to prevent.
            let usable =
                value.is_empty() || value.starts_with("http://") || value.starts_with("https://");
            if !usable {
                return Err(bad(format!("'{key}' must be an http:// or https:// URL")));
            }
        }
        Kind::Bool => {
            if !matches!(value.to_lowercase().as_str(), "true" | "false") {
                return Err(bad(format!("'{key}' must be 'true' or 'false'")));
            }
        }
        // Bounded on both sides. A floor alone accepts "keep 10 000 backups",
        // whose card grows without end and whose disk fills with the very thing
        // meant to protect it — and accepts a backup interval of a million
        // hours, which reads as configured and never runs.
        Kind::Bounded(min, max) | Kind::Retention(min, max) => {
            let n: i64 =
                value.parse().map_err(|_| bad(format!("'{key}' must be a whole number")))?;
            if !(min..=max).contains(&n) {
                return Err(bad(format!("'{key}' must be between {min} and {max}")));
            }
        }
        Kind::NonNegativeInt => {
            let n: i64 =
                value.parse().map_err(|_| bad(format!("'{key}' must be a whole number")))?;
            if n < 0 {
                return Err(bad(format!("'{key}' cannot be negative")));
            }
        }
        Kind::Category => {
            if !categories.iter().any(|c| c == value) {
                return Err(bad(format!("'{key}': category '{value}' does not exist")));
            }
        }
        Kind::Language => {
            if !crate::localization::is_supported(value) {
                return Err(bad(format!("'{key}': '{value}' is not a supported language")));
            }
        }
        Kind::Theme => {
            if !matches!(value, "dark" | "light" | "auto") {
                return Err(bad(format!("'{key}' must be 'dark', 'light' or 'auto'")));
            }
        }
        Kind::ProviderList => {
            // An unknown id is a typo, and a silently ignored typo would look
            // like a source that simply never answers. `arr` cannot be left
            // out: its data arrives with the synchronisation at no cost, and a
            // list without it makes every metadata rule dead.
            let ids: Vec<&str> =
                value.split(',').map(str::trim).filter(|v| !v.is_empty()).collect();
            for id in &ids {
                if crate::services::metadata::info(id).is_none() {
                    return Err(bad(format!("'{key}': '{id}' is not a known metadata source")));
                }
            }
            if !ids.contains(&"arr") {
                return Err(bad(format!("'{key}' must include 'arr', the Arr's own metadata")));
            }
        }
        Kind::CountryList => {
            let codes: Vec<&str> =
                value.split(',').map(str::trim).filter(|c| !c.is_empty()).collect();
            if codes.is_empty() {
                return Err(bad(format!("'{key}' needs at least one ISO 3166-1 country code")));
            }
            if let Some(bad_code) =
                codes.iter().find(|c| c.len() != 2 || !c.chars().all(|ch| ch.is_ascii_alphabetic()))
            {
                return Err(bad(format!("'{key}': '{bad_code}' is not a two-letter country code")));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_must_be_true_or_false() {
        assert!(validate("global_dry_run", "true", Kind::Bool, &[]).is_ok());
        assert!(validate("global_dry_run", "yes", Kind::Bool, &[]).is_err());
    }

    #[test]
    fn batch_limit_must_be_positive() {
        assert!(check("batch_limit", "50", &[]).is_ok());
        assert!(check("batch_limit", "0", &[]).is_err());
        assert!(check("batch_limit", "-1", &[]).is_err());
        assert!(check("batch_limit", "many", &[]).is_err());
    }

    /// A ceiling on every count, not only on the one that grows a list. Without
    /// it each of these has a value that reads as configured and behaves as
    /// off — a backup every million hours, a cache that never expires.
    #[test]
    fn every_count_is_bounded_at_both_ends() {
        for key in [
            "batch_limit",
            "confirmation_threshold",
            "metadata_cache_ttl_days",
            "backup_interval_hours",
            "backup_retention_count",
            "scheduler_interval_minutes",
        ] {
            assert!(check(key, "1", &[]).is_ok(), "{key} refuses its floor");
            assert!(check(key, "0", &[]).is_err(), "{key} has no floor");
            assert!(check(key, &i64::MAX.to_string(), &[]).is_err(), "{key} has no ceiling");
        }
    }

    /// The scheduler clamps this when it reads it. Refusing here is what stops
    /// the interface showing a number that is silently not the one running.
    #[test]
    fn the_scheduler_interval_refuses_what_it_would_have_clamped() {
        assert!(check("scheduler_interval_minutes", "1440", &[]).is_ok());
        assert!(check("scheduler_interval_minutes", "1441", &[]).is_err());
    }

    #[test]
    fn retention_may_be_zero_to_disable() {
        assert!(validate("log_retention_days", "0", Kind::NonNegativeInt, &[]).is_ok());
    }

    #[test]
    fn default_category_must_exist() {
        let categories = vec!["standard".to_string()];
        assert!(validate("default_category", "standard", Kind::Category, &categories).is_ok());
        assert!(validate("default_category", "kids", Kind::Category, &categories).is_err());
    }

    #[test]
    fn only_shipped_languages_are_accepted() {
        assert!(validate("ui_language", "fr", Kind::Language, &[]).is_ok());
        assert!(validate("ui_language", "en", Kind::Language, &[]).is_ok());
        assert!(validate("ui_language", "kl", Kind::Language, &[]).is_err());
    }

    #[test]
    fn the_theme_is_one_of_three_states() {
        assert!(validate("ui_theme", "dark", Kind::Theme, &[]).is_ok());
        assert!(validate("ui_theme", "light", Kind::Theme, &[]).is_ok());
        // `auto` follows the operating system; an explicit choice overrides it.
        assert!(validate("ui_theme", "auto", Kind::Theme, &[]).is_ok());
        assert!(validate("ui_theme", "midnight", Kind::Theme, &[]).is_err());
        assert!(validate("ui_theme", "", Kind::Theme, &[]).is_err());
    }

    #[test]
    fn the_source_list_accepts_known_ids_in_any_order() {
        assert!(validate("metadata_providers", "tmdb,arr", Kind::ProviderList, &[]).is_ok());
        assert!(validate("metadata_providers", "arr", Kind::ProviderList, &[]).is_ok());
    }

    #[test]
    fn a_misspelled_source_is_refused_rather_than_ignored() {
        assert!(validate("metadata_providers", "arr,tmbd", Kind::ProviderList, &[]).is_err());
    }

    #[test]
    fn country_list_is_checked() {
        assert!(validate("certification_regions", "FR, US", Kind::CountryList, &[]).is_ok());
        assert!(validate("certification_regions", "FRA", Kind::CountryList, &[]).is_err());
        assert!(validate("certification_regions", "", Kind::CountryList, &[]).is_err());
    }
}
