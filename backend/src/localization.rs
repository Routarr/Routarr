//! Translation of user-facing strings, as the Servarr apps do it: dictionaries shipped in
//! the binary, one language for the whole install, and English for any key a language lacks.

use crate::services::rule_engine::ConditionOutcome;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
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

/// Languages written right to left, by base tag, so `ar-EG` follows `ar`. Kept beside
/// `CATALOG`, so a right-to-left language added there turns the interface around alone.
const RTL_LANGUAGES: &[&str] = &["ar", "he", "fa", "ur", "yi", "dv", "ps"];

/// Which way a language is written. Anything unknown is left to right, since a wrong
/// guess mirrors the whole interface.
pub fn direction(code: &str) -> &'static str {
    if RTL_LANGUAGES.contains(&base_tag(code).as_str()) { "rtl" } else { "ltr" }
}

/// The byte-unit symbols a language uses, where they are not the SI default, taken from
/// the `Intl.NumberFormat` that `frontend/src/api/format.ts` reads, so both halves name one
/// figure alike. Arabic's petabyte is the one word `Intl` gives, and stays as it gives it.
const BYTE_UNITS_DEFAULT: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];
const BYTE_UNITS: &[(&str, [&str; 6])] = &[
    ("ar", ["ب", "ك.ب", "م.ب", "غ.ب", "ت.ب", "بيتابايت"]),
    ("fi", ["t", "kt", "Mt", "Gt", "Tt", "Pt"]),
    ("fr", ["o", "ko", "Mo", "Go", "To", "Po"]),
    ("ja", ["B", "KB", "MB", "GB", "TB", "PB"]),
    ("ru", ["Б", "кБ", "МБ", "ГБ", "ТБ", "ПБ"]),
    ("uk", ["Б", "кБ", "МБ", "ГБ", "ТБ", "ПБ"]),
];

/// Languages that write a fraction with a comma. Any other, a language added later
/// included, gets a full stop.
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

/// How long ago `then` was, in its largest whole unit and the reader's words, `3 h` or
/// `2 d`, since the age is what a reader weighs, never a UTC time.
pub fn human_age(then: chrono::DateTime<chrono::Utc>, localizer: &Localizer) -> String {
    age_of((chrono::Utc::now() - then).num_minutes(), localizer)
}

/// An age of `minutes` in its largest whole unit, as [`human_age`] writes it.
pub fn age_of(minutes: i64, localizer: &Localizer) -> String {
    let minutes = minutes.max(1);
    let (key, count) = match minutes {
        ..60 => ("AgeMinutes", minutes),
        60..2880 => ("AgeHours", minutes / 60),
        _ => ("AgeDays", minutes / 1440),
    };
    localizer.translate(key, &[("count", &count.to_string())])
}

/// How a language writes a date, as the interface's short date (`Intl.DateTimeFormat`) writes
/// it, in chrono's terms. Any other language writes it as English. `date_cases.json` holds
/// both sides to it.
const DATES: &[(&str, &str)] = &[
    ("ar", "%-d\u{200f}/%-m\u{200f}/%Y"),
    ("ca", "%-d/%-m/%y"),
    ("cs", "%d.%m.%y"),
    ("da", "%d.%m.%Y"),
    ("de", "%d.%m.%y"),
    ("el", "%-d/%-m/%y"),
    ("es", "%-d/%-m/%y"),
    ("fi", "%-d.%-m.%Y"),
    ("fr", "%d/%m/%Y"),
    ("hu", "%Y. %m. %d."),
    ("it", "%d/%m/%y"),
    ("ja", "%Y/%m/%d"),
    ("ko", "%y. %-m. %-d."),
    ("nb", "%d.%m.%Y"),
    ("nl", "%d-%m-%Y"),
    ("pl", "%-d.%m.%Y"),
    ("pt", "%d/%m/%Y"),
    ("ro", "%d.%m.%Y"),
    ("ru", "%d.%m.%Y"),
    ("sk", "%-d. %-m. %Y"),
    ("sv", "%Y-%m-%d"),
    ("tr", "%-d.%m.%Y"),
    ("uk", "%d.%m.%y"),
    ("zh", "%Y/%-m/%-d"),
];

/// `date` as `code` writes a date.
pub fn written_date(date: chrono::NaiveDate, code: &str) -> String {
    let tag = base_tag(code);
    let pattern =
        DATES.iter().find(|(language, _)| *language == tag).map_or("%-m/%-d/%y", |(_, p)| *p);
    date.format(pattern).to_string()
}

/// The day of `at` where Routarr runs (`TZ`, as the log reads it), as the reader's language
/// writes a date.
pub fn human_date(at: chrono::DateTime<chrono::Utc>, localizer: &Localizer) -> String {
    written_date(at.with_timezone(&chrono::Local).date_naive(), localizer.language())
}

/// Bytes as an operator reads them, in binary steps as a file manager shows them, with the
/// language's symbol and decimal mark. `frontend/src/api/format.ts` writes them alike.
pub fn human_bytes(bytes: i64, localizer: &Localizer) -> String {
    let units = byte_units(localizer.language());
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let space = if UNIT_AGAINST_ITS_NUMBER.contains(&base_tag(localizer.language()).as_str()) {
        ""
    } else {
        " "
    };
    if unit == 0 {
        return format!("{}{space}{}", bytes.max(0), units[0]);
    }
    let separator = decimal_separator(localizer.language());
    let figure = format!("{value:.1}").replace('.', &separator.to_string());
    format!("{figure}{space}{}", units[unit])
}

/// The languages that set a unit against its number, as `Intl` writes a size
/// in the interface: `512B` in Korean.
const UNIT_AGAINST_ITS_NUMBER: &[&str] = &["ko"];

/// The placeholders holding a count, its digits grouped as the language groups them
/// (`12 345` in French), which a year would not survive. `scripts/check-locales.py` refuses a
/// placeholder in neither this list nor its own. The interface reads it from `/localization`.
pub const COUNTS: &[&str] = &[
    "afterMoves",
    "afterUnmatched",
    "answered",
    "applied",
    "archives",
    "batches",
    "beforeMoves",
    "beforeUnmatched",
    "candidates",
    "cap",
    "categories",
    "changed",
    "copies",
    "count",
    "decisions",
    "done",
    "enriched",
    "failed",
    "folders",
    "instances",
    "jobs",
    "left",
    "limit",
    "logs",
    "media",
    "moves",
    "moving",
    "overrides",
    "page",
    "planned",
    "rate",
    "requested",
    "reverted",
    "rules",
    "run",
    "seconds",
    "settings",
    "shown",
    "skipped",
    "synced",
    "tests",
    "threshold",
    "total",
    "votes",
];

/// How a language groups a whole number's digits: the separator, and the fewest digits a
/// number needs to be grouped (`1234` but `12.345` in Spanish), as `Intl.NumberFormat` does.
/// `grouped_count_cases.json` holds both sides to it.
const GROUPING: &[(&str, char, usize)] = &[
    ("ca", '.', 4),
    ("cs", '\u{a0}', 4),
    ("da", '.', 4),
    ("de", '.', 4),
    ("el", '.', 4),
    ("es", '.', 5),
    ("fi", '\u{a0}', 4),
    ("fr", '\u{202f}', 4),
    ("hu", '\u{a0}', 5),
    ("it", '.', 5),
    ("nb", '\u{a0}', 4),
    ("nl", '.', 4),
    ("pl", '\u{a0}', 5),
    ("pt", '.', 4),
    ("ro", '.', 4),
    ("ru", '\u{a0}', 4),
    ("sk", '\u{a0}', 4),
    ("sv", '\u{a0}', 4),
    ("tr", '.', 4),
    ("uk", '\u{a0}', 4),
];

/// A whole number with its digits grouped as `code` groups them, or `None`
/// when `value` is not one.
pub fn group_digits(code: &str, value: &str) -> Option<String> {
    let (sign, digits) = value.strip_prefix('-').map_or(("", value), |rest| ("-", rest));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let tag = base_tag(code);
    let (separator, shortest) = GROUPING
        .iter()
        .find(|(language, ..)| *language == tag)
        .map_or((',', 4), |(_, separator, shortest)| (*separator, *shortest));
    if digits.len() < shortest {
        return Some(value.to_string());
    }
    let mut grouped = String::from(sign);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            grouped.push(separator);
        }
        grouped.push(digit);
    }
    Some(grouped)
}

/// A language code reduced to the tag these tables are keyed by.
fn base_tag(code: &str) -> String {
    code.split(['-', '_']).next().unwrap_or(code).to_lowercase()
}

/// Languages shipped with this build, in the order the picker shows them. A missing file
/// fails the build, and an untranslated key falls back to English at run time.
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

/// A language that translates two keys and leaves every other to English, for the tests of
/// the fallback, since every shipped language is complete. Never offered by `languages()`.
#[cfg(test)]
const PARTIAL: (Language, &str) = (
    Language { code: "x-partial", name: "Partial" },
    r#"{ "Dashboard": "Tableau partiel", "CategoryNameEmpty": "Nom partiel requis." }"#,
);

/// The catalogue, and under test the partial language beside it.
fn catalogue() -> impl Iterator<Item = &'static (Language, &'static str)> {
    let partial: &[(Language, &str)] = {
        #[cfg(test)]
        {
            std::slice::from_ref(&PARTIAL)
        }
        #[cfg(not(test))]
        {
            &[]
        }
    };
    CATALOG.iter().chain(partial)
}

/// Parsed dictionaries, built once on first use.
fn dictionaries() -> &'static HashMap<&'static str, Dictionary> {
    static CACHE: OnceLock<HashMap<&'static str, Dictionary>> = OnceLock::new();
    CACHE.get_or_init(|| {
        catalogue()
            .map(|(language, raw)| {
                let parsed: Dictionary = serde_json::from_str(raw).unwrap_or_else(|e| {
                    // A build mistake: degrade to keys instead of refusing to start.
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
    /// Share of English keys this language translates, 0 to 100, so the picker says how
    /// complete a language is. Its missing keys are served in English.
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
                // Rounded down, so 99% never reads 100%. With no English key, nothing is missing.
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

/// English underneath, the translation on top, apart so a test can hand it an incomplete
/// dictionary, since every shipped one is complete.
fn merge(english: Dictionary, translated: Option<&Dictionary>) -> Dictionary {
    let mut merged = english;
    if let Some(translated) = translated {
        merged.extend(translated.clone());
    }
    merged
}

/// The template for a key: the requested language first, English second.
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

    /// Look up `key`, substituting `{name}` placeholders from `params`. An unknown key
    /// returns itself, visibly wrong, so a missing translation gets reported.
    pub fn translate(&self, key: &str, params: &[(&str, &str)]) -> String {
        // Right to left, each value but a count is set in a first strong isolate, or it reorders.
        self.substituted(key, params, direction(&self.language) == "rtl")
    }

    /// A text a row keeps, as a rule's name or a category's description: substituted as
    /// `translate` does, with no isolate, since an invisible mark would make two names of one.
    pub fn stored(&self, key: &str, params: &[(&str, &str)]) -> String {
        self.substituted(key, params, false)
    }

    fn substituted(&self, key: &str, params: &[(&str, &str)], isolating: bool) -> String {
        let template = lookup(
            dictionaries().get(self.language.as_str()),
            dictionaries().get(DEFAULT_LANGUAGE),
            key,
        );

        let Some(template) = template else {
            tracing::debug!("Missing translation key: {key}");
            return key.to_string();
        };

        let grouped: Vec<(&str, String)> = params
            .iter()
            .map(|(name, value)| {
                let count = COUNTS.contains(name).then(|| group_digits(&self.language, value));
                match count.flatten() {
                    Some(grouped) => (*name, grouped),
                    None if isolating => (*name, format!("\u{2068}{value}\u{2069}")),
                    None => (*name, value.to_string()),
                }
            })
            .collect();
        let params: Vec<(&str, &str)> =
            grouped.iter().map(|(name, value)| (*name, value.as_str())).collect();
        substitute(template, &params)
    }
}

/// Replace `{placeholder}` occurrences, in one pass over the template: a value
/// is never read again, so a `{name}` inside it stays as written.
fn substitute(template: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() || !template.contains('{') {
        return template.to_string();
    }

    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let found = after.find('}').and_then(|close| {
            let name = &after[..close];
            params.iter().find(|(known, _)| *known == name).map(|(_, value)| (*value, close))
        });
        match found {
            Some((value, close)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Resolve a requested code to a shipped language: a variant shipped in its own right
/// whole, any other by its base language, so `fr-CA` gets French, and a base language by the
/// one variant shipped for it, so `nb` gets `nb_NO` (RFC 4647 lookup).
fn resolve(code: &str) -> Option<&'static str> {
    let requested = code.trim().to_lowercase().replace('_', "-");
    if let Some(exact) = shipped(&requested) {
        return Some(exact);
    }
    if let Some((_, alias)) = ALIASES.iter().find(|(alias, _)| *alias == requested) {
        return shipped(alias);
    }
    let base = requested.split('-').next().unwrap_or_default();
    shipped(base).or_else(|| {
        let mut variants = catalogue()
            .map(|(language, _)| language.code)
            .filter(|candidate| base_tag(candidate) == base);
        variants.next().filter(|_| variants.next().is_none())
    })
}

/// Tags naming a shipped language another way: Norwegian, and Chinese by its script, whose
/// two variants share a base tag.
const ALIASES: &[(&str, &str)] =
    &[("no", "nb-no"), ("zh", "zh-cn"), ("zh-hans", "zh-cn"), ("zh-hant", "zh-tw")];

/// Match a normalized code against the catalogue, whose Servarr names (`nb_NO`) are
/// normalized too, or `nb-no` would never find `nb_NO`.
fn shipped(code: &str) -> Option<&'static str> {
    catalogue()
        .map(|(language, _)| language.code)
        .find(|candidate| candidate.replace('_', "-").eq_ignore_ascii_case(code))
}

/// A sentence as a row stores it: a dictionary key and its values, said in
/// the language of whoever reads the row. A sentence written in the language
/// of the moment it was stored would stay in that language for ever.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Told {
    pub key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Told {
    pub fn new(key: &str) -> Self {
        Self { key: key.to_string(), params: BTreeMap::new() }
    }

    pub fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }

    pub fn render(&self, localizer: &Localizer) -> String {
        localizer.translate_map(&self.key, &self.params)
    }

    /// What a column stores, which [`said`] reads back.
    pub fn stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// A stored text in `localizer`'s language: a [`Told`] rendered, or a
/// sentence a row holds as it was written, which is read as it is.
pub fn said(stored: &str, localizer: &Localizer) -> String {
    match serde_json::from_str::<Told>(stored) {
        Ok(told) => told.render(localizer),
        Err(_) => stored.to_string(),
    }
}

impl Localizer {
    /// `translate`, with the values as the engine and the job registry store
    /// them.
    pub fn translate_map(&self, key: &str, params: &BTreeMap<String, String>) -> String {
        let pairs: Vec<(&str, &str)> =
            params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.translate(key, &pairs)
    }

    /// Fill in an outcome's wording for this language: what the condition
    /// asked for, and what the title carried, each as this language writes
    /// it. The engine leaves both raw.
    pub fn localize_outcome(&self, mut outcome: ConditionOutcome) -> ConditionOutcome {
        let kind = outcome.kind.clone();
        let params: BTreeMap<String, String> = outcome
            .params
            .iter()
            .map(|(name, value)| (name.clone(), self.engine_value(&kind, value)))
            .collect();
        outcome.expected = self.translate_map(&outcome.key, &params);
        outcome.observed = self.engine_value(&kind, &outcome.observed);
        outcome
    }

    /// A value the engine reports raw, as this language writes it: a yes or
    /// a no for a switch, a size in this language's units, the day a title
    /// was added. Anything else, a genre or a path, is the title's own.
    fn engine_value(&self, kind: &str, value: &str) -> String {
        match (kind, value) {
            ("has_files" | "monitored" | "has_metadata" | "original_language_known", "true") => {
                self.translate("Yes", &[])
            }
            ("has_files" | "monitored" | "has_metadata" | "original_language_known", "false") => {
                self.translate("No", &[])
            }
            ("size_on_disk_over_gb", bytes) if !bytes.is_empty() => {
                bytes.parse().map_or_else(|_| bytes.to_string(), |n| human_bytes(n, self))
            }
            // The engine's `PT<minutes>M`, an older row's date added, or the condition's own days.
            ("added_within_days", value) => {
                let minutes = value.strip_prefix("PT").and_then(|rest| rest.strip_suffix('M'));
                match minutes.and_then(|minutes| minutes.parse().ok()) {
                    Some(minutes) => age_of(minutes, self),
                    None => crate::services::routing::parse_timestamp(value)
                        .map_or_else(|| value.to_string(), |added| human_age(added, self)),
                }
            }
            // `8,1 (393 000)` in French, the votes grouped. No scale name holds a full stop.
            ("score_range", found) => {
                let decimal = |score: &str| {
                    score.replace('.', &decimal_separator(self.language()).to_string())
                };
                match found.strip_suffix(')').and_then(|rest| rest.split_once(" (")) {
                    Some((score, votes)) => {
                        let votes = group_digits(self.language(), votes)
                            .unwrap_or_else(|| votes.to_string());
                        format!("{} ({votes})", decimal(score))
                    }
                    None => decimal(found),
                }
            }
            (_, other) => other.to_string(),
        }
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
    /// Minutes under an hour, hours under two days, days beyond: the largest
    /// unit that keeps the figure readable.
    #[test]
    fn an_age_is_said_in_its_largest_whole_unit() {
        let english = Localizer::new("en");
        let ago = |minutes: i64| {
            human_age(chrono::Utc::now() - chrono::Duration::minutes(minutes), &english)
        };

        assert_eq!(ago(0), "1 min");
        assert_eq!(ago(59), "59 min");
        assert_eq!(ago(185), "3 h");
        assert_eq!(ago(47 * 60), "47 h");
        assert_eq!(ago(3 * 1440 + 30), "3 d");
    }

    /// In an Arabic sentence a path's leading slash would move to its end. In
    /// an English one the marks would be invisible noise in a log line.
    #[test]
    fn a_path_keeps_its_own_direction_in_a_right_to_left_sentence_only() {
        let params = [("path", "/movies/anime")];

        let arabic = Localizer::new("ar").translate("DestinationDeclared", &params);
        assert!(arabic.contains("\u{2068}/movies/anime\u{2069}"), "{arabic}");

        let english = Localizer::new("en").translate("DestinationDeclared", &params);
        assert!(!english.contains('\u{2068}'), "{english}");
    }

    /// A text a row keeps is compared and shown in either direction later: no mark goes in it.
    #[test]
    fn a_text_to_store_is_substituted_without_an_isolate() {
        let arabic = Localizer::new("ar");
        let params = [("name", "Anime")];

        assert_eq!(arabic.stored("RuleCopyName", &params), "Anime (نسخة)");
        assert!(arabic.translate("RuleCopyName", &params).contains("\u{2068}Anime\u{2069}"));
    }

    /// Left bare in Arabic, a name that ends in punctuation shows it at its start.
    #[test]
    fn every_value_but_a_count_keeps_its_own_direction_in_a_right_to_left_sentence() {
        let arabic = Localizer::new("ar");
        let name = arabic.translate("CategoryCreated", &[("name", "Who Framed Roger Rabbit?")]);
        assert!(name.contains("\u{2068}Who Framed Roger Rabbit?\u{2069}"), "{name}");

        let counted =
            arabic.translate("PurgeResult", &[("decisions", "3"), ("logs", "2"), ("jobs", "1")]);
        assert!(!counted.contains('\u{2068}'), "{counted}");
    }

    /// The interface groups a count through `Intl.NumberFormat`, the server
    /// through `group_digits`: one table of cases holds both to the same
    /// writing, the frontend's in `i18n.test.ts`.
    #[test]
    fn a_count_is_grouped_as_the_interface_groups_it() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("grouped_count_cases.json")).unwrap();
        let numbers = cases["numbers"].as_array().unwrap();
        let languages = cases["grouped"].as_object().unwrap();
        assert_eq!(languages.len(), CATALOG.len(), "a language has no case");
        for (language, grouped) in languages {
            for (number, expected) in numbers.iter().zip(grouped.as_array().unwrap()) {
                assert_eq!(
                    group_digits(language, &number.to_string()).as_deref(),
                    expected.as_str(),
                    "{language}"
                );
            }
        }
    }

    #[test]
    fn a_count_placeholder_is_grouped_and_any_other_is_left_as_written() {
        let french = Localizer::new("fr");
        let applied = french.translate(
            "ApplyResult",
            &[("applied", "12345"), ("requested", "12345"), ("skipped", "0")],
        );
        assert!(applied.contains("12\u{202f}345"), "{applied}");
        let years = french.translate("ConditionYearRange", &[("min", "1990"), ("max", "2026")]);
        assert!(years.contains("1990") && years.contains("2026"), "{years}");
        assert_eq!(group_digits("fr", "Radarr"), None);
    }

    #[test]
    fn the_right_to_left_scripts_are_recognised() {
        for code in ["ar", "he", "fa", "ur"] {
            assert_eq!(direction(code), "rtl", "{code} is written right to left");
        }
    }

    #[test]
    fn a_regional_variant_follows_its_script() {
        // `ar-EG` is still Arabic, and matching the whole tag would miss it.
        assert_eq!(direction("ar-EG"), "rtl");
        assert_eq!(direction("pt_BR"), "ltr");
    }

    #[test]
    fn the_catalogue_reports_each_language_its_own_direction() {
        // The direction served is the one `direction()` derives, never a list in the frontend.
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
        assert_eq!(direction("zz"), "ltr");
        assert_eq!(direction(""), "ltr");
    }

    use super::*;

    #[test]
    fn translates_a_known_key() {
        let fr = Localizer::new("fr");
        assert_eq!(
            fr.translate("CategoryNotFound", &[("name", "anime")]),
            "La catégorie «\u{a0}anime\u{a0}» n'existe pas"
        );
    }

    #[test]
    fn a_partial_translation_falls_back_to_english_key_by_key() {
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
        assert_eq!(lookup(Some(&partial), Some(&english), "Nowhere"), None);
    }

    #[test]
    fn a_partial_dictionary_is_served_complete() {
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

    /// A shipped regional variant is matched whole, in either spelling, and
    /// never collapses to its base or to a sibling: `zh-TW` is not `zh_CN`.
    #[test]
    fn a_shipped_regional_variant_is_matched_whole() {
        assert_eq!(Localizer::new("zh-TW").language(), "zh_TW");
        assert_eq!(Localizer::new("zh_tw").language(), "zh_TW");
        assert_eq!(Localizer::new("zh-CN").language(), "zh_CN");
        assert_eq!(Localizer::new("nb-NO").language(), "nb_NO");
        for (asked, shipped) in
            [("nb", "nb_NO"), ("no", "nb_NO"), ("zh-Hans", "zh_CN"), ("zh-Hant", "zh_TW")]
        {
            assert_eq!(Localizer::new(asked).language(), shipped, "{asked}");
        }
    }

    /// The interface writes a date through `Intl.DateTimeFormat`, the server through
    /// `written_date`: one table of cases holds both to the same writing, the frontend's in
    /// `format.test.ts`.
    #[test]
    fn a_date_is_written_as_the_language_writes_it() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("date_cases.json")).unwrap();
        let dates: Vec<chrono::NaiveDate> = cases["dates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| day.as_str().unwrap().parse().unwrap())
            .collect();
        let languages = cases["written"].as_object().unwrap();
        assert_eq!(languages.len(), CATALOG.len(), "a language has no case");
        for (language, written) in languages {
            for (date, expected) in dates.iter().zip(written.as_array().unwrap()) {
                assert_eq!(written_date(*date, language), expected.as_str().unwrap(), "{language}");
            }
        }
    }

    #[test]
    fn substitutes_every_placeholder() {
        assert_eq!(substitute("{a} and {b} and {a}", &[("a", "1"), ("b", "2")]), "1 and 2 and 1");
    }

    #[test]
    fn a_placeholder_inside_a_value_is_left_as_written() {
        assert_eq!(
            substitute("{name} holds {count}", &[("name", "Rule {count}"), ("count", "3")]),
            "Rule {count} holds 3"
        );
    }

    #[test]
    fn leaves_unknown_placeholders_alone() {
        // Visible in the UI, so a missing parameter gets noticed and fixed.
        assert_eq!(substitute("{a} {missing}", &[("a", "1")]), "1 {missing}");
    }
}
