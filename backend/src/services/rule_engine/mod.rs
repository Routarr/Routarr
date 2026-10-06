//! Pure, synchronous routing decision logic.
//!
//! Everything here is deliberately free of I/O so the whole engine is unit
//! testable and so a simulation over thousands of items does no per-item query.

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

mod folding;
mod validation;

pub use folding::normalise_value;
use folding::{contains_all, contains_any};
pub use validation::{RuleDraft, ValidationEnv, validate_rule};

use crate::models::{Condition, MatchMode, Media, MediaMetadata, Rule};

/// One condition of a rule, held against a title.
// Structured rather than pre-rendered prose: the engine stays language-agnostic
// and the wording is resolved through the dictionary at the edges
// (`Localizer::describe`).
#[derive(Debug, Clone, Serialize, PartialEq, utoipa::ToSchema)]
pub struct ConditionOutcome {
    pub kind: String,
    pub matched: bool,
    /// Translation key describing what the rule asked for.
    pub key: String,
    /// Placeholder values for `key`.
    pub params: BTreeMap<String, String>,
    /// What the media actually carried, verbatim.
    pub observed: String,
    /// What the condition asked for, in the interface language.
    // Filled in by `Localizer::localize_outcome`.
    #[serde(default)]
    pub expected: String,
    /// Which metadata source supplied `observed`, absent for a condition the
    /// Arr answers directly.
    // With several sources enabled, "genre does not contain Animation" is only
    // explainable if the reader can see whether the genres came from Radarr or
    // from TMDb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ConditionOutcome {
    fn new(
        kind: &str,
        matched: bool,
        key: &str,
        params: &[(&str, String)],
        observed: String,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            matched,
            key: key.to_string(),
            params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            observed,
            expected: String::new(),
            source: None,
        }
    }
}

/// Result of evaluating a single rule against a media item.
#[derive(Debug, Clone, Serialize)]
pub struct RuleMatch {
    pub rule_id: String,
    pub rule_name: String,
    pub category: String,
    pub evaluations: Vec<ConditionOutcome>,
    /// 0.0–1.0 confidence, driven by how many independent signals agreed.
    pub confidence: f32,
    /// The exclusion that vetoed an otherwise-matching rule.
    pub excluded_by: Option<ConditionOutcome>,
}

/// The rules that apply to `media`, in the order they are tried: enabled,
/// covering its type and its instance.
///
/// Lower priority number wins, ties break on name and then on id, so the
/// outcome does not depend on the order SQLite returns rows in. Name alone is
/// not enough: nothing stops two rules sharing one (the interface accepts the
/// same name twice and duplication only appends "(copy)"), and two such rules
/// can target different categories.
pub fn in_order<'a>(rules: &'a [Rule], media: &Media) -> Vec<&'a Rule> {
    let mut applicable: Vec<&Rule> = rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| r.covers_media_type(&media.media_type))
        .filter(|r| r.covers_instance(&media.instance_id))
        .collect();
    applicable.sort_by(|a, b| {
        a.priority.cmp(&b.priority).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id))
    });
    applicable
}

/// Marker used in place of a rule id when a human decision wins.
pub const OVERRIDE_RULE_ID: &str = "override";

/// The outcome of evaluating every rule for one media item.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub winner: Option<RuleMatch>,
    /// Rules that also matched but lost on priority.
    pub alternatives: Vec<RuleMatch>,
    /// Rules that matched their conditions but were vetoed by an exclusion.
    pub excluded: Vec<RuleMatch>,
}

/// Everything the engine needs about one media item, resolved up-front.
#[derive(Debug, Clone, Copy)]
pub struct EvalContext<'a> {
    pub media: &'a Media,
    pub metadata: Option<&'a MediaMetadata>,
    /// Reference instant for time-relative conditions, injected for testability.
    pub now: DateTime<Utc>,
}

/// Evaluate all rules against a single media item.
///
/// Rules are considered in ascending `priority` order and the first one whose
/// conditions hold wins. The rest are reported as alternatives so the UI can
/// explain what was considered and discarded.
pub fn evaluate_rules(
    ctx: EvalContext<'_>,
    rules: &[Rule],
    override_category: Option<&str>,
) -> Evaluation {
    // An explicit human decision outranks the entire engine: automation
    // must never prevent an explicit manual decision.
    if let Some(cat) = override_category {
        return Evaluation {
            winner: Some(RuleMatch {
                rule_id: OVERRIDE_RULE_ID.to_string(),
                rule_name: String::new(),
                category: cat.to_string(),
                evaluations: vec![],
                confidence: 1.0,
                excluded_by: None,
            }),
            alternatives: vec![],
            excluded: vec![],
        };
    }

    let mut matches: Vec<RuleMatch> = Vec::new();
    let mut excluded: Vec<RuleMatch> = Vec::new();

    for rule in in_order(rules, ctx.media) {
        let (matched, evaluations) = evaluate_conditions(&rule.conditions, rule.match_mode, ctx);
        if !matched {
            continue;
        }

        // Exclusions are evaluated only once the rule would otherwise fire, and
        // any single one of them vetoes it.
        let veto = rule
            .exclusions
            .iter()
            .map(|c| evaluate_single_condition(c, ctx))
            .find(|outcome| outcome.matched);

        let confidence = confidence_for(&evaluations, rule.match_mode);
        let mut hit = RuleMatch {
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            category: rule.target_category.clone(),
            evaluations,
            confidence,
            excluded_by: None,
        };

        match veto {
            Some(outcome) => {
                hit.excluded_by = Some(outcome);
                excluded.push(hit);
            }
            None => matches.push(hit),
        }
    }

    let winner = if matches.is_empty() { None } else { Some(matches.remove(0)) };

    Evaluation { winner, alternatives: matches, excluded }
}

/// Confidence heuristic: more agreeing signals means a more trustworthy call.
///
/// `all` mode starts higher because every condition had to hold. `any` mode is
/// intentionally capped lower since a single weak signal can carry it.
fn confidence_for(evaluations: &[ConditionOutcome], mode: MatchMode) -> f32 {
    let agreeing = evaluations.iter().filter(|e| e.matched).count() as f32;
    if agreeing == 0.0 {
        return 0.0;
    }
    let (base, step, ceiling) = match mode {
        MatchMode::All => (0.60_f32, 0.10_f32, 0.98_f32),
        MatchMode::Any => (0.45_f32, 0.08_f32, 0.85_f32),
    };
    (base + step * (agreeing - 1.0)).min(ceiling)
}

/// Evaluate the conditions of a rule under its match mode.
///
/// Every condition is always evaluated, even in `any` mode where an early exit
/// would be cheaper: the non-matching ones are part of the explanation.
pub(crate) fn evaluate_conditions(
    conditions: &[Condition],
    mode: MatchMode,
    ctx: EvalContext<'_>,
) -> (bool, Vec<ConditionOutcome>) {
    if conditions.is_empty() {
        return (false, vec![]);
    }

    let outcomes: Vec<ConditionOutcome> =
        conditions.iter().map(|c| evaluate_single_condition(c, ctx)).collect();

    let matched = match mode {
        MatchMode::All => outcomes.iter().all(|o| o.matched),
        MatchMode::Any => outcomes.iter().any(|o| o.matched),
    };

    (matched, outcomes)
}

/// Evaluate a single condition against media data.
///
/// Returns the key and parameters of the wording, never the wording itself.
pub fn evaluate_single_condition(condition: &Condition, ctx: EvalContext<'_>) -> ConditionOutcome {
    let media = ctx.media;
    let metadata = ctx.metadata;
    let kind = condition.kind();

    /// Shorthand for the very common "list of values vs a list of observations".
    fn list(
        kind: &str,
        key: &str,
        matched: bool,
        values: &[String],
        observed: &[String],
    ) -> ConditionOutcome {
        ConditionOutcome::new(
            kind,
            matched,
            key,
            &[("values", values.join(", "))],
            observed.join(", "),
        )
    }

    let mut outcome = match condition {
        Condition::GenreContains(values) => {
            let genres = metadata.map(|m| m.genres.as_slice()).unwrap_or_default();
            list(kind, "ConditionGenreContains", contains_any(genres, values), values, genres)
        }

        Condition::GenreContainsAll(values) => {
            let genres = metadata.map(|m| m.genres.as_slice()).unwrap_or(&[]);
            list(kind, "ConditionGenreContainsAll", contains_all(genres, values), values, genres)
        }

        Condition::GenreNotContains(values) => {
            let genres = metadata.map(|m| m.genres.as_slice()).unwrap_or_default();
            list(kind, "ConditionGenreNotContains", !contains_any(genres, values), values, genres)
        }

        Condition::KeywordContains(values) => {
            let keywords = metadata.map(|m| m.keywords.as_slice()).unwrap_or_default();
            list(kind, "ConditionKeywordContains", contains_any(keywords, values), values, keywords)
        }

        Condition::KeywordContainsAll(values) => {
            let keywords = metadata.map(|m| m.keywords.as_slice()).unwrap_or(&[]);
            list(
                kind,
                "ConditionKeywordContainsAll",
                contains_all(keywords, values),
                values,
                keywords,
            )
        }

        Condition::KeywordNotContains(values) => {
            let keywords = metadata.map(|m| m.keywords.as_slice()).unwrap_or_default();
            list(
                kind,
                "ConditionKeywordNotContains",
                !contains_any(keywords, values),
                values,
                keywords,
            )
        }

        Condition::OriginalLanguage(values) => {
            let lang = metadata.and_then(|m| m.original_language.as_deref()).unwrap_or("");
            ConditionOutcome::new(
                kind,
                language_listed(lang, values),
                "ConditionOriginalLanguage",
                &[("values", values.join(", "))],
                lang.to_string(),
            )
        }

        Condition::OriginalLanguageNot(values) => {
            let lang = metadata.and_then(|m| m.original_language.as_deref()).unwrap_or("");
            ConditionOutcome::new(
                kind,
                !language_listed(lang, values),
                "ConditionOriginalLanguageNot",
                &[("values", values.join(", "))],
                lang.to_string(),
            )
        }

        Condition::OriginCountry(values) => {
            let countries = metadata.map(|m| m.origin_countries.as_slice()).unwrap_or_default();
            list(kind, "ConditionOriginCountry", contains_any(countries, values), values, countries)
        }

        Condition::OriginCountryAll(values) => {
            let countries = metadata.map(|m| m.origin_countries.as_slice()).unwrap_or(&[]);
            list(
                kind,
                "ConditionOriginCountryAll",
                contains_all(countries, values),
                values,
                countries,
            )
        }

        Condition::YearRange { min, max } => {
            // A missing year must not silently satisfy an open-ended range.
            let matched = match media.year {
                None => false,
                Some(year) => {
                    min.map(|m| year >= m).unwrap_or(true) && max.map(|m| year <= m).unwrap_or(true)
                }
            };
            ConditionOutcome::new(
                kind,
                matched,
                "ConditionYearRange",
                &[
                    ("min", min.map(|v| v.to_string()).unwrap_or_else(|| "*".into())),
                    ("max", max.map(|v| v.to_string()).unwrap_or_else(|| "*".into())),
                ],
                media.year.map(|y| y.to_string()).unwrap_or_default(),
            )
        }

        Condition::CertificationIn(values) => {
            let cert = metadata.and_then(|m| m.certification.as_deref()).unwrap_or("");
            ConditionOutcome::new(
                kind,
                !cert.is_empty() && contains_any(&[cert.to_string()], values),
                "ConditionCertificationIn",
                &[("values", values.join(", "))],
                cert.to_string(),
            )
        }

        Condition::StatusIs(values) => {
            let status = media.status.as_deref().unwrap_or("");
            ConditionOutcome::new(
                kind,
                contains_any(&[status.to_string()], values),
                "ConditionStatusIs",
                &[("values", values.join(", "))],
                status.to_string(),
            )
        }

        Condition::CurrentRootFolder(path) => {
            let current = media.current_root_folder.as_deref().unwrap_or("");
            ConditionOutcome::new(
                kind,
                crate::paths::same(current, path),
                "ConditionCurrentRootFolder",
                &[("value", path.clone())],
                current.to_string(),
            )
        }

        Condition::CurrentRootFolderStartsWith(prefix) => {
            let current = media.current_root_folder.as_deref().unwrap_or("");
            ConditionOutcome::new(
                kind,
                crate::paths::within(current, prefix),
                "ConditionCurrentRootFolderStartsWith",
                &[("value", prefix.clone())],
                current.to_string(),
            )
        }

        Condition::HasFiles(expected) => ConditionOutcome::new(
            kind,
            media.has_files == *expected,
            "ConditionHasFiles",
            &[("value", expected.to_string())],
            media.has_files.to_string(),
        ),

        Condition::Monitored(expected) => ConditionOutcome::new(
            kind,
            media.monitored == *expected,
            "ConditionMonitored",
            &[("value", expected.to_string())],
            media.monitored.to_string(),
        ),

        Condition::HasMetadata(expected) => ConditionOutcome::new(
            kind,
            metadata.is_some() == *expected,
            "ConditionHasMetadata",
            &[("value", expected.to_string())],
            metadata.is_some().to_string(),
        ),

        // ---- signals the Arr already knows, no external provider involved ----
        Condition::TagIn(values) => {
            let tags = media.tag_labels();
            ConditionOutcome::new(
                kind,
                contains_any(&tags, values),
                "ConditionTagIn",
                &[("values", values.join(", "))],
                if tags.is_empty() { "-".to_string() } else { tags.join(", ") },
            )
        }

        Condition::TagInAll(values) => {
            let tags = media.tag_labels();
            ConditionOutcome::new(
                kind,
                contains_all(&tags, values),
                "ConditionTagInAll",
                &[("values", values.join(", "))],
                if tags.is_empty() { "-".to_string() } else { tags.join(", ") },
            )
        }

        Condition::SeriesTypeIs(values) => {
            let actual = media.series_type.as_deref().unwrap_or("").trim().to_lowercase();
            ConditionOutcome::new(
                kind,
                // An absent series type never matches, including for movies:
                // an unknown value must not satisfy a condition.
                !actual.is_empty() && values.iter().any(|v| v.trim().eq_ignore_ascii_case(&actual)),
                "ConditionSeriesTypeIs",
                &[("values", values.join(", "))],
                if actual.is_empty() { "-".to_string() } else { actual },
            )
        }

        Condition::SizeOnDiskOverGb(threshold) => {
            const GB: i64 = 1024 * 1024 * 1024;
            let bytes = media.size_on_disk.unwrap_or(0);
            ConditionOutcome::new(
                kind,
                media.size_on_disk.is_some_and(|size| size > threshold.saturating_mul(GB)),
                "ConditionSizeOnDiskOverGb",
                &[("value", threshold.to_string())],
                match media.size_on_disk {
                    Some(_) => format!("{:.1} GB", bytes as f64 / GB as f64),
                    None => "-".to_string(),
                },
            )
        }

        Condition::SeasonCountOver(threshold) => ConditionOutcome::new(
            kind,
            media.season_count.is_some_and(|count| count > *threshold),
            "ConditionSeasonCountOver",
            &[("value", threshold.to_string())],
            media.season_count.map_or_else(|| "-".to_string(), |c| c.to_string()),
        ),

        Condition::TitleContains(values) => {
            // Normalised on both sides, like every other string condition, so
            // `amelie` finds "Amélie" and `spider man` finds "Spider-Man" as
            // `Science-Fiction` finds "Science Fiction" in a genre. Compared
            // with `to_lowercase` alone, a title would fold case, keep the
            // accents, and be the one condition to break the engine's contract.
            let title = normalise_value(&media.title);
            ConditionOutcome::new(
                kind,
                values
                    .iter()
                    .map(|v| normalise_value(v))
                    .filter(|v| !v.is_empty())
                    .any(|v| title.contains(&v)),
                "ConditionTitleContains",
                &[("values", values.join(", "))],
                media.title.clone(),
            )
        }

        Condition::TmdbIdIn(ids) => ConditionOutcome::new(
            kind,
            media.tmdb_id.is_some_and(|id| ids.contains(&id)),
            "ConditionTmdbIdIn",
            &[("values", join_ids(ids))],
            media.tmdb_id.map(|v| v.to_string()).unwrap_or_default(),
        ),

        Condition::TvdbIdIn(ids) => ConditionOutcome::new(
            kind,
            media.tvdb_id.is_some_and(|id| ids.contains(&id)),
            "ConditionTvdbIdIn",
            &[("values", join_ids(ids))],
            media.tvdb_id.map(|v| v.to_string()).unwrap_or_default(),
        ),

        Condition::ImdbIdIn(ids) => {
            let imdb = media.imdb_id.as_deref().unwrap_or("");
            ConditionOutcome::new(
                kind,
                !imdb.is_empty() && ids.iter().any(|v| v.trim().eq_ignore_ascii_case(imdb)),
                "ConditionImdbIdIn",
                &[("values", ids.join(", "))],
                imdb.to_string(),
            )
        }

        Condition::AddedWithinDays(days) => {
            let added = media.added_at.as_deref().and_then(parse_timestamp);
            ConditionOutcome::new(
                kind,
                added
                    .is_some_and(|added| added <= ctx.now && (ctx.now - added).num_days() <= *days),
                "ConditionAddedWithinDays",
                &[("value", days.to_string())],
                media.added_at.clone().unwrap_or_default(),
            )
        }
    };

    // Which source answered, for the conditions that read one at all.
    if let Some(field) = condition.metadata_field() {
        outcome.source = metadata.and_then(|m| m.source_of(field.as_str())).map(str::to_string);
    }

    outcome
}

/// Whether a language is among `values`. A missing language is absent, never
/// the empty string, so a blank value in the list reaches no item.
fn language_listed(lang: &str, values: &[String]) -> bool {
    !lang.is_empty() && values.iter().any(|v| v.trim().eq_ignore_ascii_case(lang))
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
}

/// Parse the timestamp formats Arr and SQLite hand us.
fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(naive.and_utc());
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, fmt) {
            return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Condition, MatchMode, Media, ProviderMetadata, Rule};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 21, 12, 0, 0).unwrap()
    }

    fn media() -> Media {
        Media {
            id: "m-1".into(),
            instance_id: "inst-1".into(),
            arr_id: 1,
            media_type: "movie".into(),
            title: "My Neighbor Totoro".into(),
            sort_title: None,
            year: Some(1988),
            tmdb_id: Some(8392),
            tvdb_id: None,
            imdb_id: Some("tt0096283".into()),
            current_path: Some("/movies/standard/My Neighbor Totoro (1988)".into()),
            current_root_folder: Some("/movies/standard".into()),
            monitored: true,
            has_files: true,
            status: Some("released".into()),
            added_at: Some("2026-08-20 10:00:00".into()),
            series_type: None,
            size_on_disk: None,
            season_count: None,
            tags: None,
            genres: None,
            original_language: None,
            certification: None,
            last_synced_at: None,
        }
    }

    /// The canonical fixture, as one source would answer it.
    fn metadata() -> MediaMetadata {
        MediaMetadata::merge([(
            "tmdb",
            ProviderMetadata {
                genres: vec!["Animation".into(), "Family".into(), "Fantasy".into()],
                keywords: vec!["anime".into(), "studio ghibli".into()],
                original_language: Some("ja".into()),
                origin_countries: vec!["JP".into()],
                certification: Some("G".into()),
                certification_scale: Some("US".into()),
                status: Some("Released".into()),
                overview: None,
                poster_path: None,
            },
        )])
        .unwrap()
    }

    fn rule(name: &str, priority: i64, category: &str, conditions: Vec<Condition>) -> Rule {
        Rule {
            id: format!("rule-{name}"),
            name: name.into(),
            description: None,
            priority,
            enabled: true,
            media_type: "both".into(),
            conditions,
            exclusions: vec![],
            match_mode: MatchMode::All,
            target_category: category.into(),
            instance_ids: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn anime_rule() -> Rule {
        rule(
            "anime",
            10,
            "anime",
            vec![
                Condition::OriginalLanguage(vec!["ja".into()]),
                Condition::GenreContains(vec!["Animation".into()]),
            ],
        )
    }

    fn evaluate(rules: &[Rule], override_category: Option<&str>) -> Evaluation {
        let media = media();
        let metadata = metadata();
        evaluate_rules(
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
            rules,
            override_category,
        )
    }

    // ---------------------------------------------------------- selection

    #[test]
    fn matching_rule_wins() {
        let result = evaluate(&[anime_rule()], None);
        assert_eq!(result.winner.unwrap().category, "anime");
        assert!(result.alternatives.is_empty());
    }

    #[test]
    fn override_beats_every_rule() {
        let result = evaluate(&[anime_rule()], Some("kids"));
        let winner = result.winner.unwrap();
        assert_eq!(winner.category, "kids");
        assert_eq!(winner.rule_id, OVERRIDE_RULE_ID);
        assert_eq!(winner.confidence, 1.0);
        assert!(result.alternatives.is_empty(), "an override short-circuits evaluation");
    }

    #[test]
    fn no_match_yields_no_winner() {
        let drama =
            rule("drama", 10, "standard", vec![Condition::GenreContains(vec!["Drama".into()])]);
        assert!(evaluate(&[drama], None).winner.is_none());
    }

    #[test]
    fn lower_priority_number_wins_and_others_become_alternatives() {
        let kids = rule("kids", 5, "kids", vec![Condition::GenreContains(vec!["Family".into()])]);
        let result = evaluate(&[anime_rule(), kids], None);

        assert_eq!(result.winner.unwrap().category, "kids");
        assert_eq!(result.alternatives.len(), 1);
        assert_eq!(result.alternatives[0].category, "anime");
    }

    #[test]
    fn equal_priorities_break_ties_deterministically() {
        let a = rule("aaa", 10, "cat-a", vec![Condition::HasFiles(true)]);
        let b = rule("zzz", 10, "cat-z", vec![Condition::HasFiles(true)]);

        // Same rules, opposite input order, must give the same winner.
        let first = evaluate(&[a.clone(), b.clone()], None).winner.unwrap().category;
        let second = evaluate(&[b, a], None).winner.unwrap().category;
        assert_eq!(first, second);
        assert_eq!(first, "cat-a");
    }

    /// Nothing stops two rules sharing a name: the interface accepts the same
    /// one twice, and duplicating a rule only appends "(copy)". With the
    /// priority equal too, name leaves a genuine tie whose winner would be
    /// whatever SQLite returns first, and these two send the same film to
    /// different folders.
    #[test]
    fn two_rules_with_the_same_name_and_priority_still_have_an_order() {
        let mut a = rule("same", 10, "cat-a", vec![Condition::HasFiles(true)]);
        let mut b = rule("same", 10, "cat-z", vec![Condition::HasFiles(true)]);
        a.id = "rule-000".into();
        b.id = "rule-999".into();

        let first = evaluate(&[a.clone(), b.clone()], None).winner.unwrap().category;
        let second = evaluate(&[b, a], None).winner.unwrap().category;

        assert_eq!(first, second, "the winner depends on the input order");
        assert_eq!(first, "cat-a");
    }

    #[test]
    fn disabled_rules_are_ignored() {
        let mut disabled = anime_rule();
        disabled.enabled = false;
        assert!(evaluate(&[disabled], None).winner.is_none());
    }

    #[test]
    fn media_type_scope_is_respected() {
        let mut series_only = anime_rule();
        series_only.media_type = "series".into();
        assert!(evaluate(&[series_only.clone()], None).winner.is_none());

        let mut movies_only = anime_rule();
        movies_only.media_type = "movie".into();
        assert_eq!(evaluate(&[movies_only], None).winner.unwrap().category, "anime");

        let mut series = media();
        series.media_type = "series".into();
        let metadata = metadata();
        let ctx = EvalContext { media: &series, metadata: Some(&metadata), now: now() };
        assert_eq!(evaluate_rules(ctx, &[series_only], None).winner.unwrap().category, "anime");
    }

    #[test]
    fn instance_scope_is_respected() {
        let mut scoped = anime_rule();
        scoped.instance_ids = Some(vec!["other-instance".into()]);
        assert!(evaluate(&[scoped.clone()], None).winner.is_none());

        scoped.instance_ids = Some(vec!["inst-1".into()]);
        assert!(evaluate(&[scoped], None).winner.is_some());
    }

    #[test]
    fn an_empty_instance_list_means_all_instances() {
        let mut scoped = anime_rule();
        scoped.instance_ids = Some(vec![]);
        assert!(evaluate(&[scoped], None).winner.is_some());
    }

    #[test]
    fn a_rule_without_conditions_never_fires() {
        assert!(evaluate(&[rule("empty", 1, "kids", vec![])], None).winner.is_none());
    }

    // ------------------------------------------------- genres: OR and AND
    //
    // The one distinction the shape of a rule has to make unambiguous. Several
    // values in one condition are alternatives. Requiring several at once is
    // the `_all` form of the condition, or several conditions under
    // `MatchMode::All`. Nothing infers an AND from a separator, so a rule means
    // the same thing however its values were typed.

    /// Evaluate one rule against the fixture, whose genres are Animation,
    /// Family and Fantasy.
    fn genre_rule(conditions: Vec<Condition>, mode: MatchMode) -> bool {
        let mut r = rule("genre", 10, "anime", conditions);
        r.match_mode = mode;
        evaluate(&[r], None).winner.is_some()
    }

    #[test]
    fn one_genre_matches_the_item_carrying_it() {
        assert!(genre_rule(
            vec![Condition::GenreContains(vec!["Animation".into()])],
            MatchMode::All
        ));
        assert!(!genre_rule(vec![Condition::GenreContains(vec!["Horror".into()])], MatchMode::All));
    }

    #[test]
    fn several_genres_in_one_condition_are_alternatives() {
        // Only the second is carried, and the condition still holds.
        assert!(genre_rule(
            vec![Condition::GenreContains(vec!["Horror".into(), "Animation".into()])],
            MatchMode::All,
        ));
        // Neither is, and it does not.
        assert!(!genre_rule(
            vec![Condition::GenreContains(vec!["Horror".into(), "Western".into()])],
            MatchMode::All,
        ));
    }

    /// Whether `condition` holds for `media` and the canonical metadata.
    fn holds(condition: Condition, media: &Media) -> bool {
        let metadata = metadata();
        evaluate_single_condition(
            &condition,
            EvalContext { media, metadata: Some(&metadata), now: now() },
        )
        .matched
    }

    /// Sonarr's series type is one of a few words: any of the condition's
    /// values may name it, in any case and with stray spaces, and a title
    /// with no series type, a film among them, matches none.
    #[test]
    fn a_series_type_matches_any_of_its_values_whatever_their_case() {
        use Condition::SeriesTypeIs;
        let values = |list: &[&str]| SeriesTypeIs(list.iter().map(|v| v.to_string()).collect());
        let series = |kind: Option<&str>| Media {
            media_type: "series".into(),
            series_type: kind.map(str::to_string),
            ..media()
        };

        assert!(holds(values(&["standard", " ANIME "]), &series(Some("anime"))));
        assert!(holds(values(&["daily", "anime"]), &series(Some("Anime"))));
        assert!(!holds(values(&["standard", "daily"]), &series(Some("anime"))));
        assert!(!holds(values(&["anime"]), &series(None)));
        assert!(!holds(values(&[""]), &series(Some(" "))));
    }

    #[test]
    fn requiring_every_keyword_needs_each_of_them() {
        let both = vec!["anime".into(), "Studio Ghibli".into()];
        assert!(holds(Condition::KeywordContainsAll(both), &media()));
        let one_missing = vec!["anime".into(), "mecha".into()];
        assert!(!holds(Condition::KeywordContainsAll(one_missing.clone()), &media()));
        assert!(holds(Condition::KeywordContains(one_missing), &media()));
    }

    #[test]
    fn requiring_every_origin_country_needs_each_of_them() {
        assert!(holds(Condition::OriginCountryAll(vec!["jp".into()]), &media()));
        let coproduction = vec!["JP".into(), "FR".into()];
        assert!(!holds(Condition::OriginCountryAll(coproduction.clone()), &media()));
        assert!(holds(Condition::OriginCountry(coproduction), &media()));
    }

    #[test]
    fn requiring_every_tag_needs_each_of_them() {
        let tagged = Media { tags: Some(r#"["anime", "Kids"]"#.into()), ..media() };
        assert!(holds(Condition::TagInAll(vec!["kids".into(), "anime".into()]), &tagged));
        let one_missing = vec!["anime".into(), "4k".into()];
        assert!(!holds(Condition::TagInAll(one_missing.clone()), &tagged));
        assert!(holds(Condition::TagIn(one_missing), &tagged));
    }

    /// The Arr's own status, whatever its case, against any of the values.
    #[test]
    fn a_status_matches_any_of_the_values_it_names() {
        assert!(holds(Condition::StatusIs(vec!["announced".into(), "Released".into()]), &media()));
        assert!(!holds(Condition::StatusIs(vec!["announced".into()]), &media()));
        let unknown = Media { status: None, ..media() };
        assert!(!holds(Condition::StatusIs(vec!["released".into()]), &unknown));
    }

    /// The quantifier, which is what removes the need for a second condition.
    #[test]
    fn one_condition_can_require_every_genre_it_names() {
        assert!(genre_rule(
            vec![Condition::GenreContainsAll(vec!["Animation".into(), "Family".into()])],
            MatchMode::All,
        ));
        // Fantasy is carried, Western is not, so the pair cannot hold, where
        // the `any` form would.
        let mixed = vec!["Fantasy".into(), "Western".into()];
        assert!(!genre_rule(vec![Condition::GenreContainsAll(mixed.clone())], MatchMode::All));
        assert!(genre_rule(vec![Condition::GenreContains(mixed)], MatchMode::All));
    }

    #[test]
    fn requiring_every_genre_folds_spelling_the_same_way() {
        assert!(genre_rule(
            vec![Condition::GenreContainsAll(vec![" animation ".into(), "FAMILY".into()])],
            MatchMode::All,
        ));
    }

    /// An empty list is a condition nobody finished writing, so it matches
    /// nothing. Answering "every value in an empty set is present" would make
    /// it match the whole library instead.
    #[test]
    fn requiring_every_genre_of_nothing_matches_nothing() {
        assert!(!genre_rule(vec![Condition::GenreContainsAll(vec![])], MatchMode::All));
        assert!(Condition::GenreContainsAll(vec![]).is_empty());
    }

    /// Both quantifiers stay available as separate conditions, which is what a
    /// rule needs to say "animated, and either science fiction or fantasy".
    #[test]
    fn the_two_quantifiers_combine_in_one_rule() {
        assert!(genre_rule(
            vec![
                Condition::GenreContainsAll(vec!["Animation".into(), "Family".into()]),
                Condition::GenreContains(vec!["Fantasy".into(), "Horror".into()]),
            ],
            MatchMode::All,
        ));
    }

    #[test]
    fn the_all_form_round_trips_under_its_own_kind() {
        let condition = Condition::GenreContainsAll(vec!["Animation".into(), "Family".into()]);
        let json = serde_json::to_string(&condition).unwrap();
        assert_eq!(json, r#"{"type":"genre_contains_all","value":["Animation","Family"]}"#);
        assert_eq!(serde_json::from_str::<Condition>(&json).unwrap(), condition);
        // And the `any` form it was switched from is a different kind, so a
        // stored rule cannot change meaning by being reopened.
        assert_ne!(condition.kind(), Condition::GenreContains(vec![]).kind());
    }

    #[test]
    fn two_genre_conditions_under_all_require_both() {
        assert!(genre_rule(
            vec![
                Condition::GenreContains(vec!["Animation".into()]),
                Condition::GenreContains(vec!["Family".into()]),
            ],
            MatchMode::All,
        ));
        // Western is absent, so the pair cannot hold, where `any` would.
        let both_needed = vec![
            Condition::GenreContains(vec!["Animation".into()]),
            Condition::GenreContains(vec!["Western".into()]),
        ];
        assert!(!genre_rule(both_needed.clone(), MatchMode::All));
        assert!(genre_rule(both_needed, MatchMode::Any));
    }

    /// The case that reads ambiguously in any other arrangement: one condition
    /// offering a choice, another demanding a genre outright.
    #[test]
    fn a_multi_genre_condition_combines_with_a_second_one() {
        assert!(genre_rule(
            vec![
                Condition::GenreContains(vec!["Fantasy".into(), "Horror".into()]),
                Condition::GenreContains(vec!["Animation".into()]),
            ],
            MatchMode::All,
        ));
        // The alternative half now offers nothing the item carries.
        assert!(!genre_rule(
            vec![
                Condition::GenreContains(vec!["Western".into(), "Horror".into()]),
                Condition::GenreContains(vec!["Animation".into()]),
            ],
            MatchMode::All,
        ));
    }

    #[test]
    fn an_item_with_no_genre_at_all_matches_neither_form() {
        let media = media();
        let empty = MediaMetadata::default();
        let mut r = rule(
            "genre",
            10,
            "anime",
            vec![Condition::GenreContains(vec!["Animation".into(), "Family".into()])],
        );
        r.match_mode = MatchMode::All;
        let evaluation = evaluate_rules(
            EvalContext { media: &media, metadata: Some(&empty), now: now() },
            &[r],
            None,
        );
        assert!(evaluation.winner.is_none());
    }

    #[test]
    fn several_values_round_trip_through_the_stored_form() {
        let condition = Condition::GenreContains(vec!["Science Fiction".into(), "Fantasy".into()]);
        let json = serde_json::to_string(&condition).unwrap();
        assert_eq!(json, r#"{"type":"genre_contains","value":["Science Fiction","Fantasy"]}"#);
        assert_eq!(serde_json::from_str::<Condition>(&json).unwrap(), condition);
    }

    // ---------------------------------------------------------- match modes

    #[test]
    fn any_mode_requires_only_one_condition() {
        let mut r = rule(
            "concerts",
            10,
            "concerts",
            vec![
                Condition::GenreContains(vec!["Music".into()]),
                Condition::KeywordContains(vec!["studio ghibli".into()]),
            ],
        );
        r.match_mode = MatchMode::Any;

        let result = evaluate(&[r.clone()], None);
        assert_eq!(result.winner.unwrap().category, "concerts");

        r.conditions = vec![
            Condition::GenreContains(vec!["Music".into()]),
            Condition::KeywordContains(vec!["concert film".into()]),
        ];
        assert!(evaluate(&[r], None).winner.is_none(), "an any rule with nothing holding matched");
    }

    /// Explanation lines, rendered the way the API does.
    fn reasons_of(m: &RuleMatch) -> Vec<String> {
        crate::localization::Localizer::new("en")
            .describe_all(&m.evaluations, m.excluded_by.as_ref())
    }

    #[test]
    fn any_mode_still_explains_the_conditions_that_failed() {
        let mut r = rule(
            "concerts",
            10,
            "concerts",
            vec![
                Condition::GenreContains(vec!["Music".into()]),
                Condition::KeywordContains(vec!["studio ghibli".into()]),
            ],
        );
        r.match_mode = MatchMode::Any;

        let winner = evaluate(&[r], None).winner.unwrap();
        assert_eq!(winner.evaluations.len(), 2);
        assert!(reasons_of(&winner).iter().any(|reason| reason.starts_with('✗')));
    }

    // ---------------------------------------------------------- exclusions

    #[test]
    fn an_exclusion_vetoes_a_matching_rule() {
        let mut r = anime_rule();
        r.exclusions = vec![Condition::GenreContains(vec!["Family".into()])];

        let result = evaluate(&[r], None);
        assert!(result.winner.is_none());
        assert_eq!(result.excluded.len(), 1);
        assert!(result.excluded[0].excluded_by.is_some());
        assert!(
            reasons_of(&result.excluded[0]).last().unwrap().starts_with('⛔'),
            "the veto must be part of the explanation"
        );
    }

    /// Any one exclusion vetoes, wherever it sits in the list.
    #[test]
    fn the_second_exclusion_vetoes_as_well_as_the_first() {
        let mut r = anime_rule();
        r.exclusions = vec![
            Condition::GenreContains(vec!["Horror".into()]),
            Condition::KeywordContains(vec!["studio ghibli".into()]),
        ];

        let result = evaluate(&[r], None);
        assert!(result.winner.is_none(), "the second exclusion was ignored");
        let veto = result.excluded[0].excluded_by.as_ref().expect("a veto");
        assert_eq!(veto.kind, "keyword_contains");
    }

    #[test]
    fn a_non_matching_exclusion_leaves_the_rule_alone() {
        let mut r = anime_rule();
        r.exclusions = vec![Condition::GenreContains(vec!["Horror".into()])];
        assert_eq!(evaluate(&[r], None).winner.unwrap().category, "anime");
    }

    #[test]
    fn an_excluded_rule_lets_the_next_one_win() {
        let mut excluded = anime_rule();
        excluded.exclusions = vec![Condition::GenreContains(vec!["Family".into()])];
        let kids = rule("kids", 20, "kids", vec![Condition::GenreContains(vec!["Family".into()])]);

        let result = evaluate(&[excluded, kids], None);
        assert_eq!(result.winner.unwrap().category, "kids");
        assert_eq!(result.excluded.len(), 1);
    }

    // ---------------------------------------------------------- conditions

    fn matches(condition: Condition) -> bool {
        let media = media();
        let metadata = metadata();
        evaluate_single_condition(
            &condition,
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        )
        .matched
    }

    /// `condition` against the fixture as `adjust` leaves it.
    fn matches_on(adjust: impl FnOnce(&mut Media), condition: Condition) -> bool {
        let mut media = media();
        adjust(&mut media);
        let metadata = metadata();
        evaluate_single_condition(
            &condition,
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        )
        .matched
    }

    fn matches_without_metadata(condition: Condition) -> bool {
        let media = media();
        evaluate_single_condition(
            &condition,
            EvalContext { media: &media, metadata: None, now: now() },
        )
        .matched
    }

    #[test]
    fn genre_matching_is_case_insensitive() {
        assert!(matches(Condition::GenreContains(vec!["animation".into()])));
        assert!(matches(Condition::GenreContains(vec!["  ANIMATION  ".into()])));
    }

    #[test]
    fn negative_genre_condition_is_the_inverse() {
        assert!(!matches(Condition::GenreNotContains(vec!["Animation".into()])));
        assert!(matches(Condition::GenreNotContains(vec!["Horror".into()])));
        // Any one of the values present refuses, not all of them.
        assert!(!matches(Condition::GenreNotContains(vec!["Horror".into(), "Animation".into()])));
    }

    #[test]
    fn keyword_conditions_work_on_the_keyword_list() {
        assert!(matches(Condition::KeywordContains(vec!["Studio Ghibli".into()])));
        assert!(matches(Condition::KeywordNotContains(vec!["concert".into()])));
        assert!(!matches(Condition::KeywordNotContains(vec!["studio ghibli".into()])));
        // A genre is not a keyword.
        assert!(matches(Condition::KeywordNotContains(vec!["Animation".into()])));
    }

    #[test]
    fn an_empty_value_list_never_matches() {
        assert!(!matches(Condition::GenreContains(vec![])));
        assert!(!matches(Condition::GenreContains(vec!["   ".into()])));
    }

    #[test]
    fn language_conditions_are_symmetric() {
        assert!(matches(Condition::OriginalLanguage(vec!["JA".into()])));
        assert!(!matches(Condition::OriginalLanguageNot(vec!["ja".into()])));
        assert!(matches(Condition::OriginalLanguageNot(vec!["en".into()])));
    }

    #[test]
    fn origin_country_matches() {
        assert!(matches(Condition::OriginCountry(vec!["jp".into()])));
        assert!(!matches(Condition::OriginCountry(vec!["US".into()])));
    }

    #[test]
    fn year_range_handles_open_bounds() {
        assert!(matches(Condition::YearRange { min: Some(1980), max: Some(1990) }));
        assert!(matches(Condition::YearRange { min: Some(1980), max: None }));
        assert!(matches(Condition::YearRange { min: None, max: Some(1990) }));
        assert!(!matches(Condition::YearRange { min: Some(1990), max: None }));
        // The fixture's year is 1988: both bounds include it, neither reaches past it.
        assert!(matches(Condition::YearRange { min: Some(1988), max: Some(1988) }));
        assert!(!matches(Condition::YearRange { min: None, max: Some(1987) }));
        assert!(!matches(Condition::YearRange { min: Some(1989), max: None }));
    }

    #[test]
    fn an_unknown_year_never_satisfies_a_range() {
        let mut media = media();
        media.year = None;
        let metadata = metadata();
        let outcome = evaluate_single_condition(
            &Condition::YearRange { min: None, max: None },
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        );
        assert!(!outcome.matched, "an open range must not classify media with no year");
        assert!(outcome.observed.is_empty(), "the engine reports the raw value, not prose");
        // The blank is turned into a word at render time, in the right language.
        let english = crate::localization::Localizer::new("en").describe(&outcome);
        assert!(english.contains("unknown"), "{english}");
        let french = crate::localization::Localizer::new("fr").describe(&outcome);
        assert!(french.contains("inconnu"), "{french}");
    }

    #[test]
    fn certification_requires_a_value() {
        assert!(matches(Condition::CertificationIn(vec!["G".into()])));
        assert!(!matches(Condition::CertificationIn(vec!["PG-13".into()])));
        // Media with no metadata must not match an empty certification.
        assert!(!matches_without_metadata(Condition::CertificationIn(vec!["".into()])));
    }

    #[test]
    fn root_folder_comparison_ignores_trailing_slashes() {
        assert!(matches(Condition::CurrentRootFolder("/movies/standard/".into())));
        assert!(matches(Condition::CurrentRootFolder("/movies/standard".into())));
        assert!(!matches(Condition::CurrentRootFolder("/movies/anime".into())));
    }

    /// An item with no language has none, not the empty string: a blank value
    /// in a language list reaches no such item, and excludes none.
    #[test]
    fn a_blank_language_value_reaches_no_item_without_a_language() {
        let with_blank = vec!["ja".to_string(), String::new()];
        assert!(!matches_without_metadata(Condition::OriginalLanguage(with_blank.clone())));
        assert!(matches_without_metadata(Condition::OriginalLanguageNot(with_blank)));
    }

    /// A prefix is a folder: `/movies` holds `/movies/standard`, and
    /// `/movies/stand` is not a folder of it but a sibling sharing its letters,
    /// as `/data/movies-4k` is to `/data/movies`.
    #[test]
    fn a_folder_prefix_does_not_match_a_sibling_sharing_its_letters() {
        assert!(!matches(Condition::CurrentRootFolderStartsWith("/movies/stand".into())));
        assert!(matches(Condition::CurrentRootFolderStartsWith("/movies/standard".into())));
        assert!(matches(Condition::CurrentRootFolderStartsWith("/movies/".into())));
        assert!(matches(Condition::CurrentRootFolderStartsWith("/".into())));
    }

    #[test]
    fn root_folder_prefix_condition() {
        assert!(matches(Condition::CurrentRootFolderStartsWith("/movies".into())));
        assert!(!matches(Condition::CurrentRootFolderStartsWith("/series".into())));
    }

    #[test]
    fn boolean_conditions() {
        assert!(matches(Condition::HasFiles(true)));
        assert!(!matches(Condition::HasFiles(false)));
        assert!(matches(Condition::Monitored(true)));
        assert!(matches(Condition::HasMetadata(true)));
        assert!(matches_without_metadata(Condition::HasMetadata(false)));
        // The other side of each, on a title where the field differs.
        assert!(matches_on(|m| m.has_files = false, Condition::HasFiles(false)));
        assert!(!matches_on(|m| m.has_files = false, Condition::HasFiles(true)));
        assert!(!matches(Condition::Monitored(false)));
        assert!(matches_on(|m| m.monitored = false, Condition::Monitored(false)));
        assert!(!matches_on(|m| m.monitored = false, Condition::Monitored(true)));
        assert!(!matches(Condition::HasMetadata(false)));
        assert!(!matches_without_metadata(Condition::HasMetadata(true)));
    }

    #[test]
    fn title_condition_is_case_insensitive_substring() {
        assert!(matches(Condition::TitleContains(vec!["totoro".into()])));
        assert!(matches(Condition::TitleContains(vec!["NEIGHBOR".into()])));
        assert!(!matches(Condition::TitleContains(vec!["akira".into()])));
    }

    /// A title goes through `normalise_value` like every other string
    /// condition. Compared with `to_lowercase` alone, it would fold case and
    /// keep the accents, against the contract the engine states about itself.
    #[test]
    fn a_title_is_matched_the_way_every_other_value_is() {
        let accented = |needle: &str| {
            let mut media = media();
            media.title = "Amélie".to_string();
            evaluate_single_condition(
                &Condition::TitleContains(vec![needle.to_string()]),
                EvalContext { media: &media, metadata: None, now: now() },
            )
            .matched
        };
        assert!(accented("amelie"), "an unaccented needle has to find an accented title");
        assert!(accented("Amélie"), "and the accented one still does");

        // Punctuation collapses to a space on both sides, so the separator a
        // person happened to type stops deciding the answer.
        let punctuated = |needle: &str| {
            let mut media = media();
            media.title = "Spider-Man: No Way Home".to_string();
            evaluate_single_condition(
                &Condition::TitleContains(vec![needle.to_string()]),
                EvalContext { media: &media, metadata: None, now: now() },
            )
            .matched
        };
        assert!(punctuated("spider man"));
        assert!(punctuated("Spider-Man"));
        assert!(punctuated("no way home"));

        // Widened, not loosened: an unrelated title still does not match.
        assert!(!punctuated("batman"));
    }

    #[test]
    fn external_id_conditions_target_single_items() {
        assert!(matches(Condition::TmdbIdIn(vec![8392, 1])));
        assert!(!matches(Condition::TmdbIdIn(vec![1])));
        assert!(matches(Condition::ImdbIdIn(vec!["TT0096283".into()])));
        assert!(!matches(Condition::ImdbIdIn(vec!["tt0000001".into()])));
        assert!(!matches(Condition::TvdbIdIn(vec![1])), "media has no tvdb id");
        assert!(matches_on(|m| m.tvdb_id = Some(76885), Condition::TvdbIdIn(vec![1, 76885])));
        assert!(!matches_on(|m| m.tvdb_id = Some(76885), Condition::TvdbIdIn(vec![1])));
    }

    #[test]
    fn added_within_days_uses_the_injected_clock() {
        assert!(matches(Condition::AddedWithinDays(7)));
        // Added 26 hours before the clock: one whole day, not zero.
        assert!(matches(Condition::AddedWithinDays(1)));
        assert!(!matches(Condition::AddedWithinDays(0)));

        let mut old = media();
        old.added_at = Some("2020-01-01 00:00:00".into());
        let outcome = evaluate_single_condition(
            &Condition::AddedWithinDays(7),
            EvalContext { media: &old, metadata: None, now: now() },
        );
        assert!(!outcome.matched);
    }

    #[test]
    fn added_within_days_handles_rfc3339_from_arr() {
        let mut m = media();
        m.added_at = Some("2026-08-19T08:30:00Z".into());
        let outcome = evaluate_single_condition(
            &Condition::AddedWithinDays(7),
            EvalContext { media: &m, metadata: None, now: now() },
        );
        assert!(outcome.matched);
    }

    #[test]
    fn an_unparseable_timestamp_does_not_match() {
        let mut m = media();
        m.added_at = Some("not a date".into());
        let outcome = evaluate_single_condition(
            &Condition::AddedWithinDays(3650),
            EvalContext { media: &m, metadata: None, now: now() },
        );
        assert!(!outcome.matched);
    }

    #[test]
    fn metadata_conditions_do_not_match_when_metadata_is_missing() {
        assert!(!matches_without_metadata(Condition::GenreContains(vec!["Animation".into()])));
        assert!(!matches_without_metadata(Condition::OriginalLanguage(vec!["ja".into()])));
        // The negative form is vacuously true, which is the useful behaviour for
        // a "not anime" rule on an item TMDb knows nothing about.
        assert!(matches_without_metadata(Condition::GenreNotContains(vec!["Animation".into()])));
    }

    // ---------------------------------------------------------- explanation

    #[test]
    fn a_matched_condition_shows_what_it_found() {
        let winner = evaluate(&[anime_rule()], None).winner.unwrap();
        let reasons = reasons_of(&winner);
        assert_eq!(reasons.len(), 2);
        assert!(reasons.iter().all(|reason| reason.starts_with('✓')));
        assert!(reasons[0].contains("Original language"));
        assert!(reasons[0].contains("found [ja]"), "the observed value must be shown: {reasons:?}");
    }

    /// The condition after `previous` in a walk over every kind, `None` once
    /// all are visited. The match has no wildcard, so a kind added to
    /// `Condition` does not compile here until it has its place in the walk.
    fn next_kind(previous: Option<&Condition>) -> Option<Condition> {
        use Condition::*;
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        Some(match previous {
            None => GenreContains(list(&["Animation"])),
            Some(GenreContains(_)) => GenreContainsAll(list(&["Animation", "Family"])),
            Some(GenreContainsAll(_)) => GenreNotContains(list(&["Horror"])),
            Some(GenreNotContains(_)) => KeywordContains(list(&["anime"])),
            Some(KeywordContains(_)) => KeywordContainsAll(list(&["anime", "studio ghibli"])),
            Some(KeywordContainsAll(_)) => KeywordNotContains(list(&["horror"])),
            Some(KeywordNotContains(_)) => OriginalLanguage(list(&["ja"])),
            Some(OriginalLanguage(_)) => OriginalLanguageNot(list(&["en"])),
            Some(OriginalLanguageNot(_)) => OriginCountry(list(&["JP"])),
            Some(OriginCountry(_)) => OriginCountryAll(list(&["JP"])),
            Some(OriginCountryAll(_)) => YearRange { min: Some(1980), max: Some(1990) },
            Some(YearRange { .. }) => TagIn(list(&["anime"])),
            Some(TagIn(_)) => TagInAll(list(&["anime"])),
            Some(TagInAll(_)) => SeriesTypeIs(list(&["anime"])),
            Some(SeriesTypeIs(_)) => SizeOnDiskOverGb(1),
            Some(SizeOnDiskOverGb(_)) => SeasonCountOver(1),
            Some(SeasonCountOver(_)) => CertificationIn(list(&["G"])),
            Some(CertificationIn(_)) => StatusIs(list(&["released"])),
            Some(StatusIs(_)) => CurrentRootFolder("/movies/standard".into()),
            Some(CurrentRootFolder(_)) => CurrentRootFolderStartsWith("/movies".into()),
            Some(CurrentRootFolderStartsWith(_)) => HasFiles(true),
            Some(HasFiles(_)) => TitleContains(list(&["Totoro"])),
            Some(TitleContains(_)) => TmdbIdIn(vec![8392]),
            Some(TmdbIdIn(_)) => TvdbIdIn(vec![76885]),
            Some(TvdbIdIn(_)) => ImdbIdIn(list(&["tt0096283"])),
            Some(ImdbIdIn(_)) => AddedWithinDays(7),
            Some(AddedWithinDays(_)) => Monitored(true),
            Some(Monitored(_)) => HasMetadata(true),
            Some(HasMetadata(_)) => return None,
        })
    }

    /// Every kind explains itself in words: its key is in the dictionary and
    /// every placeholder of the sentence is filled. A key nobody wrote comes
    /// back as the key itself, and a parameter the engine stopped sending
    /// leaves `{values}` on the panel.
    #[test]
    fn every_condition_produces_a_reason() {
        let (media, metadata) = (media(), metadata());
        let english = crate::localization::Localizer::new("en");
        let mut seen = std::collections::HashSet::new();
        let mut condition = next_kind(None);
        while let Some(current) = condition {
            let outcome = evaluate_single_condition(
                &current,
                EvalContext { media: &media, metadata: Some(&metadata), now: now() },
            );
            let reason = english.describe(&outcome);
            assert!(!reason.contains(&outcome.key), "{} has no sentence: {reason}", outcome.kind);
            assert!(!reason.contains('{'), "{} left a placeholder: {reason}", outcome.kind);
            assert!(seen.insert(current.kind()), "{} is visited twice", current.kind());
            condition = next_kind(Some(&current));
        }
        assert_eq!(seen.len(), 28, "the walk visited {} kinds", seen.len());
    }

    #[test]
    fn condition_outcomes_carry_expected_and_observed() {
        let media = media();
        let metadata = metadata();
        let outcome = evaluate_single_condition(
            &Condition::GenreContains(vec!["Animation".into()]),
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        );
        assert_eq!(outcome.kind, "genre_contains");
        assert_eq!(outcome.key, "ConditionGenreContains");
        assert_eq!(outcome.params.get("values").unwrap(), "Animation");
        assert!(outcome.observed.contains("Family"));
        assert!(
            outcome.expected.is_empty(),
            "wording is resolved by the localizer, not the engine"
        );
    }

    #[test]
    fn confidence_grows_with_agreeing_signals() {
        let one =
            rule("one", 10, "anime", vec![Condition::GenreContains(vec!["Animation".into()])]);
        let two = anime_rule();

        let low = evaluate(&[one], None).winner.unwrap().confidence;
        let high = evaluate(&[two], None).winner.unwrap().confidence;
        assert!(high > low, "{high} should exceed {low}");
        assert!(high <= 1.0 && low > 0.0);
    }

    #[test]
    fn any_mode_confidence_is_capped_below_all_mode() {
        let mut any = anime_rule();
        any.match_mode = MatchMode::Any;
        let strict = evaluate(&[anime_rule()], None).winner.unwrap().confidence;
        let loose = evaluate(&[any], None).winner.unwrap().confidence;
        assert!(loose < strict);
    }

    // ---------------------------------------------------------- serde

    #[test]
    fn conditions_round_trip_through_json() {
        let conditions = vec![
            Condition::GenreContains(vec!["Animation".into()]),
            Condition::YearRange { min: Some(1980), max: None },
            Condition::HasFiles(true),
            Condition::AddedWithinDays(7),
        ];
        let json = serde_json::to_string(&conditions).unwrap();
        let back: Vec<Condition> = serde_json::from_str(&json).unwrap();
        assert_eq!(conditions, back);
    }
}
