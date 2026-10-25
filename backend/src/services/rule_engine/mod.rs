//! Pure, synchronous routing decision logic, free of I/O so a simulation does no
//! per-item query.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::cell::OnceCell;
use std::collections::BTreeMap;

mod folding;
mod validation;

pub use folding::{certification_key, normalise_value, status_key};
use folding::{contains_all, contains_any};
pub use validation::{MAX_NAME_LENGTH, MAX_PRIORITY, RuleDraft, ValidationEnv, validate_rule};

use crate::models::{Condition, MatchMode, Media, MediaMetadata, Rule};

/// One condition of a rule, held against a title.
// Keys and parameters, never prose: `Localizer::describe` words them at the edges.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ConditionOutcome {
    pub kind: String,
    pub matched: bool,
    /// Translation key describing what the rule asked for.
    pub key: String,
    /// Placeholder values for `key`.
    pub params: BTreeMap<String, String>,
    /// What the title carried, as the reader's language writes it. Empty
    /// when it carried nothing.
    // Stored raw (bytes, `true`) and written for the reader by `Localizer::localize_outcome`.
    pub observed: String,
    /// What the condition asked for, in the interface language.
    // Filled in by `Localizer::localize_outcome`.
    #[serde(default)]
    pub expected: String,
    /// Which metadata source supplied `observed`, absent for a condition the
    /// Arr answers directly.
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
    /// Confidence from 0.0 to 1.0, driven by how many independent signals agreed.
    pub confidence: f32,
    /// The exclusion that vetoed an otherwise-matching rule.
    pub excluded_by: Option<ConditionOutcome>,
}

/// The rules that apply to `media`, enabled and covering its type and instance,
/// lowest priority first, then by name, then by id, since two rules may share a
/// name. The order never depends on the one SQLite returns.
pub fn in_order<'a>(rules: &'a [Rule], media: &Media) -> Vec<&'a Rule> {
    let mut applicable: Vec<&Rule> =
        rules.iter().filter(|r| r.enabled && applies(r, media)).collect();
    applicable.sort_by(tried_before);
    applicable
}

fn applies(rule: &Rule, media: &Media) -> bool {
    rule.covers_media_type(&media.media_type) && rule.covers_instance(&media.instance_id)
}

fn tried_before(a: &&Rule, b: &&Rule) -> std::cmp::Ordering {
    a.priority.cmp(&b.priority).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id))
}

/// A rule set ready to be held against many titles: its enabled rules in the order
/// [`in_order`] gives, each condition's values folded once rather than once per title.
pub struct Prepared<'a> {
    rules: Vec<PreparedRule<'a>>,
}

struct PreparedRule<'a> {
    rule: &'a Rule,
    conditions: Vec<Vec<String>>,
    exclusions: Vec<Vec<String>>,
}

impl<'a> Prepared<'a> {
    pub fn new(rules: &'a [Rule]) -> Self {
        let mut enabled: Vec<&Rule> = rules.iter().filter(|r| r.enabled).collect();
        enabled.sort_by(tried_before);
        let fold = |list: &[Condition]| list.iter().map(folded_values).collect();
        let rules = enabled
            .into_iter()
            .map(|rule| PreparedRule {
                rule,
                conditions: fold(&rule.conditions),
                exclusions: fold(&rule.exclusions),
            })
            .collect();
        Self { rules }
    }
}

/// The values of a condition compared through [`normalise_value`], folded, blanks dropped,
/// and none for a kind compared otherwise.
fn folded_values(condition: &Condition) -> Vec<String> {
    use Condition::*;
    match condition {
        GenreContains(values)
        | GenreContainsAll(values)
        | GenreNotContains(values)
        | KeywordContains(values)
        | KeywordContainsAll(values)
        | KeywordNotContains(values)
        | OriginCountry(values)
        | OriginCountryAll(values)
        | TagIn(values)
        | TagInAll(values)
        | QualityProfileIn(values)
        | StudioIn(values)
        | NetworkIn(values)
        | CollectionIn(values)
        | TitleContains(values) => {
            values.iter().map(|v| normalise_value(v)).filter(|v| !v.is_empty()).collect()
        }
        OriginalLanguage(_)
        | OriginalLanguageNot(_)
        | OriginalLanguageKnown(_)
        | YearRange { .. }
        | SeriesTypeIs(_)
        | RuntimeRange { .. }
        | ScoreRange { .. }
        | SizeOnDiskOverGb(_)
        | SeasonCountOver(_)
        | CertificationIn(_)
        | StatusIs(_)
        | CurrentRootFolder(_)
        | CurrentRootFolderStartsWith(_)
        | HasFiles(_)
        | TmdbIdIn(_)
        | TvdbIdIn(_)
        | ImdbIdIn(_)
        | AddedWithinDays(_)
        | Monitored(_)
        | HasMetadata(_) => Vec::new(),
    }
}

/// One title as an evaluation reads it, each of its values folded on its first read, so
/// every condition reading genres shares one fold of them.
struct Reading<'a> {
    ctx: EvalContext<'a>,
    genres: OnceCell<Vec<String>>,
    keywords: OnceCell<Vec<String>>,
    countries: OnceCell<Vec<String>>,
    tags: OnceCell<(Vec<String>, Vec<String>)>,
    title: OnceCell<String>,
    profile: OnceCell<Vec<String>>,
    studio: OnceCell<Vec<String>>,
    network: OnceCell<Vec<String>>,
    collection: OnceCell<Vec<String>>,
}

fn fold_all(values: &[String]) -> Vec<String> {
    values.iter().map(|v| normalise_value(v)).collect()
}

impl<'a> Reading<'a> {
    fn new(ctx: EvalContext<'a>) -> Self {
        Self {
            ctx,
            genres: OnceCell::new(),
            keywords: OnceCell::new(),
            countries: OnceCell::new(),
            tags: OnceCell::new(),
            title: OnceCell::new(),
            profile: OnceCell::new(),
            studio: OnceCell::new(),
            network: OnceCell::new(),
            collection: OnceCell::new(),
        }
    }

    fn genres(&self) -> &[String] {
        self.ctx.metadata.map(|m| m.genres.as_slice()).unwrap_or_default()
    }

    fn keywords(&self) -> &[String] {
        self.ctx.metadata.map(|m| m.keywords.as_slice()).unwrap_or_default()
    }

    fn countries(&self) -> &[String] {
        self.ctx.metadata.map(|m| m.origin_countries.as_slice()).unwrap_or_default()
    }

    fn folded_genres(&self) -> &[String] {
        self.genres.get_or_init(|| fold_all(self.genres()))
    }

    fn folded_keywords(&self) -> &[String] {
        self.keywords.get_or_init(|| fold_all(self.keywords()))
    }

    fn folded_countries(&self) -> &[String] {
        self.countries.get_or_init(|| fold_all(self.countries()))
    }

    /// The tags as the Arr names them, and folded.
    fn tags(&self) -> &(Vec<String>, Vec<String>) {
        self.tags.get_or_init(|| {
            let tags = self.ctx.media.tag_labels();
            let folded = fold_all(&tags);
            (tags, folded)
        })
    }

    fn folded_title(&self) -> &str {
        self.title.get_or_init(|| normalise_value(&self.ctx.media.title))
    }

    fn folded_one<'c>(cell: &'c OnceCell<Vec<String>>, value: &Option<String>) -> &'c [String] {
        cell.get_or_init(|| value.iter().map(|v| normalise_value(v)).collect())
    }
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

/// Evaluate all rules against a single media item: the first to hold wins, and
/// the rest are reported as alternatives the explanation shows.
pub fn evaluate_prepared(
    ctx: EvalContext<'_>,
    rules: &Prepared<'_>,
    override_category: Option<&str>,
) -> Evaluation {
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
    let reading = Reading::new(ctx);

    for prepared in rules.rules.iter().filter(|prepared| applies(prepared.rule, ctx.media)) {
        let rule = prepared.rule;
        let (matched, evaluations) =
            hold(&rule.conditions, &prepared.conditions, rule.match_mode, &reading);
        if !matched {
            continue;
        }

        let veto = first_veto(&rule.exclusions, &prepared.exclusions, &reading);

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

/// The exclusion that sets aside `rule`, whose conditions hold. Read only
/// once the rule would otherwise fire, and any one of them vetoes it.
pub fn veto(rule: &Rule, ctx: EvalContext<'_>) -> Option<ConditionOutcome> {
    let folded: Vec<Vec<String>> = rule.exclusions.iter().map(folded_values).collect();
    first_veto(&rule.exclusions, &folded, &Reading::new(ctx))
}

fn first_veto(
    exclusions: &[Condition],
    folded: &[Vec<String>],
    reading: &Reading<'_>,
) -> Option<ConditionOutcome> {
    exclusions
        .iter()
        .zip(folded)
        .map(|(condition, wanted)| evaluate_single_condition(condition, wanted, reading))
        .find(|outcome| outcome.matched)
}

/// Confidence heuristic: more agreeing signals make a more trustworthy call. `any`
/// mode is capped lower, since one weak signal can carry it.
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

/// Evaluate the conditions of a rule under its match mode, every one even in `any`
/// mode, since the failures are part of the explanation.
pub(crate) fn evaluate_conditions(
    conditions: &[Condition],
    mode: MatchMode,
    ctx: EvalContext<'_>,
) -> (bool, Vec<ConditionOutcome>) {
    let folded: Vec<Vec<String>> = conditions.iter().map(folded_values).collect();
    hold(conditions, &folded, mode, &Reading::new(ctx))
}

fn hold(
    conditions: &[Condition],
    folded: &[Vec<String>],
    mode: MatchMode,
    reading: &Reading<'_>,
) -> (bool, Vec<ConditionOutcome>) {
    if conditions.is_empty() {
        return (false, vec![]);
    }

    let outcomes: Vec<ConditionOutcome> = conditions
        .iter()
        .zip(folded)
        .map(|(condition, wanted)| evaluate_single_condition(condition, wanted, reading))
        .collect();

    let matched = match mode {
        MatchMode::All => outcomes.iter().all(|o| o.matched),
        MatchMode::Any => outcomes.iter().any(|o| o.matched),
    };

    (matched, outcomes)
}

/// One condition held against `reading`, `wanted` its values as [`folded_values`] folds them,
/// answering the key and parameters of its wording, never the wording.
fn evaluate_single_condition(
    condition: &Condition,
    wanted: &[String],
    reading: &Reading<'_>,
) -> ConditionOutcome {
    let ctx = reading.ctx;
    let media = ctx.media;
    let metadata = ctx.metadata;
    let kind = condition.kind();

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
            let matched = contains_any(reading.folded_genres(), wanted);
            list(kind, "ConditionGenreContains", matched, values, reading.genres())
        }

        Condition::GenreContainsAll(values) => {
            let matched = contains_all(reading.folded_genres(), wanted);
            list(kind, "ConditionGenreContainsAll", matched, values, reading.genres())
        }

        Condition::GenreNotContains(values) => {
            let matched = !contains_any(reading.folded_genres(), wanted);
            list(kind, "ConditionGenreNotContains", matched, values, reading.genres())
        }

        Condition::KeywordContains(values) => {
            let matched = contains_any(reading.folded_keywords(), wanted);
            list(kind, "ConditionKeywordContains", matched, values, reading.keywords())
        }

        Condition::KeywordContainsAll(values) => {
            let matched = contains_all(reading.folded_keywords(), wanted);
            list(kind, "ConditionKeywordContainsAll", matched, values, reading.keywords())
        }

        Condition::KeywordNotContains(values) => {
            let matched = !contains_any(reading.folded_keywords(), wanted);
            list(kind, "ConditionKeywordNotContains", matched, values, reading.keywords())
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

        Condition::OriginalLanguageKnown(expected) => {
            let known = metadata
                .and_then(|m| m.original_language.as_deref())
                .is_some_and(|lang| !lang.is_empty());
            ConditionOutcome::new(
                kind,
                known == *expected,
                "ConditionOriginalLanguageKnown",
                &[("value", expected.to_string())],
                known.to_string(),
            )
        }

        Condition::OriginCountry(values) => {
            let matched = contains_any(reading.folded_countries(), wanted);
            list(kind, "ConditionOriginCountry", matched, values, reading.countries())
        }

        Condition::OriginCountryAll(values) => {
            let matched = contains_all(reading.folded_countries(), wanted);
            list(kind, "ConditionOriginCountryAll", matched, values, reading.countries())
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
            let held = certification_key(cert);
            ConditionOutcome::new(
                kind,
                !held.is_empty() && values.iter().any(|value| certification_key(value) == held),
                "ConditionCertificationIn",
                &[("values", values.join(", "))],
                cert.to_string(),
            )
        }

        Condition::StatusIs(values) => {
            let status = media.status.as_deref().unwrap_or("");
            let held = status_key(status);
            ConditionOutcome::new(
                kind,
                !held.is_empty() && values.iter().any(|value| status_key(value) == held),
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

        Condition::TagIn(values) => {
            let (tags, folded) = reading.tags();
            list(kind, "ConditionTagIn", contains_any(folded, wanted), values, tags)
        }

        Condition::TagInAll(values) => {
            let (tags, folded) = reading.tags();
            list(kind, "ConditionTagInAll", contains_all(folded, wanted), values, tags)
        }

        Condition::SeriesTypeIs(values) => {
            let actual = media.series_type.as_deref().unwrap_or("").trim().to_lowercase();
            ConditionOutcome::new(
                kind,
                // An absent series type never matches, movies included.
                !actual.is_empty() && values.iter().any(|v| v.trim().eq_ignore_ascii_case(&actual)),
                "ConditionSeriesTypeIs",
                &[("values", values.join(", "))],
                actual,
            )
        }

        Condition::QualityProfileIn(values) => {
            let profile: Vec<String> = media.quality_profile.iter().cloned().collect();
            let folded = Reading::folded_one(&reading.profile, &media.quality_profile);
            list(kind, "ConditionQualityProfileIn", contains_any(folded, wanted), values, &profile)
        }

        Condition::RuntimeRange { min, max } => {
            // An unknown runtime must not satisfy an open-ended range, as a year must not.
            let matched = media.runtime.is_some_and(|minutes| {
                min.is_none_or(|m| minutes >= m) && max.is_none_or(|m| minutes <= m)
            });
            ConditionOutcome::new(
                kind,
                matched,
                "ConditionRuntimeRange",
                &[
                    ("min", min.map(|v| v.to_string()).unwrap_or_else(|| "*".into())),
                    ("max", max.map(|v| v.to_string()).unwrap_or_else(|| "*".into())),
                ],
                media.runtime.map(|minutes| minutes.to_string()).unwrap_or_default(),
            )
        }

        Condition::StudioIn(values) => {
            let studio: Vec<String> = media.studio.iter().cloned().collect();
            let folded = Reading::folded_one(&reading.studio, &media.studio);
            list(kind, "ConditionStudioIn", contains_any(folded, wanted), values, &studio)
        }

        Condition::NetworkIn(values) => {
            let network: Vec<String> = media.network.iter().cloned().collect();
            let folded = Reading::folded_one(&reading.network, &media.network);
            list(kind, "ConditionNetworkIn", contains_any(folded, wanted), values, &network)
        }

        Condition::CollectionIn(values) => {
            let collection: Vec<String> = media.collection.iter().cloned().collect();
            let folded = Reading::folded_one(&reading.collection, &media.collection);
            list(kind, "ConditionCollectionIn", contains_any(folded, wanted), values, &collection)
        }

        Condition::ScoreRange { scale, min, max, min_votes } => {
            let rating = metadata.and_then(|m| m.ratings.get(scale));
            let matched = rating.is_some_and(|rating| {
                min.is_none_or(|m| rating.value >= m)
                    && max.is_none_or(|m| rating.value <= m)
                    && min_votes.is_none_or(|n| rating.votes.is_some_and(|votes| votes >= n))
            });
            let named = crate::models::scale(scale).map_or(scale.as_str(), |known| known.name);
            let bound = |bound: &Option<f64>| bound.map_or_else(|| "*".into(), |v| v.to_string());
            let key = if min_votes.is_some() {
                "ConditionScoreRangeVotes"
            } else {
                "ConditionScoreRange"
            };
            ConditionOutcome::new(
                kind,
                matched,
                key,
                &[
                    ("scale", named.to_string()),
                    ("min", bound(min)),
                    ("max", bound(max)),
                    ("votes", min_votes.map(|votes| votes.to_string()).unwrap_or_default()),
                ],
                rating
                    .map(|rating| match rating.votes {
                        Some(votes) => format!("{} ({votes})", rating.value),
                        None => rating.value.to_string(),
                    })
                    .unwrap_or_default(),
            )
        }

        Condition::SizeOnDiskOverGb(threshold) => {
            const GB: i64 = 1024 * 1024 * 1024;
            ConditionOutcome::new(
                kind,
                media.size_on_disk.is_some_and(|size| size > threshold.saturating_mul(GB)),
                "ConditionSizeOnDiskOverGb",
                &[("value", threshold.to_string())],
                media.size_on_disk.map(|bytes| bytes.to_string()).unwrap_or_default(),
            )
        }

        Condition::SeasonCountOver(threshold) => ConditionOutcome::new(
            kind,
            media.season_count.is_some_and(|count| count > *threshold),
            "ConditionSeasonCountOver",
            &[("value", threshold.to_string())],
            media.season_count.map(|count| count.to_string()).unwrap_or_default(),
        ),

        Condition::TitleContains(values) => {
            // Normalised like every string condition, so `amelie` finds "Amélie".
            let title = reading.folded_title();
            ConditionOutcome::new(
                kind,
                wanted.iter().any(|v| title.contains(v.as_str())),
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
            let added =
                media.added_at.as_deref().and_then(crate::services::routing::parse_timestamp);
            ConditionOutcome::new(
                kind,
                added.is_some_and(|added| added_within(ctx.now - added, *days)),
                "ConditionAddedWithinDays",
                &[("value", days.to_string())],
                // The age at `now`, what the condition compares, as an ISO 8601 duration.
                added
                    .map(|added| format!("PT{}M", (ctx.now - added).num_minutes()))
                    .unwrap_or_default(),
            )
        }
    };

    if let Some(field) = condition.metadata_field() {
        outcome.source = metadata.and_then(|m| m.source_of(field.as_str())).map(str::to_string);
    }
    // A scale's own source, which may not be the one that gave the first score.
    if let Condition::ScoreRange { scale, .. } = condition {
        outcome.source =
            metadata.and_then(|m| m.source_of(&format!("ratings.{scale}"))).map(str::to_string);
    }

    outcome
}

/// How far ahead of Routarr's clock an Arr's may run: a title it dates that
/// much in the future has just been added.
const CLOCK_AHEAD: chrono::TimeDelta = chrono::TimeDelta::days(1);

/// Whether a title `age` old was added within the last `days` days of 24
/// hours. A count past what a duration holds reaches every past date.
fn added_within(age: chrono::TimeDelta, days: i64) -> bool {
    age >= -CLOCK_AHEAD && chrono::TimeDelta::try_days(days).is_none_or(|window| age <= window)
}

/// Whether a language is among `values`. A missing language is absent, never
/// the empty string, so a blank value in the list reaches no item.
fn language_listed(lang: &str, values: &[String]) -> bool {
    !lang.is_empty() && values.iter().any(|v| v.trim().eq_ignore_ascii_case(lang))
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Condition, MatchMode, Media, ProviderMetadata, Rule, RuleMediaType};
    use chrono::TimeZone;

    fn evaluate_one(condition: &Condition, ctx: EvalContext<'_>) -> ConditionOutcome {
        evaluate_single_condition(condition, &folded_values(condition), &Reading::new(ctx))
    }

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
            quality_profile: None,
            runtime: None,
            studio: None,
            network: None,
            collection: None,
            collection_tmdb_id: None,
            ratings: None,
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
                ratings: [(
                    "imdb".to_string(),
                    crate::models::Rating { value: 8.1, votes: Some(393_000) },
                )]
                .into(),
                ..Default::default()
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
            media_type: RuleMediaType::Both,
            conditions,
            exclusions: vec![],
            match_mode: MatchMode::All,
            target_category: category.into(),
            instance_ids: None,
            template: None,
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

    fn evaluate_rules(
        ctx: EvalContext<'_>,
        rules: &[Rule],
        override_category: Option<&str>,
    ) -> Evaluation {
        evaluate_prepared(ctx, &Prepared::new(rules), override_category)
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

        let first = evaluate(&[a.clone(), b.clone()], None).winner.unwrap().category;
        let second = evaluate(&[b, a], None).winner.unwrap().category;
        assert_eq!(first, second);
        assert_eq!(first, "cat-a");
    }

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
        series_only.media_type = RuleMediaType::Series;
        assert!(evaluate(&[series_only.clone()], None).winner.is_none());

        let mut movies_only = anime_rule();
        movies_only.media_type = RuleMediaType::Movie;
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
        assert!(genre_rule(
            vec![Condition::GenreContains(vec!["Horror".into(), "Animation".into()])],
            MatchMode::All,
        ));
        assert!(!genre_rule(
            vec![Condition::GenreContains(vec!["Horror".into(), "Western".into()])],
            MatchMode::All,
        ));
    }

    /// Whether `condition` holds for `media` and the canonical metadata.
    fn holds(condition: Condition, media: &Media) -> bool {
        let metadata = metadata();
        evaluate_one(&condition, EvalContext { media, metadata: Some(&metadata), now: now() })
            .matched
    }

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

    #[test]
    fn a_status_matches_any_of_the_values_it_names() {
        assert!(holds(Condition::StatusIs(vec!["announced".into(), "Released".into()]), &media()));
        assert!(!holds(Condition::StatusIs(vec!["announced".into()]), &media()));
        let showing = Media { status: Some("inCinemas".into()), ..media() };
        assert!(holds(Condition::StatusIs(vec!["In Cinemas".into()]), &showing));
        let unknown = Media { status: None, ..media() };
        assert!(!holds(Condition::StatusIs(vec!["released".into()]), &unknown));
    }

    #[test]
    fn one_condition_can_require_every_genre_it_names() {
        assert!(genre_rule(
            vec![Condition::GenreContainsAll(vec!["Animation".into(), "Family".into()])],
            MatchMode::All,
        ));
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

    #[test]
    fn requiring_every_genre_of_nothing_matches_nothing() {
        assert!(!genre_rule(vec![Condition::GenreContainsAll(vec![])], MatchMode::All));
        assert!(Condition::GenreContainsAll(vec![]).is_empty());
    }

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
        // The `any` form is another kind, so reopening a stored rule keeps its meaning.
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
        let both_needed = vec![
            Condition::GenreContains(vec!["Animation".into()]),
            Condition::GenreContains(vec!["Western".into()]),
        ];
        assert!(!genre_rule(both_needed.clone(), MatchMode::All));
        assert!(genre_rule(both_needed, MatchMode::Any));
    }

    #[test]
    fn a_multi_genre_condition_combines_with_a_second_one() {
        assert!(genre_rule(
            vec![
                Condition::GenreContains(vec!["Fantasy".into(), "Horror".into()]),
                Condition::GenreContains(vec!["Animation".into()]),
            ],
            MatchMode::All,
        ));
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

    fn matches(condition: Condition) -> bool {
        let media = media();
        let metadata = metadata();
        evaluate_one(
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
        evaluate_one(
            &condition,
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        )
        .matched
    }

    fn matches_without_metadata(condition: Condition) -> bool {
        let media = media();
        evaluate_one(&condition, EvalContext { media: &media, metadata: None, now: now() }).matched
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
    fn a_score_range_reads_its_scale_and_its_votes_and_names_the_scale_s_source() {
        let (media, metadata) = (media(), metadata());
        let check = |scale: &str, min, max, min_votes| {
            let condition = Condition::ScoreRange { scale: scale.into(), min, max, min_votes };
            evaluate_one(
                &condition,
                EvalContext { media: &media, metadata: Some(&metadata), now: now() },
            )
        };
        assert!(check("imdb", Some(8.0), Some(8.1), None).matched);
        assert!(check("imdb", None, None, Some(393_000)).matched);
        assert!(!check("imdb", None, None, Some(393_001)).matched, "too few votes");
        assert!(!check("imdb", Some(8.2), None, None).matched);
        assert!(!check("tmdb", None, Some(10.0), None).matched, "an unrated scale matched");
        let outcome = check("imdb", Some(7.5), None, None);
        assert_eq!(
            (outcome.source.as_deref(), outcome.observed.as_str()),
            (Some("tmdb"), "8.1 (393000)")
        );
    }

    #[test]
    fn a_runtime_range_reads_minutes_and_an_unknown_runtime_never_matches() {
        let holds = |runtime, min, max| {
            let mut m = media();
            m.runtime = runtime;
            let condition = Condition::RuntimeRange { min, max };
            evaluate_one(&condition, EvalContext { media: &m, metadata: None, now: now() }).matched
        };
        assert!(holds(Some(86), Some(80), Some(90)));
        assert!(holds(Some(86), None, Some(86)));
        assert!(!holds(Some(86), Some(87), None));
        assert!(!holds(None, None, Some(1440)), "an unknown runtime met an open range");
    }

    #[test]
    fn a_profile_studio_network_or_collection_is_matched_as_spelled_any_way() {
        let mut m = media();
        m.quality_profile = Some("Ultra-HD".into());
        m.studio = Some("Studio Ghibli".into());
        m.network = Some("Fuji TV".into());
        m.collection = Some("Studio Ghibli Collection".into());
        let holds = |condition: Condition, m: &Media| {
            evaluate_one(&condition, EvalContext { media: m, metadata: None, now: now() }).matched
        };
        let one = |value: &str| vec![value.to_string()];
        assert!(holds(Condition::QualityProfileIn(one("ultra hd")), &m));
        assert!(holds(Condition::StudioIn(one("STUDIO GHIBLI")), &m));
        assert!(holds(Condition::NetworkIn(one("fuji-tv")), &m));
        assert!(holds(Condition::CollectionIn(one("studio ghibli collection")), &m));
        assert!(!holds(Condition::QualityProfileIn(one("HD-1080p")), &m));
        assert!(!holds(Condition::StudioIn(one("")), &media()), "an unknown studio matched");
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
        let outcome = evaluate_one(
            &Condition::YearRange { min: None, max: None },
            EvalContext { media: &media, metadata: Some(&metadata), now: now() },
        );
        assert!(!outcome.matched, "an open range must not classify media with no year");
        assert!(outcome.observed.is_empty(), "the engine reports the raw value, not prose");
        // The blank is turned into a word at render time, in the right language.
        let english = crate::localization::Localizer::new("en").describe(&outcome);
        assert!(english.contains("Unknown"), "{english}");
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
    fn a_myanimelist_r_plus_is_not_an_r() {
        let rated = |code: &str| {
            let media = media();
            let metadata = MediaMetadata { certification: Some(code.into()), ..metadata() };
            move |values: &[&str]| {
                let values = values.iter().map(|v| v.to_string()).collect();
                evaluate_one(
                    &Condition::CertificationIn(values),
                    EvalContext { media: &media, metadata: Some(&metadata), now: now() },
                )
                .matched
            }
        };
        assert!(!rated("R")(&["R+"]));
        assert!(rated("R")(&["R"]));
        assert!(rated("R+")(&["r+"]));
        assert!(rated("PG-13")(&["pg 13"]), "a separator is still one");
    }

    #[test]
    fn root_folder_comparison_ignores_trailing_slashes() {
        assert!(matches(Condition::CurrentRootFolder("/movies/standard/".into())));
        assert!(matches(Condition::CurrentRootFolder("/movies/standard".into())));
        assert!(!matches(Condition::CurrentRootFolder("/movies/anime".into())));
    }

    #[test]
    fn a_blank_language_value_reaches_no_item_without_a_language() {
        let with_blank = vec!["ja".to_string(), String::new()];
        assert!(!matches_without_metadata(Condition::OriginalLanguage(with_blank.clone())));
        assert!(matches_without_metadata(Condition::OriginalLanguageNot(with_blank)));
    }

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
        assert!(matches(Condition::OriginalLanguageKnown(true)));
        assert!(!matches(Condition::OriginalLanguageKnown(false)));
        assert!(matches_without_metadata(Condition::OriginalLanguageKnown(false)));
    }

    /// Over is strictly over, a gigabyte being 1024³ bytes as the root folders table counts it.
    #[test]
    fn size_and_season_thresholds_are_strict() {
        const GIB: i64 = 1 << 30;
        assert!(!matches_on(|m| m.size_on_disk = Some(GIB), Condition::SizeOnDiskOverGb(1)));
        assert!(matches_on(|m| m.size_on_disk = Some(GIB + 1), Condition::SizeOnDiskOverGb(1)));
        assert!(!matches_on(|m| m.season_count = Some(2), Condition::SeasonCountOver(2)));
        assert!(matches_on(|m| m.season_count = Some(3), Condition::SeasonCountOver(2)));
    }

    #[test]
    fn title_condition_is_case_insensitive_substring() {
        assert!(matches(Condition::TitleContains(vec!["totoro".into()])));
        assert!(matches(Condition::TitleContains(vec!["NEIGHBOR".into()])));
        assert!(!matches(Condition::TitleContains(vec!["akira".into()])));
    }

    #[test]
    fn a_title_is_matched_the_way_every_other_value_is() {
        let accented = |needle: &str| {
            let mut media = media();
            media.title = "Amélie".to_string();
            evaluate_one(
                &Condition::TitleContains(vec![needle.to_string()]),
                EvalContext { media: &media, metadata: None, now: now() },
            )
            .matched
        };
        assert!(accented("amelie"), "an unaccented needle has to find an accented title");
        assert!(accented("Amélie"), "and the accented one still does");

        // Punctuation folds to a space on both sides, so the typed separator decides nothing.
        let punctuated = |needle: &str| {
            let mut media = media();
            media.title = "Spider-Man: No Way Home".to_string();
            evaluate_one(
                &Condition::TitleContains(vec![needle.to_string()]),
                EvalContext { media: &media, metadata: None, now: now() },
            )
            .matched
        };
        assert!(punctuated("spider man"));
        assert!(punctuated("Spider-Man"));
        assert!(punctuated("no way home"));

        // Folding finds more spellings of a title, never another title.
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

        let (media, metadata) = (media(), metadata());
        let ctx = EvalContext { media: &media, metadata: Some(&metadata), now: now() };
        for condition in [Condition::TmdbIdIn(vec![8392, 1]), Condition::TvdbIdIn(vec![8392, 1])] {
            let outcome = evaluate_one(&condition, ctx);
            assert_eq!(outcome.params["values"], "8392, 1", "{}", outcome.kind);
        }
    }

    #[test]
    fn added_within_days_counts_whole_days_back_from_the_clock() {
        use chrono::TimeDelta;
        let added = |before: TimeDelta| {
            let stamp = (now() - before).format("%Y-%m-%d %H:%M:%S").to_string();
            matches_on(|m| m.added_at = Some(stamp), Condition::AddedWithinDays(7))
        };
        assert!(!added(TimeDelta::days(7) + TimeDelta::hours(23)));
        assert!(added(TimeDelta::days(6) + TimeDelta::hours(23)));
        assert!(added(TimeDelta::days(7)));
        assert!(added(-TimeDelta::minutes(2)), "a clock two minutes ahead");
        assert!(!added(-TimeDelta::days(2)));
        assert!(!added(TimeDelta::days(2000)));
    }

    /// Two days back from the fixture's clock end at noon on the 19th, UTC.
    #[test]
    fn added_within_days_reads_every_shape_a_title_is_dated_in() {
        let added = |stamp: &str| {
            let stamp = stamp.to_string();
            matches_on(|m| m.added_at = Some(stamp), Condition::AddedWithinDays(2))
        };
        assert!(added("2026-08-19T12:30:00Z"));
        assert!(added("2026-08-19T14:30:00+02:00"));
        assert!(!added("2026-08-19T13:30:00+02:00"));
        assert!(added("2026-08-19T12:30:00"), "a stamp with no zone");
        assert!(!added("2026-08-19T11:30:00"));
        assert!(added("2026-08-20"), "a bare date");
        assert!(!added("2026-08-19"));
    }

    /// The explanation says the age the condition compared, at the clock it was judged by, in
    /// the reader's units.
    #[test]
    fn an_age_condition_explains_itself_with_the_title_s_age() {
        let mut media = media();
        media.added_at = Some("2026-07-31 12:00:00".into());
        let outcome = evaluate_one(
            &Condition::AddedWithinDays(30),
            EvalContext { media: &media, metadata: None, now: now() },
        );
        assert!(outcome.matched);
        let english = crate::localization::Localizer::new("en").describe(&outcome);
        assert!(english.ends_with("found [21 d]"), "{english}");
        let french = crate::localization::Localizer::new("fr").describe(&outcome);
        assert!(french.ends_with("[21\u{a0}j]"), "{french}");
    }

    #[test]
    fn an_unparseable_timestamp_does_not_match() {
        let mut m = media();
        m.added_at = Some("not a date".into());
        let outcome = evaluate_one(
            &Condition::AddedWithinDays(3650),
            EvalContext { media: &m, metadata: None, now: now() },
        );
        assert!(!outcome.matched);
    }

    #[test]
    fn metadata_conditions_do_not_match_when_metadata_is_missing() {
        assert!(!matches_without_metadata(Condition::GenreContains(vec!["Animation".into()])));
        assert!(!matches_without_metadata(Condition::OriginalLanguage(vec!["ja".into()])));
        // The negative form holds, as a "not anime" rule needs on an unknown title.
        assert!(matches_without_metadata(Condition::GenreNotContains(vec!["Animation".into()])));
    }

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
            Some(SeriesTypeIs(_)) => QualityProfileIn(list(&["Ultra-HD"])),
            Some(QualityProfileIn(_)) => RuntimeRange { min: Some(60), max: Some(120) },
            Some(RuntimeRange { .. }) => StudioIn(list(&["Studio Ghibli"])),
            Some(StudioIn(_)) => NetworkIn(list(&["Fuji TV"])),
            Some(NetworkIn(_)) => CollectionIn(list(&["Studio Ghibli Collection"])),
            Some(CollectionIn(_)) => ScoreRange {
                scale: "imdb".into(),
                min: Some(7.5),
                max: None,
                min_votes: Some(1000),
            },
            Some(ScoreRange { .. }) => SizeOnDiskOverGb(1),
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
            Some(HasMetadata(_)) => OriginalLanguageKnown(true),
            Some(OriginalLanguageKnown(_)) => return None,
        })
    }

    /// The cost of a pass is one fold per value, of the rules and of each title, and folding
    /// inside each comparison pays both again for every condition reading them.
    #[test]
    fn an_evaluation_folds_each_title_value_and_each_rule_value_at_most_once() {
        use crate::services::templates;
        let localizer = crate::localization::Localizer::new("en");
        let answers = templates::Answers { category: "x".into(), ..Default::default() };
        let rules: Vec<Rule> = ["children", "concerts", "documentaries"]
            .into_iter()
            .flat_map(|id| templates::render(templates::find(id).unwrap(), &answers, &localizer))
            .enumerate()
            .map(|(index, req)| {
                let mut rule = rule(&req.name, index as i64, "x", req.conditions);
                rule.exclusions = req.exclusions;
                rule
            })
            .collect();
        let rule_values: usize = rules
            .iter()
            .flat_map(|rule| rule.conditions.iter().chain(&rule.exclusions))
            .map(|condition| match condition {
                Condition::GenreContains(values) | Condition::KeywordContains(values) => {
                    values.len()
                }
                _ => 0,
            })
            .sum();
        let mut media = media();
        media.tags = Some(r#"["kids"]"#.into());
        let metadata = metadata();
        let title_values = metadata.genres.len()
            + metadata.keywords.len()
            + metadata.origin_countries.len()
            + media.tag_labels().len()
            + 1;

        let ctx = EvalContext { media: &media, metadata: Some(&metadata), now: now() };
        let (prepared, folded) = folding::folds_in(|| Prepared::new(&rules));
        assert!(folded <= rule_values, "{folded} folds for {rule_values} rule values");
        for _ in 0..2 {
            let (_, folded) = folding::folds_in(|| evaluate_prepared(ctx, &prepared, None));
            assert!(folded <= title_values, "{folded} folds for {title_values} title values");
        }
    }

    #[test]
    fn every_condition_is_worded_whole_in_english_and_french() {
        let (mut media, metadata) = (media(), metadata());
        media.size_on_disk = Some(13_207_024_435);
        let english = crate::localization::Localizer::new("en");
        let french = crate::localization::Localizer::new("fr");
        let mut seen = std::collections::HashSet::new();
        let mut condition = next_kind(None);
        while let Some(current) = condition {
            let outcome = evaluate_one(
                &current,
                EvalContext { media: &media, metadata: Some(&metadata), now: now() },
            );
            let reason = english.describe(&outcome);
            assert!(!reason.contains(&outcome.key), "{} has no sentence: {reason}", outcome.kind);
            assert!(!reason.contains('{'), "{} left a placeholder: {reason}", outcome.kind);
            // Whole in French: no `true`, no English size unit, no stray time of day.
            let reason = french.describe(&outcome);
            for english_only in ["true", "false", " GB", "T00:", ":00Z", "[-]"] {
                assert!(!reason.contains(english_only), "{}: {reason}", outcome.kind);
            }
            if outcome.kind == "score_range" {
                assert!(reason.contains("8,1 (393\u{202f}000)"), "the votes found: {reason}");
            }
            assert!(seen.insert(current.kind()), "{} is visited twice", current.kind());
            condition = next_kind(Some(&current));
        }
        assert_eq!(seen.len(), 35, "the walk visited {} kinds", seen.len());
    }

    #[test]
    fn condition_outcomes_carry_expected_and_observed() {
        let media = media();
        let metadata = metadata();
        let outcome = evaluate_one(
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
    fn confidence_grows_by_a_step_per_agreeing_signal() {
        let one =
            rule("one", 10, "anime", vec![Condition::GenreContains(vec!["Animation".into()])]);
        let confidence = |rule| evaluate(&[rule], None).winner.unwrap().confidence;

        let (low, high) = (confidence(one), confidence(anime_rule()));
        assert!((low - 0.60).abs() < 1e-6, "{low}");
        assert!((high - 0.70).abs() < 1e-6, "{high}");
    }

    #[test]
    fn any_mode_confidence_is_capped_below_all_mode() {
        let mut any = anime_rule();
        any.match_mode = MatchMode::Any;
        let strict = evaluate(&[anime_rule()], None).winner.unwrap().confidence;
        let loose = evaluate(&[any], None).winner.unwrap().confidence;
        assert!(loose < strict);
    }

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
