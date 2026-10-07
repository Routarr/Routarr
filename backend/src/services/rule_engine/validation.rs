//! What makes a rule refused or flagged before it is stored.

use super::{certification_key, normalise_value, status_key};
use crate::models::{Condition, MatchMode, MetadataField, ValidationIssue};

/// The first value a list condition names twice, compared as matching compares
/// them: `science fiction` repeats `Science-Fiction`, and `R+` does not repeat
/// `R`. Read through the stored shape, `{"type": …, "value": […]}`, so a list
/// variant added later is covered without an arm here.
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
    pub media_type: &'a str,
    pub match_mode: MatchMode,
    pub conditions: &'a [Condition],
    pub exclusions: &'a [Condition],
    pub target_category: &'a str,
}

/// The surrounding configuration a rule is validated against.
#[derive(Debug, Clone, Copy)]
pub struct ValidationEnv<'a> {
    /// Every category that exists.
    pub known_categories: &'a [String],
    /// Categories mapped onto a folder of an instance the rule can route: in
    /// its scope, and of the kind that holds its media type.
    pub mapped_categories: &'a [String],
    /// Metadata fields at least one enabled source can answer.
    ///
    /// Naming the fields rather than a provider is what keeps the warning
    /// truthful once there are several sources: with Radarr alone, a genre rule
    /// is perfectly fine and only a keyword rule is unanswerable.
    pub covered_fields: &'a [MetadataField],
    /// The year the caller considers current.
    ///
    /// Injected rather than read here, for the reason `EvalContext` carries its
    /// own `now`: a validator that reads the clock cannot be tested against a
    /// year that is not today's.
    pub current_year: i64,
}

/// The first surviving film, and the only defensible floor for a year: any
/// later date would be a preference, any earlier one describes nothing.
pub const MIN_YEAR: i64 = 1888;

/// How far ahead of the current year a rule may reach.
///
/// Radarr indexes announcements long before release, so a rule about a film
/// still to come is legitimate. The ceiling exists to catch `2999` and `20255`,
/// which are typing mistakes rather than intentions.
pub const MAX_YEARS_AHEAD: i64 = 5;

/// Longest name a rule may carry. A name is a table cell and a badge, and a
/// paragraph in either breaks the layout of every screen that lists rules.
pub const MAX_NAME_LENGTH: usize = 200;

/// Longest description a rule may carry: every rule list sends it.
pub const MAX_DESCRIPTION_LENGTH: usize = 1000;

/// Most conditions one list of a rule holds, its exclusions as many again.
/// Every pass reads every condition of every rule for every title, and the
/// check for contradictions reads each pair.
pub const MAX_CONDITIONS: usize = 50;

/// Most values one condition holds.
pub const MAX_VALUES: usize = 200;

/// Validate a rule before it is stored or enabled.
///
/// Errors block the write. Warnings are surfaced in the UI but do not.
pub fn validate_rule(draft: RuleDraft<'_>, env: ValidationEnv<'_>) -> Vec<ValidationIssue> {
    let RuleDraft {
        name,
        description,
        media_type,
        match_mode,
        conditions,
        exclusions,
        target_category,
    } = draft;
    let ValidationEnv { known_categories, mapped_categories, covered_fields, current_year } = env;

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

    // Each issue about one condition carries its section, its place in that
    // section and its kind: the reader finds it by where the editor shows it,
    // and the handler turns the three into a caption the reader knows.
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
        // An error, not a warning: a condition with no operand never matches,
        // so the rule would be stored dead and read on screen exactly like a
        // rule that correctly matches nothing.
        if condition.is_empty() {
            issues.push(ValidationIssue::error(section, "ValidationConditionEmpty", &at(&[])));
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
        // A year outside these bounds is a typing mistake, and stored it makes a
        // rule that matches nothing while reading on screen exactly like one
        // that correctly matches nothing.
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
        // No title was added within the last zero days, and every title with
        // a file is over zero gigabytes: a count below one is a mistake, and
        // stored it reads like a rule that correctly matches nothing.
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
        if let Some(field) = condition.metadata_field()
            && !covered_fields.contains(&field)
        {
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

    for exclusion in exclusions {
        if conditions.iter().any(|condition| folded(condition) == folded(exclusion)) {
            issues.push(ValidationIssue::error(
                "exclusions",
                "ValidationExclusionConflict",
                &[("kind", exclusion.kind().to_string())],
            ));
        }
    }

    issues
}

/// Detect the pairs of conditions that are mutually exclusive by construction.
fn contradicts(a: &Condition, b: &Condition) -> bool {
    use Condition::*;
    match (a, b) {
        (HasFiles(x), HasFiles(y))
        | (Monitored(x), Monitored(y))
        | (HasMetadata(x), HasMetadata(y)) => x != y,
        // Any of a list, with values forbidden: impossible only when every
        // value of the list is.
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
        (YearRange { min: amin, max: amax }, YearRange { min: bmin, max: bmax }) => {
            // A missing bound is infinite, which `Option`'s own ordering gets
            // backwards for the upper end (it treats `None` as the smallest).
            let lo = tightest(*amin, *bmin, i64::max);
            let hi = tightest(*amax, *bmax, i64::min);
            matches!((lo, hi), (Some(l), Some(h)) if l > h)
        }
        (CurrentRootFolder(x), CurrentRootFolder(y)) => !crate::paths::same(x, y),
        _ => false,
    }
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

    // ---------------------------------------------------------- validation

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
            media_type,
            match_mode: mode,
            conditions,
            exclusions,
            target_category: target,
        }
    }

    /// Every field covered, i.e. the shipped default with a TMDB key set.
    const ALL_FIELDS: &[MetadataField] = &[
        MetadataField::Genres,
        MetadataField::Keywords,
        MetadataField::OriginalLanguage,
        MetadataField::OriginCountries,
        MetadataField::Certification,
    ];

    fn env<'a>(
        known: &'a [String],
        mapped: &'a [String],
        covered: &'a [MetadataField],
    ) -> ValidationEnv<'a> {
        ValidationEnv {
            known_categories: known,
            mapped_categories: mapped,
            covered_fields: covered,
            // Pinned, like `now()` above: a bound relative to the real clock
            // would make these tests pass or fail depending on the year.
            current_year: 2026,
        }
    }

    fn validate(conditions: Vec<Condition>, exclusions: Vec<Condition>) -> Vec<ValidationIssue> {
        let known = ["anime".to_string(), "standard".to_string()];
        let mapped = ["anime".to_string()];
        validate_rule(
            draft("both", MatchMode::All, &conditions, &exclusions, "anime"),
            env(&known, &mapped, ALL_FIELDS),
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
            env(&known, &known, ALL_FIELDS),
        );
        assert!(issues.iter().any(|i| i.is_error() && i.field == "target_category"));
    }

    #[test]
    fn an_unmapped_category_is_only_a_warning() {
        let known = ["anime".to_string()];
        let issues = validate_rule(
            draft("both", MatchMode::All, &[Condition::HasFiles(true)], &[], "anime"),
            env(&known, &[], ALL_FIELDS),
        );
        assert!(issues.iter().any(|i| !i.is_error() && i.key == "ValidationCategoryUnmapped"));
    }

    /// Each field of a draft is refused when it cannot make a rule: an empty
    /// name, a media type that is none, no condition at all, no category.
    #[test]
    fn a_draft_missing_what_a_rule_needs_is_refused_field_by_field() {
        let known = ["anime".to_string()];
        let genre = [Condition::GenreContains(vec!["Animation".into()])];
        let refused = |name, media_type, conditions: &[Condition], target| {
            let draft = RuleDraft {
                name,
                description: None,
                media_type,
                match_mode: MatchMode::All,
                conditions,
                exclusions: &[],
                target_category: target,
            };
            let issues = validate_rule(draft, env(&known, &known, ALL_FIELDS));
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

    /// The `all` quantifier forbids nothing the `any` one does not, so pairing
    /// it with a negation of the same value is the same impossibility.
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

    /// Two spellings the engine folds into one value are one value here too:
    /// a rule the validator lets through must be one the engine can satisfy.
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

    /// Compared as the engine compares: an exclusion spelled otherwise than its
    /// condition vetoes every title the condition lets in all the same.
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

    /// An error, not a warning: stored, the rule would never match and would
    /// read on screen exactly like one that correctly matches nothing.
    #[test]
    fn an_empty_condition_value_is_an_error() {
        let issues = validate(vec![Condition::GenreContains(vec![])], vec![]);
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationConditionEmpty"));

        let blank = validate(vec![Condition::TagIn(vec!["   ".into()])], vec![]);
        assert!(blank.iter().any(|i| i.is_error() && i.key == "ValidationConditionEmpty"));
    }

    /// The values of a condition are alternatives compared once folded, so a
    /// repeat adds nothing, and a client drawing each value under its own key
    /// cannot draw it twice.
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
    }

    #[test]
    fn an_inverted_year_range_is_an_error() {
        let issues =
            validate(vec![Condition::YearRange { min: Some(2020), max: Some(2000) }], vec![]);
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationYearRangeInverted"));

        let fine =
            validate(vec![Condition::YearRange { min: Some(2000), max: Some(2000) }], vec![]);
        assert!(!fine.iter().any(|i| i.key == "ValidationYearRangeInverted"));
    }

    /// A year such as 12 or 200000, stored, makes a rule that matches nothing
    /// while reading on screen exactly like one that correctly matches nothing.
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

    /// Radarr indexes announcements long before release, so a rule about a film
    /// still to come is a rule somebody means.
    #[test]
    fn a_year_a_few_ahead_is_accepted() {
        let issues = validate(vec![Condition::YearRange { min: Some(2029), max: None }], vec![]);
        assert!(!issues.iter().any(|i| i.key == "ValidationYearImplausible"), "{issues:?}");
    }

    /// A count of days, of gigabytes or of seasons below one is refused for
    /// what it is: no day is the last zero, and "over 0" holds for nearly
    /// every title. A count that is not empty is not called empty.
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

    /// What a rule may hold is bounded, at the bound included: every pass
    /// reads every condition of every rule for every title, and every rule
    /// list sends every description.
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
            validate_rule(draft, env(&known, &known, ALL_FIELDS))
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

    /// A warning phrased as "no TMDB key" would be wrong: the Arr is a source
    /// too, and it answers genres without any key at all.
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
            env(&known, &known, ALL_FIELDS),
        );
        assert!(issues.iter().any(|i| i.is_error() && i.field == "media_type"));
    }

    #[test]
    fn any_mode_does_not_flag_contradictions() {
        let known = ["anime".to_string()];
        let conditions = [Condition::HasFiles(true), Condition::HasFiles(false)];
        let issues = validate_rule(
            draft("both", MatchMode::Any, &conditions, &[], "anime"),
            env(&known, &known, ALL_FIELDS),
        );
        assert!(!issues.iter().any(|i| i.key == "ValidationContradiction"));
    }
}
