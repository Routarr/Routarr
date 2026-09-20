//! Translation of user-facing strings.
//!
//! Follows the Servarr convention: the dictionaries live in JSON files shipped
//! with the binary, the backend serves the one matching the configured UI
//! language, and any missing key falls back to English rather than showing a raw
//! identifier. The language is an application setting, not a browser preference,
//! so the whole install speaks one language.

use crate::services::rule_engine::ConditionOutcome;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A language Routarr ships translations for.
#[derive(Debug, Clone, Serialize)]
pub struct Language {
    /// IETF tag used as the setting value.
    pub code: &'static str,
    /// Name written in that language, as language pickers conventionally do.
    pub name: &'static str,
}

pub const DEFAULT_LANGUAGE: &str = "en";

/// Languages written right to left.
///
/// Held here rather than in the frontend because the catalogue is here: adding
/// `ar` to `CATALOG` should turn the interface around on its own, with nothing
/// else to remember. Base tags only — a regional variant carries its script's
/// direction, so `ar-EG` matches `ar`.
const RTL_LANGUAGES: &[&str] = &["ar", "he", "fa", "ur", "yi", "dv", "ps"];

/// Which way a language is written.
///
/// Anything unknown is left to right: guessing wrong mirrors an entire
/// interface, and the default is right for all but a handful of scripts.
pub fn direction(code: &str) -> &'static str {
    if RTL_LANGUAGES.contains(&base_tag(code).as_str()) { "rtl" } else { "ltr" }
}

/// The byte-unit symbols a language uses, where they are not the SI default.
///
/// Locale data, so it sits beside `RTL_LANGUAGES` rather than in the
/// dictionaries: these are symbols, not prose, and a translator has nothing to
/// decide about them. Generated from `Intl.NumberFormat`, which is the same
/// source `frontend/src/api/format.ts` reads at run time — that is what stops
/// the two halves of the application naming one figure two ways.
///
/// Japanese differs from the default by one capital, and Arabic's petabyte is
/// the one entry `Intl` gives as a word rather than a symbol; inventing a
/// prefix for it would be worse than repeating what the platform says, and no
/// homelab reports a petabyte of free space.
const BYTE_UNITS_DEFAULT: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];
const BYTE_UNITS: &[(&str, [&str; 6])] = &[
    ("ar", ["ب", "ك.ب", "م.ب", "غ.ب", "ت.ب", "بيتابايت"]),
    ("fi", ["t", "kt", "Mt", "Gt", "Tt", "Pt"]),
    ("fr", ["o", "ko", "Mo", "Go", "To", "Po"]),
    ("ja", ["B", "KB", "MB", "GB", "TB", "PB"]),
    ("ru", ["Б", "кБ", "МБ", "ГБ", "ТБ", "ПБ"]),
    ("uk", ["Б", "кБ", "МБ", "ГБ", "ТБ", "ПБ"]),
];

/// Languages that write a fraction with a comma.
///
/// The majority of the shipped set, but the *default* is the point: a language
/// added later gets a full stop, which is wrong for it far less often than
/// silently printing `2.2` where the interface beside it prints `2,2`.
const DECIMAL_COMMA: &[&str] = &[
    "ca", "cs", "da", "de", "el", "es", "fi", "fr", "hu", "it", "nb", "nl", "pl", "pt", "ro", "ru",
    "sk", "sv", "tr", "uk",
];

/// The byte-unit symbols for a language, smallest first.
pub fn byte_units(code: &str) -> [&'static str; 6] {
    BYTE_UNITS
        .iter()
        .find(|(language, _)| *language == base_tag(code))
        .map(|(_, units)| *units)
        .unwrap_or(BYTE_UNITS_DEFAULT)
}

/// The character a language puts between a whole number and its fraction.
pub fn decimal_separator(code: &str) -> char {
    if DECIMAL_COMMA.contains(&base_tag(code).as_str()) { ',' } else { '.' }
}

/// A language code reduced to the tag these tables are keyed by.
fn base_tag(code: &str) -> String {
    code.split(['-', '_']).next().unwrap_or(code).to_lowercase()
}

/// Languages shipped with this build, in the order the picker shows them.
///
/// Each entry needs a matching `locales/<code>.json`; `scripts/check-locales.py`
/// enforces that the file exists, carries no unknown key and keeps every
/// `{placeholder}`. Untranslated keys fall back to English at runtime, so a
/// language may ship before it is complete.
const CATALOG: &[(Language, &str)] = &[
    (Language { code: "en", name: "English" }, include_str!("../locales/en.json")),
    (Language { code: "de", name: "Deutsch" }, include_str!("../locales/de.json")),
    (Language { code: "es", name: "Español" }, include_str!("../locales/es.json")),
    (Language { code: "fr", name: "Français" }, include_str!("../locales/fr.json")),
    (Language { code: "it", name: "Italiano" }, include_str!("../locales/it.json")),
    (Language { code: "nl", name: "Nederlands" }, include_str!("../locales/nl.json")),
    (Language { code: "pt", name: "Português" }, include_str!("../locales/pt.json")),
    (Language { code: "ca", name: "Català" }, include_str!("../locales/ca.json")),
    (Language { code: "cs", name: "Čeština" }, include_str!("../locales/cs.json")),
    (Language { code: "da", name: "Dansk" }, include_str!("../locales/da.json")),
    (Language { code: "el", name: "Ελληνικά" }, include_str!("../locales/el.json")),
    (Language { code: "fi", name: "Suomi" }, include_str!("../locales/fi.json")),
    (Language { code: "hu", name: "Magyar" }, include_str!("../locales/hu.json")),
    (Language { code: "nb_NO", name: "Norsk bokmål" }, include_str!("../locales/nb_NO.json")),
    (Language { code: "pl", name: "Polski" }, include_str!("../locales/pl.json")),
    (Language { code: "ro", name: "Română" }, include_str!("../locales/ro.json")),
    (Language { code: "sk", name: "Slovenčina" }, include_str!("../locales/sk.json")),
    (Language { code: "sv", name: "Svenska" }, include_str!("../locales/sv.json")),
    (Language { code: "tr", name: "Türkçe" }, include_str!("../locales/tr.json")),
    (Language { code: "ru", name: "Русский" }, include_str!("../locales/ru.json")),
    (Language { code: "uk", name: "Українська" }, include_str!("../locales/uk.json")),
    (Language { code: "ja", name: "日本語" }, include_str!("../locales/ja.json")),
    (Language { code: "ko", name: "한국어" }, include_str!("../locales/ko.json")),
    (Language { code: "zh_CN", name: "简体中文" }, include_str!("../locales/zh_CN.json")),
    (Language { code: "zh_TW", name: "繁體中文" }, include_str!("../locales/zh_TW.json")),
    (Language { code: "ar", name: "العربية" }, include_str!("../locales/ar.json")),
];

type Dictionary = HashMap<String, String>;

/// Parsed dictionaries, built once on first use.
fn dictionaries() -> &'static HashMap<&'static str, Dictionary> {
    static CACHE: OnceLock<HashMap<&'static str, Dictionary>> = OnceLock::new();
    CACHE.get_or_init(|| {
        CATALOG
            .iter()
            .map(|(language, raw)| {
                let parsed: Dictionary = serde_json::from_str(raw).unwrap_or_else(|e| {
                    // A malformed shipped locale is a build mistake, not a
                    // runtime condition; degrade to keys rather than refusing to
                    // start.
                    tracing::error!("Locale {} is not valid JSON: {e}", language.code);
                    Dictionary::new()
                });
                (language.code, parsed)
            })
            .collect()
    })
}

/// A language as the picker needs to present it.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageInfo {
    pub code: &'static str,
    pub name: &'static str,
    /// `ltr` or `rtl`, so the interface can turn itself around without keeping
    /// its own list of scripts in step with this one.
    pub direction: &'static str,
    /// Share of English keys this language actually translates, 0–100.
    ///
    /// Shipped because a partial translation is allowed: rather than hiding an
    /// incomplete language or refusing it, the picker says how complete it is.
    /// Anything a language does not translate is served in English, so the
    /// interface stays usable either way — but the user gets to know.
    pub completion: u8,
}

/// Languages available in this build, with how complete each one is.
pub fn languages() -> Vec<LanguageInfo> {
    let english_keys = dictionaries().get(DEFAULT_LANGUAGE).map_or(0, |d| d.len());

    CATALOG
        .iter()
        .map(|(language, _)| {
            let translated = dictionaries().get(language.code).map_or(0, |d| d.len());
            LanguageInfo {
                code: language.code,
                name: language.name,
                direction: direction(language.code),
                // Rounded down: 99% must not display as 100%, or the picker
                // would claim a completeness the language does not have. With
                // no English keys at all there is nothing to be missing, which
                // is what the division by zero falls back to.
                completion: (translated * 100)
                    .checked_div(english_keys)
                    .map_or(100, |pct| pct.min(100) as u8),
            }
        })
        .collect()
}

/// True when `code` resolves to a shipped language, variants included.
pub fn is_supported(code: &str) -> bool {
    resolve(code).is_some()
}

/// The full dictionary for a language, merged over English so the frontend
/// never has to implement the fallback itself.
pub fn dictionary(code: &str) -> Dictionary {
    let english = dictionaries().get(DEFAULT_LANGUAGE).cloned().unwrap_or_default();
    if code.eq_ignore_ascii_case(DEFAULT_LANGUAGE) {
        return english;
    }

    merge(english, resolve(code).and_then(|code| dictionaries().get(code)))
}

/// English underneath, the translation on top.
///
/// Extracted so the partial-translation contract can be tested against a
/// deliberately incomplete dictionary: every shipped locale is complete today,
/// so a test written against them would prove nothing.
fn merge(english: Dictionary, translated: Option<&Dictionary>) -> Dictionary {
    let mut merged = english;
    if let Some(translated) = translated {
        merged.extend(translated.clone());
    }
    merged
}

/// The template for a key: the requested language first, English second.
///
/// This is what makes a partial translation safe rather than broken — see
/// [`merge`] for why it is a free function.
fn lookup<'a>(
    translated: Option<&'a Dictionary>,
    english: Option<&'a Dictionary>,
    key: &str,
) -> Option<&'a String> {
    translated.and_then(|d| d.get(key)).or_else(|| english.and_then(|d| d.get(key)))
}

/// Translates keys for one language.
#[derive(Debug, Clone)]
pub struct Localizer {
    language: String,
}

impl Localizer {
    pub fn new(language: &str) -> Self {
        Self { language: resolve(language).unwrap_or(DEFAULT_LANGUAGE).to_string() }
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    /// Look up `key`, substituting `{name}` placeholders from `params`.
    ///
    /// An unknown key returns the key itself: visibly wrong in the UI, which is
    /// what makes a missing translation get reported instead of silently
    /// rendering an empty string.
    pub fn translate(&self, key: &str, params: &[(&str, &str)]) -> String {
        let template = lookup(
            dictionaries().get(self.language.as_str()),
            dictionaries().get(DEFAULT_LANGUAGE),
            key,
        );

        let Some(template) = template else {
            tracing::debug!("Missing translation key: {key}");
            return key.to_string();
        };

        substitute(template, params)
    }
}

/// Replace `{placeholder}` occurrences.
fn substitute(template: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() || !template.contains('{') {
        return template.to_string();
    }

    let mut out = template.to_string();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Resolve a requested code to a shipped language.
///
/// A regional variant is honoured when it is shipped in its own right
/// (`pt-BR` differs from `pt` across the Servarr ecosystem) and otherwise falls
/// back to its base language, so `fr-CA` still gets French rather than English.
fn resolve(code: &str) -> Option<&'static str> {
    let requested = code.trim().to_lowercase().replace('_', "-");
    if let Some(exact) = shipped(&requested) {
        return Some(exact);
    }
    let base = requested.split('-').next().unwrap_or_default();
    shipped(base)
}

/// Match a normalized code against the catalogue.
///
/// Locale files keep the Servarr naming (`nb_NO`, `zh_CN`), so both sides are
/// normalized before comparison — otherwise `nb-no` would never find `nb_NO`.
fn shipped(code: &str) -> Option<&'static str> {
    CATALOG
        .iter()
        .map(|(language, _)| language.code)
        .find(|candidate| candidate.replace('_', "-").eq_ignore_ascii_case(code))
}

// ------------------------------------------------------------ domain rendering

impl Localizer {
    /// Fill in an outcome's wording for this language.
    pub fn localize_outcome(&self, mut outcome: ConditionOutcome) -> ConditionOutcome {
        let params: Vec<(&str, &str)> =
            outcome.params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        outcome.expected = self.translate(&outcome.key, &params);
        outcome
    }

    /// One explanation line: the expectation and what was actually observed.
    pub fn describe(&self, outcome: &ConditionOutcome) -> String {
        let localized = self.localize_outcome(outcome.clone());
        let observed = if localized.observed.is_empty() {
            self.translate("Unknown", &[])
        } else {
            localized.observed.clone()
        };

        self.translate(
            if outcome.matched { "ReasonMatched" } else { "ReasonNotMatched" },
            &[("expected", &localized.expected), ("observed", &observed)],
        )
    }

    /// The line shown when an exclusion vetoed an otherwise-matching rule.
    pub fn describe_exclusion(&self, outcome: &ConditionOutcome) -> String {
        let localized = self.localize_outcome(outcome.clone());
        self.translate("ReasonExcluded", &[("expected", &localized.expected)])
    }

    /// Every explanation line for one rule match, exclusion included.
    pub fn describe_all(
        &self,
        evaluations: &[ConditionOutcome],
        excluded_by: Option<&ConditionOutcome>,
    ) -> Vec<String> {
        let mut reasons: Vec<String> = evaluations.iter().map(|o| self.describe(o)).collect();
        if let Some(veto) = excluded_by {
            reasons.push(self.describe_exclusion(veto));
        }
        reasons
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_right_to_left_scripts_are_recognised() {
        for code in ["ar", "he", "fa", "ur"] {
            assert_eq!(direction(code), "rtl", "{code} is written right to left");
        }
    }

    #[test]
    fn a_regional_variant_follows_its_script() {
        // `ar-EG` is still Arabic; matching the whole tag would have missed it.
        assert_eq!(direction("ar-EG"), "rtl");
        assert_eq!(direction("pt_BR"), "ltr");
    }

    #[test]
    fn the_catalogue_reports_each_language_its_own_direction() {
        // The invariant is that the direction served is the one `direction()`
        // derives from the tag, never a hand-kept list in the frontend — which
        // is what lets a right-to-left language be added to `CATALOG` alone.
        for language in languages() {
            assert_eq!(
                language.direction,
                direction(language.code),
                "{} is served a direction that does not match its tag",
                language.code
            );
        }
        let arabic = languages()
            .into_iter()
            .find(|language| language.code == "ar")
            .expect("Arabic is in the catalogue");
        assert_eq!(arabic.direction, "rtl");
    }

    #[test]
    fn an_unknown_language_is_not_guessed_at() {
        // Mirroring a whole interface on a guess is worse than not mirroring.
        assert_eq!(direction("zz"), "ltr");
        assert_eq!(direction(""), "ltr");
    }

    use super::*;

    #[test]
    fn translates_a_known_key() {
        let fr = Localizer::new("fr");
        assert_eq!(
            fr.translate("CategoryNotFound", &[("name", "anime")]),
            "La catégorie « anime » n'existe pas"
        );
    }

    #[test]
    fn a_partial_translation_falls_back_to_english_key_by_key() {
        // Built by hand rather than taken from `locales/`: every shipped
        // dictionary is complete, so a test written against them would pass
        // whether or not the fallback exists.
        let english = Dictionary::from([
            ("Translated".to_string(), "English one".to_string()),
            ("Untranslated".to_string(), "English two".to_string()),
        ]);
        let partial = Dictionary::from([("Translated".to_string(), "Traduit".to_string())]);

        assert_eq!(
            lookup(Some(&partial), Some(&english), "Translated").map(String::as_str),
            Some("Traduit")
        );
        assert_eq!(
            lookup(Some(&partial), Some(&english), "Untranslated").map(String::as_str),
            Some("English two"),
            "a key the translation lacks must come from English"
        );
        // A key nobody has surfaces as itself, visibly wrong rather than blank.
        assert_eq!(lookup(Some(&partial), Some(&english), "Nowhere"), None);
    }

    #[test]
    fn a_partial_dictionary_is_served_complete() {
        // The frontend gets one flat dictionary, so the same guarantee has to
        // hold there: an incomplete translation must never yield a blank label.
        let english = Dictionary::from([
            ("Translated".to_string(), "English one".to_string()),
            ("Untranslated".to_string(), "English two".to_string()),
        ]);
        let partial = Dictionary::from([("Translated".to_string(), "Traduit".to_string())]);

        let served = merge(english.clone(), Some(&partial));

        assert_eq!(served.len(), english.len(), "every key must be present");
        assert_eq!(served["Translated"], "Traduit");
        assert_eq!(served["Untranslated"], "English two");
    }

    #[test]
    fn an_unknown_key_returns_itself() {
        assert_eq!(Localizer::new("fr").translate("NoSuchKey", &[]), "NoSuchKey");
    }

    #[test]
    fn an_unknown_language_falls_back_to_english() {
        let localizer = Localizer::new("kl");
        assert_eq!(localizer.language(), "en");
    }

    #[test]
    fn regional_variants_fall_back_to_the_base_language() {
        assert_eq!(Localizer::new("fr-FR").language(), "fr");
        assert_eq!(Localizer::new("fr_CA").language(), "fr");
        assert_eq!(Localizer::new("FR").language(), "fr");
    }

    #[test]
    fn a_shipped_regional_variant_wins_over_its_base() {
        // pt-BR is its own language across the Servarr ecosystem, not a synonym.
        if shipped("pt-br").is_some() {
            assert_eq!(Localizer::new("pt-BR").language(), "pt-br");
            assert_eq!(Localizer::new("pt_br").language(), "pt-br");
        }
        if shipped("pt").is_some() {
            assert_eq!(Localizer::new("pt-PT").language(), "pt");
        }
    }

    #[test]
    fn substitutes_every_placeholder() {
        assert_eq!(substitute("{a} and {b} and {a}", &[("a", "1"), ("b", "2")]), "1 and 2 and 1");
    }

    #[test]
    fn leaves_unknown_placeholders_alone() {
        // Visible in the UI, so a missing parameter gets noticed and fixed.
        assert_eq!(substitute("{a} {missing}", &[("a", "1")]), "1 {missing}");
    }

    #[test]
    fn no_language_defines_a_key_english_does_not() {
        // Missing keys are allowed — they fall back to English, which is what
        // lets a translation land incomplete. A key English does *not* have is
        // a different thing entirely: a typo, or one left behind when the
        // English string was renamed. It can never be reached, so it is a bug.
        let english = dictionaries().get("en").expect("english dictionary");

        for (language, _) in CATALOG {
            if language.code == "en" {
                continue;
            }
            let translated = dictionaries().get(language.code).expect("dictionary");

            let extra: Vec<&String> =
                translated.keys().filter(|key| !english.contains_key(*key)).collect();
            assert!(extra.is_empty(), "{} has keys English does not: {extra:?}", language.code);
        }
    }

    #[test]
    fn placeholders_match_across_languages() {
        // A translation that drops `{name}` would render a message missing the
        // very value it is about.
        let english = dictionaries().get("en").expect("english dictionary");

        for (language, _) in CATALOG {
            if language.code == "en" {
                continue;
            }
            let translated = dictionaries().get(language.code).expect("dictionary");

            for (key, source) in english {
                let Some(target) = translated.get(key) else { continue };
                let mut expected = placeholders(source);
                let mut actual = placeholders(target);
                expected.sort();
                actual.sort();
                assert_eq!(
                    expected, actual,
                    "{}: placeholders differ for key '{key}'",
                    language.code
                );
            }
        }
    }

    #[test]
    fn the_merged_dictionary_covers_every_english_key() {
        let english = dictionaries().get("en").expect("english dictionary");
        let merged = dictionary("fr");
        assert_eq!(merged.len(), english.len());
    }

    fn placeholders(template: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}') else { break };
            found.push(rest[start + 1..start + end].to_string());
            rest = &rest[start + end + 1..];
        }
        found
    }

    /// Every job kind the registry can write must have a label.
    ///
    /// `t()` falls back to the raw key, so a kind nobody translated renders as
    /// `JobBackup` on the Tasks screen — and backups are on by default, so it
    /// was on every installation. `check-locales.py` cannot catch it: the
    /// `Job*` prefix is exempt from its orphan check, which necessarily makes
    /// it blind to a missing one too. The list is the enum, not a copy of it.
    #[test]
    fn every_job_kind_has_a_label() {
        use crate::jobs::JobKind::*;

        let english: serde_json::Value =
            serde_json::from_str(include_str!("../locales/en.json")).expect("en.json");

        for kind in [Sync, Enrich, Simulate, Apply, Revert, Maintenance, Backup, Scheduler] {
            let name = kind.as_str();
            let key = format!("Job{}{}", name[..1].to_uppercase(), &name[1..]);
            assert!(english.get(&key).is_some(), "{key} is missing from en.json");
        }
    }
}
