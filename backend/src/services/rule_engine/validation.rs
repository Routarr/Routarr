//! What makes a rule refused or flagged before it is stored.

use super::normalise_value;
use crate::models::{Condition, MatchMode, MetadataField, ValidationIssue};

/// The first value a list condition names twice, compared as matching compares
/// them: `science fiction` repeats `Science-Fiction`. Read through the stored
/// shape, `{"type": …, "value": […]}`, so a list variant added later is covered
/// without an arm here.
fn repeated_value(condition: &Condition) -> Option<String> {
    let stored = serde_json::to_value(condition).ok()?;
    let mut seen = std::collections::HashSet::new();
    stored
        .get("value")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map_or_else(|| value.to_string(), str::to_string))
        .find(|text| {
            let key = normalise_value(text);
            !key.is_empty() && !seen.insert(key)
        })
}

/// The rule being validated, independent of whether it exists yet.
#[derive(Debug, Clone, Copy)]
pub struct RuleDraft<'a> {
    pub name: &'a str,
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
    /// Categories that at least one root folder is mapped to.
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

/// Validate a rule before it is stored or enabled.
///
/// Errors block the write. Warnings are surfaced in the UI but do not.
pub fn validate_rule(draft: RuleDraft<'_>, env: ValidationEnv<'_>) -> Vec<ValidationIssue> {
    let RuleDraft { name, media_type, match_mode, conditions, exclusions, target_category } = draft;
    let ValidationEnv { known_categories, mapped_categories, covered_fields, current_year } = env;

    let mut issues = Vec::new();

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
        // `added <= now` makes the elapsed day count non-negative, so a negative
        // threshold can never match. Zero can: it means the last day.
        if let Condition::AddedWithinDays(days) = condition
            && *days < 0
        {
            issues.push(ValidationIssue::error(
                section,
                "ValidationDaysNegative",
                &at(&[("value", days.to_string())]),
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
        if conditions.contains(exclusion) {
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
        // Requiring a value (any of them or all of them) and forbidding the
        // same value can never both hold.
        (GenreContains(x), GenreNotContains(y))
        | (GenreNotContains(y), GenreContains(x))
        | (GenreContainsAll(x), GenreNotContains(y))
        | (GenreNotContains(y), GenreContainsAll(x)) => overlaps(x, y),
        (KeywordContains(x), KeywordNotContains(y))
        | (KeywordNotContains(y), KeywordContains(x))
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
            media_type,
            match_mode: mode,
            conditions,
            exclusions,
            target_category: target,
        }
    }

    /// Every field covered, i.e. the shipped default with a TMDb key set.
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

    #[test]
    fn a_condition_that_is_also_an_exclusion_is_rejected() {
        let condition = Condition::GenreContains(vec!["Animation".into()]);
        let issues = validate(vec![condition.clone()], vec![condition]);
        assert!(issues.iter().any(|i| i.is_error() && i.field == "exclusions"));
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

    /// `added <= now` makes the day count non-negative, so a negative threshold
    /// never matches, while zero means the last day and is a real answer.
    #[test]
    fn a_negative_day_count_is_an_error_and_zero_is_not() {
        let issues = validate(vec![Condition::AddedWithinDays(-5)], vec![]);
        assert!(issues.iter().any(|i| i.is_error() && i.key == "ValidationDaysNegative"));

        let today = validate(vec![Condition::AddedWithinDays(0)], vec![]);
        assert!(today.is_empty(), "{today:?}");
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

    /// A warning phrased as "no TMDb key" would be wrong: the Arr is a source
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
