//! The settings Routarr knows, and what each one may hold.

use crate::error::{AppError, AppResult};
use crate::localization::Localizer;

/// The values the `onboarding` setting may hold.
pub const ONBOARDING_STATES: [&str; 3] = ["pending", "dismissed", "done"];

/// The longest retention in days a setting may hold.
const MAX_RETENTION_DAYS: i64 = 3_650;

/// The most backups that may be kept: the length of a list the interface renders in full, and
/// the number of archives a disk has to hold.
const MAX_BACKUPS_KEPT: i64 = 50;

/// Settings Routarr knows about, with the validation each one requires. An unknown key is
/// refused: a typo like `global_dryrun` would look saved while dry run stayed on.
const KNOWN: &[(&str, Kind, &str)] = &[
    ("default_category", Kind::Category, crate::state::DEFAULT_CATEGORY),
    ("global_dry_run", Kind::Bool, "true"),
    ("auto_sync_enabled", Kind::Bool, "true"),
    ("auto_simulate_enabled", Kind::Bool, "true"),
    ("auto_apply_enabled", Kind::Bool, "false"),
    // Every count is bounded on both sides: unbounded, each has a value that reads as set and
    // behaves as off, as a backup every million hours.
    ("batch_limit", Kind::Bounded(1, 1_000), "50"),
    ("confirmation_threshold", Kind::Bounded(1, 10_000), "10"),
    // Ten years. Past that the intent is "never expire", which should be said
    // rather than approximated with a big number.
    ("metadata_cache_ttl_days", Kind::Bounded(1, 3_650), "7"),
    // OMDb's patron keys allow far more than a free key's thousand, and its
    // pacing sends fewer than half a million a day, so a million is no limit.
    ("omdb_daily_requests", Kind::Bounded(1, 1_000_000), "1000"),
    ("metadata_providers", Kind::ProviderList, crate::services::metadata::ARR),
    ("anime_search", Kind::AnimeSearch, crate::services::metadata::ANIME_SEARCH[0]),
    // Off until somebody turns it on: the download is a request to a third party.
    ("anime_ids_weekly", Kind::Bool, "false"),
    ("backup_enabled", Kind::Bool, "true"),
    // A week, which `jobs::scheduler` clamps it to: a wider bound would show a number that is
    // not the one running.
    ("backup_interval_hours", Kind::Bounded(1, 24 * 7), "24"),
    ("backup_retention_count", Kind::Retention(1, MAX_BACKUPS_KEPT), "7"),
    // Every archive sealed with it, so that a copy of the backup folder opens
    // nothing without it (`services::backup::sealed`).
    ("backup_passphrase", Kind::Passphrase, ""),
    // Zero keeps everything, and ten years is the most a count of days is
    // worth: further, SQLite's date arithmetic runs past every stored date
    // and then to NULL, which purges nothing while the screen shows a number.
    ("decision_retention_days", Kind::Retention(0, MAX_RETENTION_DAYS), "30"),
    ("log_retention_days", Kind::Retention(0, MAX_RETENTION_DAYS), "90"),
    ("security_log_retention_days", Kind::Retention(0, MAX_RETENTION_DAYS), "365"),
    // The bounds the scheduler clamps it to, as for `backup_interval_hours`.
    ("scheduler_interval_minutes", Kind::Bounded(1, 24 * 60), "15"),
    ("certification_regions", Kind::CountryList, "US"),
    ("notification_webhook_url", Kind::WebhookUrl, ""),
    ("notification_format", Kind::NotificationFormat, "auto"),
    // What the webhook receives beside the failures, which it always does.
    ("notify_sync_failed", Kind::Bool, "false"),
    ("notify_simulation_completed", Kind::Bool, "false"),
    ("notify_moves_completed", Kind::Bool, "false"),
    ("ui_language", Kind::Language, crate::localization::DEFAULT_LANGUAGE),
    ("ui_theme", Kind::Theme, "dark"),
    // Whether the getting-started guide is still wanted. Written by
    // `PUT /onboarding`, listed here so a configuration bundle carries it.
    ("onboarding", Kind::Onboarding, "pending"),
    // Each key is named `<id>_api_key` after its source's id in `metadata::PROVIDERS`, the name
    // `AppState::provider_key_from` builds: a key stored under any other is never read.
    ("tmdb_api_key", Kind::Secret, ""),
    ("omdb_api_key", Kind::Secret, ""),
    ("tvdb_api_key", Kind::Secret, ""),
    // A MyAnimeList client ID, read by the official API's source under the name its id builds.
    ("myanimelist_api_key", Kind::Secret, ""),
    // What TheTVDB asks beside a user-supported key, read by
    // `AppState::tvdb_pin_from` under this name.
    ("tvdb_subscriber_pin", Kind::Secret, ""),
];

/// What a setting's value has to be, and therefore how it is validated.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Bool,
    /// A credential of no shape anyone can predict, sealed (see [`Kind::sealed`]).
    Secret,
    /// An `http://` or `https://` address, sealed like a `Secret`: whoever
    /// holds a Discord or Slack webhook address can post to the channel.
    WebhookUrl,
    /// A passphrase a person chose, sealed like a `Secret`, and set only
    /// through its own route.
    Passphrase,
    /// A whole number within an inclusive range, converged into it at startup
    /// (`maintenance::converge_setting_bounds`).
    Bounded(i64, i64),
    /// A count of things kept, bounded on save like `Bounded` and raised to its floor at startup,
    /// but never lowered by a start: lowering it removes what is beyond it. `offline_warnings`
    /// names a stored value above the ceiling.
    Retention(i64, i64),
    Category,
    CountryList,
    Language,
    Theme,
    /// One of `notify::FORMATS`.
    NotificationFormat,
    /// One of `metadata::ANIME_SEARCH`.
    AnimeSearch,
    ProviderList,
    Onboarding,
}

impl Kind {
    /// Whether the value is a credential: sealed on the way in, never read back
    /// out, left out of a bundle and resealed when the master key rotates. An
    /// empty value is stored empty, and that is how a credential is removed.
    fn sealed(self) -> bool {
        matches!(self, Kind::Secret | Kind::WebhookUrl | Kind::Passphrase)
    }

    /// The inclusive range a value must sit in to be saved, if it is a number with one.
    fn range(self) -> Option<(i64, i64)> {
        match self {
            Kind::Bounded(min, max) | Kind::Retention(min, max) => Some((min, max)),
            _ => None,
        }
    }

    /// Whether a start may lower a stored value into its range (see [`Kind::Retention`]).
    fn lowered_at_startup(self) -> bool {
        matches!(self, Kind::Bounded(..))
    }
}

/// Whether a value may be stored under this key. `POST /config/import` goes through it too, or a
/// bundle would store what `PUT /settings` refuses.
pub fn check(
    key: &str,
    value: &str,
    categories: &[String],
    localizer: &Localizer,
) -> AppResult<()> {
    let kind =
        KNOWN.iter().find(|(k, _, _)| *k == key).map(|(_, kind, _)| *kind).ok_or_else(|| {
            AppError::BadRequest(localizer.translate("ErrorSettingUnknown", &[("key", key)]))
        })?;
    validate(key, value, kind, categories, localizer)
}

/// What `key` holds when nothing is stored, or what is stored cannot be read:
/// one answer for every reader, the interface's fallback included
/// (`frontend/src/lib/settings.ts`, held to this by its test).
pub fn default_of(key: &str) -> &'static str {
    KNOWN.iter().find(|(k, _, _)| *k == key).map_or("", |(_, _, default)| *default)
}

/// Whether `key` holds a category's name, which its writers normalise before [`check`]
/// compares it.
pub fn names_a_category(key: &str) -> bool {
    KNOWN.iter().any(|(k, kind, _)| *k == key && matches!(kind, Kind::Category))
}

/// The label a setting is shown under, for a refusal or a warning that names it: only those the
/// form cannot check itself, and the retentions a warning names.
fn label(key: &str) -> &str {
    match key {
        "notification_webhook_url" => "SettingNotificationWebhook",
        "certification_regions" => "SettingCertificationRegions",
        "backup_retention_count" => "SettingBackupRetention",
        "decision_retention_days" => "SettingDecisionRetention",
        "log_retention_days" => "SettingLogRetention",
        "security_log_retention_days" => "SettingSecurityLogRetention",
        other => other,
    }
}

/// The range a ranged key is converged into at startup, a `Retention` count answering no
/// ceiling. Left out of range, one value would refuse every save that sends it back, as a caller
/// resending every value it read does.
pub fn bounds(key: &str) -> Option<(i64, i64)> {
    KNOWN.iter().find(|(k, _, _)| *k == key).and_then(|(_, kind, _)| {
        let (min, max) = kind.range()?;
        Some((min, if kind.lowered_at_startup() { max } else { i64::MAX }))
    })
}

/// Every retention count with the key of its label and its ceiling, for the
/// warning that names a stored value above one.
pub fn retention_counts() -> impl Iterator<Item = (&'static str, &'static str, i64)> {
    KNOWN.iter().filter_map(|(k, kind, _)| match kind {
        Kind::Retention(_, max) => Some((*k, label(k), *max)),
        _ => None,
    })
}

/// Every ranged key: its save range, and whether a start may lower it, for the tests.
#[cfg(test)]
pub fn ranged_keys() -> Vec<(&'static str, i64, i64, bool)> {
    KNOWN
        .iter()
        .filter_map(|(k, kind, _)| {
            kind.range().map(|(min, max)| (*k, min, max, kind.lowered_at_startup()))
        })
        .collect()
}

/// Whether this key holds a sealed value, which a bundle neither carries nor accepts.
pub fn is_secret(key: &str) -> bool {
    KNOWN.iter().any(|(k, kind, _)| *k == key && kind.sealed())
}

/// Every key that holds a sealed value, for the pass that reseals them all.
pub fn sealed_keys() -> Vec<&'static str> {
    KNOWN.iter().filter(|(_, kind, _)| kind.sealed()).map(|(k, _, _)| *k).collect()
}

fn validate(
    key: &str,
    value: &str,
    kind: Kind,
    categories: &[String],
    localizer: &Localizer,
) -> AppResult<()> {
    let value = value.trim();
    let bad = |message: String| AppError::BadRequest(message);
    // Read on the Settings screen, in the interface's language, naming the
    // field the value was typed in.
    let field = localizer.translate(label(key), &[]);
    let refused = |refusal: &str, params: &[(&str, &str)]| {
        let mut all = vec![("field", field.as_str())];
        all.extend_from_slice(params);
        AppError::BadRequest(localizer.translate(refusal, &all))
    };

    match kind {
        // Unchecked: TMDB takes a v3 key or a v4 token, OMDb eight characters, TheTVDB a UUID and a
        // PIN, and the diagnostics screen probes each one.
        Kind::Secret => {}
        // Set on its own, with the proof a session gives and the archives
        // converted to it (`api::backup::set_passphrase`).
        Kind::Passphrase => {
            return Err(bad(format!("'{key}' is set with PUT /api/v1/backups/passphrase")));
        }
        Kind::WebhookUrl => {
            // Empty turns notifications off. Anything else must be an address a POST can reach:
            // a typo would fail in the background, where nobody sees it.
            let usable = value.is_empty()
                || reqwest::Url::parse(value).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some_and(|host| !host.is_empty())
                });
            if !usable {
                return Err(refused("SettingRefusedAddress", &[]));
            }
        }
        Kind::Bool => {
            if !matches!(value.to_lowercase().as_str(), "true" | "false") {
                return Err(bad(format!("'{key}' must be 'true' or 'false'")));
            }
        }
        Kind::Bounded(min, max) | Kind::Retention(min, max) => {
            let n: i64 =
                value.parse().map_err(|_| bad(format!("'{key}' must be a whole number")))?;
            if !(min..=max).contains(&n) {
                return Err(bad(format!("'{key}' must be between {min} and {max}")));
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
        Kind::NotificationFormat => {
            if !crate::services::notify::FORMATS.contains(&value) {
                return Err(bad(format!(
                    "'{key}' must be one of {}",
                    crate::services::notify::FORMATS.join(", ")
                )));
            }
        }
        Kind::AnimeSearch => {
            if !crate::services::metadata::ANIME_SEARCH.contains(&value) {
                return Err(bad(format!(
                    "'{key}' must be one of {}",
                    crate::services::metadata::ANIME_SEARCH.join(", ")
                )));
            }
        }
        Kind::Onboarding => {
            if !ONBOARDING_STATES.contains(&value) {
                return Err(bad(format!(
                    "'{key}' must be one of {}",
                    ONBOARDING_STATES.join(", ")
                )));
            }
        }
        Kind::ProviderList => {
            // An ignored typo would pass for a source that never answers. `arr` is required: its
            // data comes with the sync at no cost.
            let ids: Vec<&str> =
                value.split(',').map(str::trim).filter(|v| !v.is_empty()).collect();
            for (at, id) in ids.iter().enumerate() {
                if crate::services::metadata::info(id).is_none() {
                    return Err(bad(format!("'{key}': '{id}' is not a known metadata source")));
                }
                if ids[..at].contains(id) {
                    return Err(bad(format!("'{key}': '{id}' is listed twice")));
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
                return Err(refused("SettingRefusedNoRegion", &[]));
            }
            if let Some(bad_code) =
                codes.iter().find(|c| c.len() != 2 || !c.chars().all(|ch| ch.is_ascii_alphabetic()))
            {
                return Err(refused("SettingRefusedRegion", &[("code", bad_code)]));
            }
            if let Some(unknown) =
                codes.iter().find(|code| !crate::integrations::language::is_country_code(code))
            {
                return Err(refused("SettingRefusedRegionUnknown", &[("code", unknown)]));
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
        assert!(validate("global_dry_run", "true", Kind::Bool, &[], &Localizer::new("en")).is_ok());
        assert!(validate("global_dry_run", "yes", Kind::Bool, &[], &Localizer::new("en")).is_err());
    }

    #[test]
    fn every_ranged_setting_takes_its_bounds_and_nothing_past_them() {
        let english = Localizer::new("en");
        let ranged = ranged_keys();
        assert!(ranged.len() >= 10, "{ranged:?}");
        for (key, min, max, _) in ranged {
            let checked = |value: String| check(key, &value, &[], &english);
            assert!(checked(min.to_string()).is_ok(), "{key} refuses its floor {min}");
            assert!(checked(max.to_string()).is_ok(), "{key} refuses its ceiling {max}");
            assert!(checked((min - 1).to_string()).is_err(), "{key} takes {}", min - 1);
            assert!(checked((max + 1).to_string()).is_err(), "{key} takes {}", max + 1);
            assert!(checked("many".into()).is_err(), "{key} takes a word");
        }
    }

    #[test]
    fn every_setting_has_one_default_that_its_own_validation_accepts() {
        for (key, kind, default) in KNOWN {
            if default.is_empty() {
                continue;
            }
            let categories = [default.to_string()];
            let checked = validate(key, default, *kind, &categories, &Localizer::new("en"));
            assert!(checked.is_ok(), "{key}'s default {default:?}: {checked:?}");
        }
        assert_eq!(default_of("an unknown key"), "");
    }

    #[test]
    fn default_category_must_exist() {
        let categories = vec!["standard".to_string()];
        assert!(
            validate(
                "default_category",
                "standard",
                Kind::Category,
                &categories,
                &Localizer::new("en")
            )
            .is_ok()
        );
        assert!(
            validate(
                "default_category",
                "kids",
                Kind::Category,
                &categories,
                &Localizer::new("en")
            )
            .is_err()
        );
    }

    #[test]
    fn the_theme_is_one_of_three_states() {
        assert!(validate("ui_theme", "dark", Kind::Theme, &[], &Localizer::new("en")).is_ok());
        assert!(validate("ui_theme", "light", Kind::Theme, &[], &Localizer::new("en")).is_ok());
        assert!(validate("ui_theme", "auto", Kind::Theme, &[], &Localizer::new("en")).is_ok());
        assert!(validate("ui_theme", "midnight", Kind::Theme, &[], &Localizer::new("en")).is_err());
        assert!(validate("ui_theme", "", Kind::Theme, &[], &Localizer::new("en")).is_err());
    }

    #[test]
    fn the_source_list_accepts_known_ids_in_any_order() {
        assert!(
            validate(
                "metadata_providers",
                "tmdb,arr",
                Kind::ProviderList,
                &[],
                &Localizer::new("en")
            )
            .is_ok()
        );
        assert!(
            validate("metadata_providers", "arr", Kind::ProviderList, &[], &Localizer::new("en"))
                .is_ok()
        );
    }

    #[test]
    fn a_misspelled_source_is_refused_rather_than_ignored() {
        assert!(
            validate(
                "metadata_providers",
                "arr,tmbd",
                Kind::ProviderList,
                &[],
                &Localizer::new("en")
            )
            .is_err()
        );
    }

    /// The interface draws one row per source, keyed by its id.
    #[test]
    fn a_source_listed_twice_is_refused() {
        assert!(
            validate(
                "metadata_providers",
                "arr,tmdb,tmdb",
                Kind::ProviderList,
                &[],
                &Localizer::new("en")
            )
            .is_err()
        );
        assert!(
            validate(
                "metadata_providers",
                "arr, arr",
                Kind::ProviderList,
                &[],
                &Localizer::new("en")
            )
            .is_err()
        );
    }

    #[test]
    fn country_list_is_checked() {
        assert!(
            validate(
                "certification_regions",
                "FR, US",
                Kind::CountryList,
                &[],
                &Localizer::new("en")
            )
            .is_ok()
        );
        assert!(
            validate("certification_regions", "FRA", Kind::CountryList, &[], &Localizer::new("en"))
                .is_err()
        );
        assert!(
            validate("certification_regions", "", Kind::CountryList, &[], &Localizer::new("en"))
                .is_err()
        );
        // Two letters ISO 3166-1 assigns to no country match no source's rating.
        for unknown in ["XX", "EU", "FR, ZZ"] {
            let refused = validate(
                "certification_regions",
                unknown,
                Kind::CountryList,
                &[],
                &Localizer::new("en"),
            );
            assert!(refused.is_err(), "{unknown} was taken");
        }
        let retired =
            validate("certification_regions", "SU", Kind::CountryList, &[], &Localizer::new("en"));
        assert!(retired.is_ok(), "a country gone keeps the code its films are filed under");
    }
}
