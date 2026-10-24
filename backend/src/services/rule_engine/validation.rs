//! What makes a rule refused or flagged before it is stored.

use super::{certification_key, normalise_value, status_key};
use crate::models::{Condition, MatchMode, MetadataField, ValidationIssue};

/// The first value a list condition names twice, compared as matching compares
/// them: `science fiction` repeats `Science-Fiction`, `R+` does not repeat `R`.
/// Read through the stored shape, so a new list variant needs no arm here.
fn repeated_value(condition: &Condition) -> Option<String> {
    let stored = serde_json::to_value(condition).ok()?;
    let key = key_of(condition);
    let mut seen = std::collections::HashSet::new();
    stored
        .get("value")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map_or_else(|| value.to_string(), str::to_string))
        .find(|text| {
            let key = key(text);
            !key.is_empty() && !seen.insert(key)
        })
}

/// The rule being validated, independent of whether it exists yet.
#[derive(Debug, Clone, Copy)]
pub struct RuleDraft<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    /// `None` places the rule, and every place is within the bounds.
    pub priority: Option<i64>,
    pub media_type: &'a str,
    pub match_mode: MatchMode,
    pub conditions: &'a [Condition],
    pub exclusions: &'a [Condition],
    pub target_category: &'a str,
}

/// The surrounding configuration a rule is validated against.
#[derive(Debug, Clone, Copy)]
pub struct ValidationEnv<'a> {
    pub known_categories: &'a [String],
    /// Categories mapped onto a folder of an instance the rule can route: in
    /// its scope, and of the kind that holds its media type.
    pub mapped_categories: &'a [String],
    /// Metadata fields at least one enabled source can answer, by field, since the
    /// Arr alone answers genres and no keyword.
    pub covered_fields: &'a [MetadataField],
    /// The score scales at least one enabled source gives, which a score range reads.
    pub covered_scales: &'a [&'static str],
    /// The year the caller considers current, injected so a test can pin it.
    pub current_year: i64,
}

/// The year of the first surviving film, the floor for a year.
pub const MIN_YEAR: i64 = 1888;

/// How far ahead of the current year a rule may reach: Radarr lists films long
/// before release, and the ceiling catches a typing mistake such as `20255`.
pub const MAX_YEARS_AHEAD: i64 = 5;

/// The longest runtime a rule may name, in minutes: a day holds every film released.
pub const MAX_RUNTIME: i64 = 1440;

/// Longest name a rule may carry. A name is a table cell and a badge, and a
/// paragraph in either breaks the layout of every screen that lists rules.
pub const MAX_NAME_LENGTH: usize = 200;

/// Longest description a rule may carry: every rule list sends it.
pub const MAX_DESCRIPTION_LENGTH: usize = 1000;

/// Most conditions one list of a rule holds, its exclusions as many again: every
/// pass reads each one per title, and the contradiction check reads each pair.
pub const MAX_CONDITIONS: usize = 50;

/// Most values one condition holds.
pub const MAX_VALUES: usize = 200;

/// The lowest priority a rule may take, far beyond what `MAX_RULES` rules need at any step.
pub const MIN_PRIORITY: i64 = -1_000_000;

/// The highest priority a rule may take. A rule placed after the last stops here, so no sum
/// leaves the integers SQLite stores nor those a browser's number holds exactly.
pub const MAX_PRIORITY: i64 = 1_000_000;

/// Validate a rule before it is stored or enabled. Errors block the write, and
/// warnings do not.
pub fn validate_rule(draft: RuleDraft<'_>, env: ValidationEnv<'_>) -> Vec<ValidationIssue> {
    let RuleDraft {
        name,
        description,
        priority,
        media_type,
        match_mode,
        conditions,
        exclusions,
        target_category,
    } = draft;
    let ValidationEnv {
        known_categories,
        mapped_categories,
        covered_fields,
        covered_scales,
        current_year,
    } = env;

    let mut issues = Vec::new();

    // Refused before anything reads them one by one, let alone in pairs.
    for (section, list) in [("conditions", conditions), ("exclusions", exclusions)] {
        if list.len() > MAX_CONDITIONS {
            issues.push(ValidationIssue::error(
                section,
                "ValidationTooManyConditions",
                &[("max", MAX_CONDITIONS.to_string())],
            ));
        }
    }
    if !issues.is_empty() {
        return issues;
    }
    if description.is_some_and(|text| text.chars().count() > MAX_DESCRIPTION_LENGTH) {
        issues.push(ValidationIssue::error(
            "description",
            "ValidationDescriptionTooLong",
            &[("max", MAX_DESCRIPTION_LENGTH.to_string())],
        ));
    }

    if name.trim().is_empty() {
        issues.push(ValidationIssue::error("name", "ValidationNameEmpty", &[]));
    } else if name.chars().count() > MAX_NAME_LENGTH {
        issues.push(ValidationIssue::error(
            "name",
            "ValidationNameTooLong",
            &[("max", MAX_NAME_LENGTH.to_string())],
        ));
    }
    if priority.is_some_and(|priority| !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority)) {
        issues.push(ValidationIssue::error(
            "priority",
            "ValidationPriorityRange",
            &[("min", MIN_PRIORITY.to_string()), ("max", MAX_PRIORITY.to_string())],
        ));
    }
    if media_type.parse::<crate::models::RuleMediaType>().is_err() {
        issues.push(ValidationIssue::error("media_type", "ValidationMediaType", &[]));
    }
    if conditions.is_empty() {
        issues.push(ValidationIssue::error("conditions", "ValidationNoConditions", &[]));
    }
    if target_category.trim().is_empty() {
        issues.push(ValidationIssue::error("target_category", "ValidationCategoryEmpty", &[]));
    } else if !known_categories.iter().any(|c| c == target_category) {
        issues.push(ValidationIssue::error(
            "target_category",
            "CategoryNotFound",
            &[("name", target_category.to_string())],
        ));
    } else if !mapped_categories.iter().any(|c| c == target_category) {
        issues.push(ValidationIssue::warning(
            "target_category",
            "ValidationCategoryUnmapped",
            &[("name", target_category.to_string())],
        ));
    }

    // Each issue names its section, place and kind, which the handler turns into a caption.
    let placed =
        conditions.iter().enumerate().map(|(idx, condition)| ("conditions", idx, condition)).chain(
            exclusions.iter().enumerate().map(|(idx, condition)| ("exclusions", idx, condition)),
        );
    for (section, idx, condition) in placed {
        let at = |extra: &[(&'static str, String)]| {
            let mut params = vec![
                ("section", section.to_string()),
                ("index", (idx + 1).to_string()),
                ("kind", condition.kind().to_string()),
            ];
            params.extend_from_slice(extra);
            params
        };
        // An error, since a rule that never matches reads on screen as a sound one.
        if condition.is_empty()
            || (value_count(condition) > 0 && folded_values(condition).is_empty())
        {
            issues.push(ValidationIssue::error(section, "ValidationConditionEmpty", &at(&[])));
        }
        // A warning, since a name no table knows is kept as a source wrote it.
        for (value, code) in uncoded_values(condition) {
            issues.push(ValidationIssue::warning(
                section,
                "ValidationValueNotCode",
                &at(&[("value", value), ("code", code)]),
            ));
        }
        if value_count(condition) > MAX_VALUES {
            issues.push(ValidationIssue::error(
                section,
                "ValidationTooManyValues",
                &at(&[("max", MAX_VALUES.to_string())]),
            ));
        }
        if let Some(value) = repeated_value(condition) {
            issues.push(ValidationIssue::error(
                section,
                "ValidationValueRepeated",
                &at(&[("value", value)]),
            ));
        }
        if let Condition::YearRange { min: Some(min), max: Some(max) } = condition
            && min > max
        {
            issues.push(ValidationIssue::error(
                section,
                "ValidationYearRangeInverted",
                &at(&[("min", min.to_string()), ("max", max.to_string())]),
            ));
        }
        if let Condition::YearRange { min, max } = condition {
            let ceiling = current_year + MAX_YEARS_AHEAD;
            for year in [min, max].into_iter().flatten() {
                if *year < MIN_YEAR || *year > ceiling {
                    issues.push(ValidationIssue::error(
                        section,
                        "ValidationYearImplausible",
                        &at(&[
                            ("year", year.to_string()),
                            ("min", MIN_YEAR.to_string()),
                            ("max", ceiling.to_string()),
                        ]),
                    ));
                }
            }
        }
        if let Condition::RuntimeRange { min: Some(min), max: Some(max) } = condition
            && min > max
        {
            issues.push(ValidationIssue::error(
                section,
                "ValidationRuntimeRangeInverted",
                &at(&[("min", min.to_string()), ("max", max.to_string())]),
            ));
        }
        if let Condition::RuntimeRange { min, max } = condition {
            for minutes in [min, max].into_iter().flatten() {
                if !(0..=MAX_RUNTIME).contains(minutes) {
                    issues.push(ValidationIssue::error(
                        section,
                        "ValidationRuntimeImplausible",
                        &at(&[("value", minutes.to_string()), ("max", MAX_RUNTIME.to_string())]),
                    ));
                }
            }
        }
        if let Condition::ScoreRange { scale, min, max, min_votes } = condition {
            for (key, params) in score_issues(scale, *min, *max, *min_votes) {
                issues.push(ValidationIssue::error(section, key, &at(&params)));
            }
        }
        // A count below one is a mistake: no title was added within the last zero days.
        if let Condition::AddedWithinDays(count)
        | Condition::SizeOnDiskOverGb(count)
        | Condition::SeasonCountOver(count) = condition
            && *count < 1
        {
            issues.push(ValidationIssue::error(
                section,
                "ValidationValueBelowOne",
                &at(&[("value", count.to_string())]),
            ));
        }
        let answered = match condition {
            Condition::ScoreRange { scale, .. } => covered_scales.contains(&scale.as_str()),
            _ => condition.metadata_field().is_none_or(|field| covered_fields.contains(&field)),
        };
        if !answered {
            issues.push(ValidationIssue::warning(section, "ValidationConditionNoSource", &at(&[])));
        }
    }

    // A condition and its own negation in `all` mode can never both hold.
    if match_mode == MatchMode::All {
        for (i, a) in conditions.iter().enumerate() {
            for b in conditions.iter().skip(i + 1) {
                if contradicts(a, b) {
                    issues.push(ValidationIssue::error(
                        "conditions",
                        "ValidationContradiction",
                        &[("first", a.kind().to_string()), ("second", b.kind().to_string())],
                    ));
                }
            }
        }
    }

    // In `any` mode another condition admits titles one exclusion may leave alone.
    if match_mode == MatchMode::All || conditions.len() == 1 {
        for exclusion in exclusions {
            if conditions.iter().any(|condition| vetoes_all(exclusion, condition)) {
                issues.push(ValidationIssue::error(
                    "exclusions",
                    "ValidationExclusionConflict",
                    &[("kind", exclusion.kind().to_string())],
                ));
            }
        }
    }

    issues
}

/// Whether `exclusion` holds for every title `condition` admits, so the rule never fires.
fn vetoes_all(exclusion: &Condition, condition: &Condition) -> bool {
    use Condition::*;
    match (condition, exclusion) {
        (CurrentRootFolder(folder), CurrentRootFolder(vetoed)) => {
            crate::paths::same(folder, vetoed)
        }
        (
            CurrentRootFolder(folder) | CurrentRootFolderStartsWith(folder),
            CurrentRootFolderStartsWith(vetoed),
        ) => crate::paths::within(folder, vetoed),
        _ => match (quantified(condition), quantified(exclusion)) {
            (Some((field, every)), Some((vetoed_field, vetoes_every))) if field == vetoed_field => {
                let wanted = folded_values(condition);
                let vetoed = folded_values(exclusion);
                match (every, vetoes_every) {
                    (false, false) => !wanted.is_empty() && wanted.is_subset(&vetoed),
                    (true, false) => !wanted.is_disjoint(&vetoed),
                    (true, true) => !vetoed.is_empty() && vetoed.is_subset(&wanted),
                    (false, true) => wanted.len() == 1 && wanted == vetoed,
                }
            }
            _ => folded(condition) == folded(exclusion),
        },
    }
}

/// The field a condition whose values are alternatives, or all required, reads, and whether
/// it requires all: `genre_contains` and `genre_contains_all` read one field.
fn quantified(condition: &Condition) -> Option<(&'static str, bool)> {
    use Condition::*;
    let any = |condition: &Condition| Some((condition.kind(), false));
    match condition {
        GenreContainsAll(_) => Some(("genre_contains", true)),
        KeywordContainsAll(_) => Some(("keyword_contains", true)),
        OriginCountryAll(_) => Some(("origin_country", true)),
        TagInAll(_) => Some(("tag_in", true)),
        GenreContains(_) | KeywordContains(_) | OriginCountry(_) | TagIn(_)
        | OriginalLanguage(_) | SeriesTypeIs(_) | QualityProfileIn(_) | StudioIn(_)
        | NetworkIn(_) | CollectionIn(_) | CertificationIn(_) | StatusIs(_) | TitleContains(_)
        | TmdbIdIn(_) | TvdbIdIn(_) | ImdbIdIn(_) => any(condition),
        _ => None,
    }
}

/// The values of a list condition, each folded as its kind compares them, blanks dropped.
fn folded_values(condition: &Condition) -> std::collections::HashSet<String> {
    let key = key_of(condition);
    serde_json::to_value(condition)
        .ok()
        .and_then(|stored| stored.get("value")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_str().map_or_else(|| value.to_string(), key))
        .filter(|key| !key.is_empty())
        .collect()
}

/// Detect the pairs of conditions that are mutually exclusive by construction.
fn contradicts(a: &Condition, b: &Condition) -> bool {
    use Condition::*;
    match (a, b) {
        (HasFiles(x), HasFiles(y))
        | (Monitored(x), Monitored(y))
        | (HasMetadata(x), HasMetadata(y))
        | (OriginalLanguageKnown(x), OriginalLanguageKnown(y)) => x != y,
        // A listed language is a known one.
        (OriginalLanguage(_), OriginalLanguageKnown(false))
        | (OriginalLanguageKnown(false), OriginalLanguage(_)) => true,
        (HasMetadata(false), other) | (other, HasMetadata(false)) => needs_metadata(other),
        // Any of a list, with values forbidden: impossible only when every value is.
        (GenreContains(x), GenreNotContains(y))
        | (GenreNotContains(y), GenreContains(x))
        | (KeywordContains(x), KeywordNotContains(y))
        | (KeywordNotContains(y), KeywordContains(x)) => forbids_all(y, x),
        // All of a list, with values forbidden: impossible as soon as one is.
        (GenreContainsAll(x), GenreNotContains(y))
        | (GenreNotContains(y), GenreContainsAll(x))
        | (KeywordContainsAll(x), KeywordNotContains(y))
        | (KeywordNotContains(y), KeywordContainsAll(x)) => overlaps(x, y),
        (OriginalLanguage(x), OriginalLanguageNot(y))
        | (OriginalLanguageNot(y), OriginalLanguage(x)) => {
            !x.is_empty() && x.iter().all(|v| y.iter().any(|w| w.eq_ignore_ascii_case(v)))
        }
        (
            ScoreRange { scale: ascale, min: amin, max: amax, .. },
            ScoreRange { scale: bscale, min: bmin, max: bmax, .. },
        ) if ascale == bscale => {
            let lo = [*amin, *bmin].into_iter().flatten().reduce(f64::max);
            let hi = [*amax, *bmax].into_iter().flatten().reduce(f64::min);
            matches!((lo, hi), (Some(l), Some(h)) if l > h)
        }
        (YearRange { min: amin, max: amax }, YearRange { min: bmin, max: bmax })
        | (RuntimeRange { min: amin, max: amax }, RuntimeRange { min: bmin, max: bmax }) => {
            // A missing bound is infinite, where `Option` would order `None` as the smallest.
            let lo = tightest(*amin, *bmin, i64::max);
            let hi = tightest(*amax, *bmax, i64::min);
            matches!((lo, hi), (Some(l), Some(h)) if l > h)
        }
        (CurrentRootFolder(x), CurrentRootFolder(y)) => !crate::paths::same(x, y),
        _ => false,
    }
}

/// Whether a condition matches only a title some source answered for: a negative one also
/// matches a title nothing is known of.
fn needs_metadata(condition: &Condition) -> bool {
    use Condition::*;
    let negative = matches!(
        condition,
        GenreNotContains(_)
            | KeywordNotContains(_)
            | OriginalLanguageNot(_)
            | OriginalLanguageKnown(false)
    );
    condition.metadata_field().is_some() && !negative
}

/// Each value of a language or country condition that names a code it is not, with that
/// code: the engine compares codes, so `Japanese` reaches no title where `ja` would.
fn uncoded_values(condition: &Condition) -> Vec<(String, String)> {
    use crate::services::templates::{country_code, language_code};
    type CodeOf = fn(&str) -> Option<String>;
    let (values, code_of): (&[String], CodeOf) = match condition {
        Condition::OriginalLanguage(values) | Condition::OriginalLanguageNot(values) => {
            (values, language_code)
        }
        Condition::OriginCountry(values) | Condition::OriginCountryAll(values) => {
            (values, country_code)
        }
        _ => return Vec::new(),
    };
    values
        .iter()
        .filter_map(|value| {
            let code = code_of(value)?;
            (!code.eq_ignore_ascii_case(value.trim())).then(|| (value.clone(), code))
        })
        .collect()
}

/// A dictionary key and the values its sentence names.
type Issue = (&'static str, Vec<(&'static str, String)>);

/// What is wrong with a score range: a scale nobody gives, a bound off its scale or inverted, a
/// vote count on a scale that counts none, or below one.
fn score_issues(
    scale: &str,
    min: Option<f64>,
    max: Option<f64>,
    min_votes: Option<i64>,
) -> Vec<Issue> {
    let Some(known) = crate::models::scale(scale) else {
        return vec![("ValidationScoreScale", vec![("value", scale.to_string())])];
    };
    let mut issues = Vec::new();
    for bound in [min, max].into_iter().flatten() {
        if !(0.0..=known.max as f64).contains(&bound) {
            let range = vec![("value", bound.to_string()), ("max", known.max.to_string())];
            issues.push(("ValidationScoreImplausible", range));
        }
    }
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        let range = vec![("min", min.to_string()), ("max", max.to_string())];
        issues.push(("ValidationScoreRangeInverted", range));
    }
    match min_votes {
        Some(_) if !known.votes => {
            issues.push(("ValidationScoreNoVotes", vec![("scale", known.name.to_string())]));
        }
        Some(votes) if votes < 1 => {
            issues.push(("ValidationValueBelowOne", vec![("value", votes.to_string())]));
        }
        _ => {}
    }
    issues
}

/// Combine two optional bounds, where `None` means "unbounded".
fn tightest(a: Option<i64>, b: Option<i64>, pick: fn(i64, i64) -> i64) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(pick(a, b)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

/// Compared as the engine compares, so two spellings it treats as one value are
/// a contradiction here too.
fn overlaps(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| normalise_value(x) == normalise_value(y)))
}

/// Whether `forbidden` holds every value of `wanted`, compared as `overlaps`.
fn forbids_all(forbidden: &[String], wanted: &[String]) -> bool {
    let forbidden: Vec<String> = forbidden.iter().map(|v| normalise_value(v)).collect();
    let mut wanted = wanted.iter().map(|v| normalise_value(v)).filter(|v| !v.is_empty()).peekable();
    wanted.peek().is_some() && wanted.all(|v| forbidden.contains(&v))
}

/// How many values a list condition holds, read through the stored shape as
/// `repeated_value` reads it.
fn value_count(condition: &Condition) -> usize {
    serde_json::to_value(condition)
        .ok()
        .and_then(|stored| stored.get("value")?.as_array().map(Vec::len))
        .unwrap_or(0)
}

/// How the values of a condition of this kind are told apart.
fn key_of(condition: &Condition) -> fn(&str) -> String {
    match condition {
        Condition::CertificationIn(_) => certification_key,
        Condition::StatusIs(_) => status_key,
        _ => normalise_value,
    }
}

/// A condition as the engine reads it: its kind, and its values folded, once
/// each, in no particular order.
fn folded(condition: &Condition) -> serde_json::Value {
    let mut stored = serde_json::to_value(condition).unwrap_or_default();
    if let Some(values) = stored.get_mut("value").and_then(serde_json::Value::as_array_mut) {
        let key = key_of(condition);
        let mut kept: Vec<serde_json::Value> = values
            .iter()
            .map(|value| value.as_str().map_or_else(|| value.clone(), |text| key(text).into()))
            .collect();
        kept.sort_by_key(serde_json::Value::to_string);
        kept.dedup();
        *values = kept;
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft<'a>(
        media_type: &'a str,
        mode: MatchMode,
        conditions: &'a [Condition],
        exclusions: &'a [Condition],
        target: &'a str,
    ) -> RuleDraft<'a> {
        RuleDraft {
            name: "Anime",
            description: None,
            priority: None,
            media_type,
            match_mode: mode,
            conditions,
            exclusions,
            target_category: target,
        }
    }

    fn env<'a>(
        known: &'a [String],
        mapped: &'a [String],
        covered: &'a [MetadataField],
    ) -> ValidationEnv<'a> {
        ValidationEnv {
            known_categories: known,
            mapped_categories: mapped,
            covered_fields: covered,
            covered_scales: &["imdb", "tmdb", "metacritic", "rotten_tomatoes", "mal", "anilist"],
            // Pinned, like `now()`, so the bounds do not move with the real year.
            current_year: 2026,
        }
    }

    fn validate(conditions: Vec<Condition>, exclusions: Vec<Condition>) -> Vec<ValidationIssue> {
        let known = ["anime".to_string(), "standard".to_string()];
        let mapped = ["anime".to_string()];
        validate_rule(
            draft("both", MatchMode::All, &conditions, &exclusions, "anime"),
            env(&known, &mapped, &MetadataField::ALL),
        )
    }

    #[test]
    fn a_sound_rule_has_no_issues() {
        assert!(
            validate(vec![Condition::GenreContains(vec!["Animation".into()])], vec![]).is_empty()
        );
    }

    #[test]
    fn an_unknown_category_is_an_error() {
        let known = ["standard".to_string()];
        let issues = validate_rule(
            draft("both", MatchMode::All, &[Condition::HasFiles(true)], &[], "does-not-exist"),
            env(&known, &known, &MetadataField::ALL),
        );
        assert!(issues.iter().any(|i| i.is_error() && i.field == "target_category"));
    }

    #[test]
    fn an_unmapped_category_is_only_a_warning() {
        let known = ["anime".to_string()];
        let issues = validate_rule(
            draft("both", MatchMode::All, &[Condition::HasFiles(true)], &[], "anime"),
            env(&known, &[], &MetadataField::ALL),
        );
        assert!(issues.iter().any(|i| !i.is_error() && i.key == "ValidationCategoryUnmapped"));
    }

    #[test]
    fn a_draft_missing_what_a_rule_needs_is_refused_field_by_field() {
        let known = ["anime".to_string()];
        let genre = [Condition::GenreContains(vec!["Animation".into()])];
        let refused = |name, media_type, conditions: &[Condition], target| {
            let draft = RuleDraft {
                name,
                description: None,
                priority: None,
                media_type,
                match_mode: MatchMode::All,
                conditions,
                exclusions: &[],
                target_category: target,
            };
            let issues = validate_rule(draft, env(&known, &known, &MetadataField::ALL));
            issues.into_iter().filter(|i| i.is_error()).map(|i| i.key).collect::<Vec<_>>()
        };

        assert_eq!(refused("  ", "both", &genre, "anime"), ["ValidationNameEmpty"]);
        assert_eq!(refused("Anime", "films", &genre, "anime"), ["ValidationMediaType"]);
        assert_eq!(refused("Anime", "both", &[], "anime"), ["ValidationNoConditions"]);
        assert_eq!(refused("Anime", "both", &genre, " "), ["ValidationCategoryEmpty"]);
        assert!(refused("Anime", "both", &genre, "anime").is_empty(), "the control was refused");
    }

    /// Every pair that can never hold together, each beside a pair that can.
    #[test]
    fn each_kind_of_contradiction_is_caught_and_only_it() {
        use Condition::*;
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        let cases = [
            (Monitored(true), Monitored(false), Monitored(true)),
            (HasMetadata(true), HasMetadata(false), HasMetadata(true)),
            (
                OriginalLanguageKnown(true),
                OriginalLanguageKnown(false),
                OriginalLanguageKnown(true),
            ),
            (
                OriginalLanguage(list(&["ja"])),
                OriginalLanguageKnown(false),
                OriginalLanguageKnown(true),
            ),
            (
                KeywordContains(list(&["mecha"])),
                KeywordNotContains(list(&["Mecha"])),
                KeywordNotContains(list(&["isekai"])),
            ),
            // Any of two genres, one of them forbidden: the other still holds.
            (
                GenreContains(list(&["Animation", "Drama"])),
                GenreNotContains(list(&["animation", "drama", "Western"])),
                GenreNotContains(list(&["Drama"])),
            ),
            (
                KeywordContainsAll(list(&["mecha", "space"])),
                KeywordNotContains(list(&["space"])),
                KeywordNotContains(list(&["isekai"])),
            ),
            (
                OriginalLanguage(list(&["ja"])),
                OriginalLanguageNot(list(&["JA", "ko"])),
                OriginalLanguageNot(list(&["ko"])),
            ),
            (
                CurrentRootFolder("/movies/anime".into()),
                CurrentRootFolder("/movies/standard".into()),
                CurrentRootFolder("/movies/anime/".into()),
            ),
        ];
        for (first, contradicting, compatible) in cases {
            assert!(contradicts(&first, &contradicting), "{first:?} beside {contradicting:?}");
            assert!(contradicts(&contradicting, &first), "{contradicting:?} beside {first:?}");
            assert!(!contradicts(&first, &compatible), "{first:?} beside {compatible:?}");
        }
    }

    #[test]
    fn contradictory_conditions_are_rejected() {
        let issues = validate(vec![Condition::HasFiles(true), Condition::HasFiles(false)], vec![]);
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationContradiction"));
    }

    #[test]
    fn overlapping_positive_and_negative_genres_contradict() {
        let issues = validate(
            vec![
                Condition::GenreContains(vec!["Animation".into()]),
                Condition::GenreNotContains(vec!["animation".into()]),
            ],
            vec![],
        );
        assert!(issues.iter().any(|i| i.is_error()));
    }

    #[test]
    fn requiring_all_of_a_genre_that_is_also_forbidden_contradicts() {
        let issues = validate(
            vec![
                Condition::GenreContainsAll(vec!["Animation".into(), "Family".into()]),
                Condition::GenreNotContains(vec!["family".into()]),
            ],
            vec![],
        );
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationContradiction"));
    }

    #[test]
    fn spellings_the_engine_folds_contradict_too() {
        let issues = validate(
            vec![
                Condition::GenreContains(vec!["Science-Fiction".into()]),
                Condition::GenreNotContains(vec!["Science Fiction".into()]),
            ],
            vec![],
        );
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationContradiction"));
    }

    #[test]
    fn impossible_year_ranges_contradict() {
        let issues = validate(
            vec![
                Condition::YearRange { min: Some(2000), max: None },
                Condition::YearRange { min: None, max: Some(1990) },
            ],
            vec![],
        );
        assert!(issues.iter().any(|i| i.is_error()));
    }

    /// A respelled exclusion vetoes every title its condition lets in all the same.
    #[test]
    fn a_condition_that_is_also_an_exclusion_is_rejected() {
        let condition = Condition::GenreContains(vec!["Animation".into(), "Family".into()]);
        let respelled = Condition::GenreContains(vec!["family".into(), " ANIMATION".into()]);
        for exclusion in [condition.clone(), respelled] {
            let issues = validate(vec![condition.clone()], vec![exclusion]);
            assert!(
                issues.iter().any(|i| i.is_error() && i.key == "ValidationExclusionConflict"),
                "{issues:?}"
            );
        }
        let narrower = Condition::GenreContains(vec!["Family".into()]);
        let issues = validate(vec![condition], vec![narrower]);
        assert!(!issues.iter().any(|i| i.key == "ValidationExclusionConflict"), "{issues:?}");
    }

    /// In `any` mode another condition still admits titles the exclusion lets through.
    #[test]
    fn an_exclusion_repeating_one_of_several_any_conditions_is_no_conflict() {
        let known = ["anime".to_string()];
        let conditions = [
            Condition::GenreContains(vec!["Music".into()]),
            Condition::KeywordContains(vec!["concert film".into()]),
        ];
        let exclusions = [Condition::GenreContains(vec!["Music".into()])];
        let issues = validate_rule(
            draft("both", MatchMode::Any, &conditions, &exclusions, "anime"),
            env(&known, &known, &MetadataField::ALL),
        );
        assert!(!issues.iter().any(|i| i.key == "ValidationExclusionConflict"), "{issues:?}");
    }

    /// An exclusion wider than its condition vetoes every title the condition admits, though
    /// the two differ.
    #[test]
    fn an_exclusion_covering_every_title_its_condition_admits_is_a_conflict() {
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        let conflicting = [
            (
                Condition::GenreContains(list(&["Animation"])),
                Condition::GenreContains(list(&["Animation", "Horror"])),
            ),
            (
                Condition::GenreContainsAll(list(&["Animation", "Family"])),
                Condition::GenreContains(list(&["family"])),
            ),
            (Condition::TagInAll(list(&["kids", "anime"])), Condition::TagInAll(list(&["anime"]))),
            (
                Condition::OriginalLanguage(list(&["ja"])),
                Condition::OriginalLanguage(list(&["ko", "ja"])),
            ),
            (
                Condition::CurrentRootFolder("/movies/anime/kids".into()),
                Condition::CurrentRootFolderStartsWith("/movies/anime".into()),
            ),
        ];
        for (condition, exclusion) in conflicting {
            let issues = validate(vec![condition.clone()], vec![exclusion.clone()]);
            assert!(
                issues.iter().any(|i| i.is_error() && i.key == "ValidationExclusionConflict"),
                "{condition:?} under {exclusion:?}: {issues:?}"
            );
        }
        let apart = validate(
            vec![Condition::GenreContains(list(&["Animation", "Drama"]))],
            vec![Condition::GenreContainsAll(list(&["Animation", "Drama"]))],
        );
        assert!(!apart.iter().any(|i| i.key == "ValidationExclusionConflict"), "{apart:?}");
    }

    #[test]
    fn an_empty_condition_value_is_an_error() {
        let issues = validate(vec![Condition::GenreContains(vec![])], vec![]);
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationConditionEmpty"));

        let blank = validate(vec![Condition::TagIn(vec!["   ".into()])], vec![]);
        assert!(blank.iter().any(|i| i.is_error() && i.key == "ValidationConditionEmpty"));
    }

    /// A value made only of signs folds to nothing, so a condition holding no other value
    /// matches no title.
    #[test]
    fn a_condition_whose_values_all_fold_to_nothing_is_empty() {
        let empty = |issues: &[ValidationIssue]| {
            issues.iter().any(|i| i.is_error() && i.key == "ValidationConditionEmpty")
        };
        for nothing in [
            Condition::GenreContains(vec!["?!".into()]),
            Condition::TagIn(vec!["-".into(), "...".into()]),
        ] {
            assert!(empty(&validate(vec![nothing.clone()], vec![])), "{nothing:?}");
        }
        let kept =
            validate(vec![Condition::GenreContains(vec!["?!".into(), "Drama".into()])], vec![]);
        assert!(!empty(&kept), "{kept:?}");
    }

    /// The engine compares codes, so a language or a country written as its name matches no
    /// title: the warning names the code, and the value stays as written.
    #[test]
    fn a_language_or_country_written_as_a_name_is_warned_of_with_its_code() {
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        let named = |condition: Condition| {
            validate(vec![condition], vec![])
                .into_iter()
                .filter(|i| i.key == "ValidationValueNotCode")
                .map(|i| {
                    assert!(!i.is_error(), "{i:?}");
                    (i.params["value"].clone(), i.params["code"].clone())
                })
                .collect::<Vec<_>>()
        };
        let pair = |value: &str, code: &str| vec![(value.to_string(), code.to_string())];
        assert_eq!(named(Condition::OriginalLanguage(list(&["Japanese"]))), pair("Japanese", "ja"));
        assert_eq!(named(Condition::OriginalLanguageNot(list(&["jpn"]))), pair("jpn", "ja"));
        assert_eq!(named(Condition::OriginalLanguage(list(&["nb"]))), pair("nb", "no"));
        assert_eq!(named(Condition::OriginCountry(list(&["Japan"]))), pair("Japan", "JP"));
        assert_eq!(named(Condition::OriginCountryAll(list(&["kor"]))), pair("kor", "KR"));
        for coded in [
            Condition::OriginalLanguage(list(&["ja", "KO"])),
            Condition::OriginCountry(list(&["JP", "kr"])),
            Condition::OriginalLanguage(list(&["klingon"])),
        ] {
            assert!(named(coded.clone()).is_empty(), "{coded:?}");
        }
    }

    /// With no metadata, a condition reading it never matches, and a negative one always does.
    #[test]
    fn no_metadata_beside_a_condition_reading_it_contradicts() {
        use Condition::*;
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        for reading in [
            GenreContains(list(&["Animation"])),
            OriginalLanguageKnown(true),
            ScoreRange { scale: "imdb".into(), min: Some(7.0), max: None, min_votes: None },
        ] {
            assert!(contradicts(&HasMetadata(false), &reading), "{reading:?}");
            assert!(contradicts(&reading, &HasMetadata(false)), "{reading:?}");
            assert!(!contradicts(&HasMetadata(true), &reading), "{reading:?}");
        }
        for negative in [
            GenreNotContains(list(&["Horror"])),
            KeywordNotContains(list(&["gore"])),
            OriginalLanguageNot(list(&["en"])),
            OriginalLanguageKnown(false),
            HasFiles(true),
        ] {
            assert!(!contradicts(&HasMetadata(false), &negative), "{negative:?}");
        }
    }

    /// A client drawing each value under its own key cannot draw one twice.
    #[test]
    fn a_value_listed_twice_in_one_condition_is_an_error() {
        let folded = validate(
            vec![Condition::GenreContains(vec![
                "Science-Fiction".into(),
                "science fiction".into(),
            ])],
            vec![],
        );
        assert!(
            folded.iter().any(|i| i.is_error() && i.key == "ValidationValueRepeated"),
            "{folded:?}"
        );

        let ids = validate(vec![], vec![Condition::TmdbIdIn(vec![603, 603])]);
        assert!(ids.iter().any(|i| i.is_error() && i.key == "ValidationValueRepeated"), "{ids:?}");

        let distinct =
            validate(vec![Condition::GenreContains(vec!["Drama".into(), "Comedy".into()])], vec![]);
        assert!(!distinct.iter().any(|i| i.key == "ValidationValueRepeated"), "{distinct:?}");

        let ratings =
            validate(vec![Condition::CertificationIn(vec!["R".into(), "R+".into()])], vec![]);
        assert!(!ratings.iter().any(|i| i.key == "ValidationValueRepeated"), "{ratings:?}");

        // A status folds as an Arr writes it, `inCinemas`, and as a person does.
        let statuses = validate(
            vec![Condition::StatusIs(vec!["inCinemas".into(), "In Cinemas".into()])],
            vec![],
        );
        assert!(statuses.iter().any(|i| i.key == "ValidationValueRepeated"), "{statuses:?}");
    }

    #[test]
    fn a_score_range_off_its_scale_or_asking_votes_of_a_scale_without_them_is_an_error() {
        let score = |scale: &str, min, max, min_votes| Condition::ScoreRange {
            scale: scale.into(),
            min,
            max,
            min_votes,
        };
        let has = |conditions: Vec<Condition>, key: &str| {
            validate(conditions, vec![]).iter().any(|issue| issue.is_error() && issue.key == key)
        };
        assert!(has(vec![score("trakt", Some(7.0), None, None)], "ValidationScoreScale"));
        assert!(has(vec![score("imdb", Some(11.0), None, None)], "ValidationScoreImplausible"));
        assert!(has(vec![score("anilist", None, Some(101.0), None)], "ValidationScoreImplausible"));
        assert!(has(
            vec![score("imdb", Some(8.0), Some(6.0), None)],
            "ValidationScoreRangeInverted"
        ));
        assert!(has(
            vec![score("metacritic", Some(60.0), None, Some(10))],
            "ValidationScoreNoVotes"
        ));
        assert!(has(vec![score("imdb", None, None, Some(0))], "ValidationValueBelowOne"));
        let apart =
            vec![score("imdb", Some(8.0), None, None), score("imdb", None, Some(6.0), None)];
        assert!(has(apart, "ValidationContradiction"));
        let scales =
            vec![score("imdb", Some(8.0), None, None), score("tmdb", None, Some(6.0), None)];
        assert!(!has(scales, "ValidationContradiction"), "two scales never contradict");
        for fine in [
            score("mal", Some(7.5), Some(10.0), Some(1000)),
            score("imdb", Some(7.0), Some(7.0), Some(1)),
        ] {
            let issues = validate(vec![fine], vec![]);
            assert!(issues.iter().all(|issue| !issue.is_error()), "{issues:?}");
        }
    }

    #[test]
    fn an_inverted_or_implausible_runtime_range_is_an_error_and_two_apart_contradict() {
        let runtime = |min, max| Condition::RuntimeRange { min, max };
        let has = |issues: &[ValidationIssue], key: &str| {
            issues.iter().any(|issue| issue.is_error() && issue.key == key)
        };

        assert!(has(
            &validate(vec![runtime(Some(120), Some(60))], vec![]),
            "ValidationRuntimeRangeInverted"
        ));
        assert!(has(
            &validate(vec![runtime(Some(-1), None)], vec![]),
            "ValidationRuntimeImplausible"
        ));
        assert!(has(
            &validate(vec![runtime(None, Some(1441))], vec![]),
            "ValidationRuntimeImplausible"
        ));
        let apart = validate(vec![runtime(Some(100), None), runtime(None, Some(40))], vec![]);
        assert!(has(&apart, "ValidationContradiction"), "{apart:?}");

        for fine in [runtime(Some(0), Some(MAX_RUNTIME)), runtime(Some(90), Some(90))] {
            let issues = validate(vec![fine], vec![]);
            assert!(issues.iter().all(|issue| !issue.is_error()), "{issues:?}");
        }
    }

    /// Inverted, and nothing more: a condition is never weighed against itself.
    #[test]
    fn an_inverted_year_range_is_an_error() {
        let issues =
            validate(vec![Condition::YearRange { min: Some(2020), max: Some(2000) }], vec![]);
        let keys: Vec<&str> = issues.iter().map(|issue| issue.key.as_str()).collect();
        assert_eq!(keys, ["ValidationYearRangeInverted"]);

        let fine =
            validate(vec![Condition::YearRange { min: Some(2000), max: Some(2000) }], vec![]);
        assert!(!fine.iter().any(|i| i.key == "ValidationYearRangeInverted"));
    }

    #[test]
    fn an_implausible_year_is_an_error_and_the_first_film_year_is_not() {
        let first = validate(vec![Condition::YearRange { min: Some(MIN_YEAR), max: None }], vec![]);
        assert!(!first.iter().any(|i| i.key == "ValidationYearImplausible"), "{first:?}");

        for range in [
            Condition::YearRange { min: Some(12), max: None },
            Condition::YearRange { min: None, max: Some(200_000) },
            // 2026 is the pinned current year, so the ceiling is 2031.
            Condition::YearRange { min: Some(2032), max: None },
        ] {
            let issues = validate(vec![range.clone()], vec![]);
            assert!(
                issues.iter().any(|i| i.is_error() && i.key == "ValidationYearImplausible"),
                "{range:?} was accepted"
            );
        }
    }

    #[test]
    fn a_year_a_few_ahead_is_accepted() {
        let issues = validate(vec![Condition::YearRange { min: Some(2031), max: None }], vec![]);
        assert!(!issues.iter().any(|i| i.key == "ValidationYearImplausible"), "{issues:?}");
    }

    /// One key alone: a count that is not empty is not called empty.
    #[test]
    fn a_count_below_one_is_refused_as_too_low() {
        for low in [
            Condition::AddedWithinDays(0),
            Condition::AddedWithinDays(-5),
            Condition::SizeOnDiskOverGb(0),
            Condition::SeasonCountOver(-1),
        ] {
            let keys: Vec<String> =
                validate(vec![low.clone()], vec![]).into_iter().map(|i| i.key).collect();
            assert_eq!(keys, ["ValidationValueBelowOne"], "{low:?}");
        }
        for one in [
            Condition::AddedWithinDays(1),
            Condition::SizeOnDiskOverGb(1),
            Condition::SeasonCountOver(1),
        ] {
            let issues = validate(vec![one.clone()], vec![]);
            assert!(issues.is_empty(), "{one:?}: {issues:?}");
        }
    }

    #[test]
    fn a_rule_holds_what_its_limits_allow_and_no_more() {
        let known = ["anime".to_string()];
        let values = |count: usize| (0..count).map(|i| format!("Genre {i}")).collect::<Vec<_>>();
        let genre = |count: usize| Condition::GenreContains(values(count));
        let keys = |conditions: &[Condition], exclusions: &[Condition], description: &str| {
            let draft = RuleDraft {
                description: Some(description),
                ..draft("both", MatchMode::All, conditions, exclusions, "anime")
            };
            validate_rule(draft, env(&known, &known, &MetadataField::ALL))
                .into_iter()
                .filter(|issue| issue.is_error())
                .map(|issue| issue.key)
                .collect::<Vec<_>>()
        };
        let many = |count: usize, prefix: &str| {
            (0..count)
                .map(|i| Condition::TitleContains(vec![format!("{prefix}{i}")]))
                .collect::<Vec<_>>()
        };
        let most = (many(MAX_CONDITIONS, "t"), many(MAX_CONDITIONS, "x"));

        let full = keys(&most.0, &most.1, &"d".repeat(MAX_DESCRIPTION_LENGTH));
        assert!(full.is_empty(), "{full:?}");
        assert!(keys(&[genre(MAX_VALUES)], &[], "").is_empty());
        assert_eq!(keys(&many(MAX_CONDITIONS + 1, "t"), &[], ""), ["ValidationTooManyConditions"]);
        assert_eq!(
            keys(&[genre(1)], &many(MAX_CONDITIONS + 1, "x"), ""),
            ["ValidationTooManyConditions"]
        );
        assert_eq!(keys(&[genre(MAX_VALUES + 1)], &[], ""), ["ValidationTooManyValues"]);
        let long = "d".repeat(MAX_DESCRIPTION_LENGTH + 1);
        assert_eq!(keys(&[genre(1)], &[], &long), ["ValidationDescriptionTooLong"]);
    }

    #[test]
    fn a_name_longer_than_the_limit_is_an_error_and_one_at_the_limit_is_not() {
        let known = ["anime".to_string()];
        let conditions = [Condition::GenreContains(vec!["Animation".into()])];
        let name = "a".repeat(MAX_NAME_LENGTH + 1);
        let mut long = draft("both", MatchMode::All, &conditions, &[], "anime");
        long.name = &name;
        let issues = validate_rule(long, env(&known, &known, &[MetadataField::Genres]));
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationNameTooLong"));

        let name = "a".repeat(MAX_NAME_LENGTH);
        let mut at_limit = draft("both", MatchMode::All, &conditions, &[], "anime");
        at_limit.name = &name;
        let issues = validate_rule(at_limit, env(&known, &known, &[MetadataField::Genres]));
        assert!(!issues.iter().any(|i| i.key == "ValidationNameTooLong"), "{issues:?}");
    }

    #[test]
    fn a_condition_no_enabled_source_can_answer_is_flagged() {
        let known = ["anime".to_string()];
        let conditions = [Condition::KeywordContains(vec!["anime".into()])];
        let issues = validate_rule(
            draft("both", MatchMode::All, &conditions, &[], "anime"),
            env(&known, &known, &[MetadataField::Genres]),
        );
        assert!(issues.iter().any(|i| i.key == "ValidationConditionNoSource"));
    }

    #[test]
    fn a_condition_a_keyless_source_can_answer_is_not_flagged() {
        let known = ["anime".to_string()];
        let conditions = [Condition::GenreContains(vec!["Animation".into()])];
        let issues = validate_rule(
            draft("both", MatchMode::All, &conditions, &[], "anime"),
            env(&known, &known, &[MetadataField::Genres]),
        );
        assert!(!issues.iter().any(|i| i.key == "ValidationConditionNoSource"));
    }

    #[test]
    fn an_invalid_media_type_is_an_error() {
        let known = ["anime".to_string()];
        let issues = validate_rule(
            draft("audiobook", MatchMode::All, &[Condition::HasFiles(true)], &[], "anime"),
            env(&known, &known, &MetadataField::ALL),
        );
        assert!(issues.iter().any(|i| i.is_error() && i.field == "media_type"));
    }

    #[test]
    fn any_mode_does_not_flag_contradictions() {
        let known = ["anime".to_string()];
        let conditions = [Condition::HasFiles(true), Condition::HasFiles(false)];
        let issues = validate_rule(
            draft("both", MatchMode::Any, &conditions, &[], "anime"),
            env(&known, &known, &MetadataField::ALL),
        );
        assert!(!issues.iter().any(|i| i.key == "ValidationContradiction"));
    }
}
