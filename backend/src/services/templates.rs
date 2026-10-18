//! Ready-made rules for the usual library splits. A template is rendered
//! into ordinary rules in the reader's language, which then belong to the user
//! and keep the template they came from.

use std::collections::HashMap;

use crate::integrations::language;
use crate::localization::Localizer;
use crate::models::{Condition, CreateRuleRequest, MatchMode, Rule};
use crate::services::metadata;
use crate::services::routing::PRIORITY_STEP;

/// What a template asks beside its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parameter {
    None,
    /// The languages the household watches in.
    Languages,
    /// Who asked for each title, by the tag Seerr gives each requester.
    Requesters,
    /// The quality profiles that make a title 4K, by name.
    QualityProfiles,
}

impl Parameter {
    pub fn name(self) -> Option<&'static str> {
        match self {
            Parameter::None => None,
            Parameter::Languages => Some("languages"),
            Parameter::Requesters => Some("requesters"),
            Parameter::QualityProfiles => Some("quality_profiles"),
        }
    }
}

/// One rule of a template. Its name is a dictionary key.
struct RuleShape {
    name: &'static str,
    media_type: &'static str,
    match_mode: MatchMode,
    conditions: fn(&Answers) -> Vec<Condition>,
    exclusions: fn() -> Vec<Condition>,
}

pub struct Template {
    pub id: &'static str,
    /// Raised when a template's rules change, so a library can tell an old copy.
    pub version: u32,
    pub name: &'static str,
    pub description: &'static str,
    /// The suggested category's name, a dictionary key. Empty for a template
    /// whose categories come from its answers.
    pub category: &'static str,
    pub parameter: Parameter,
    /// The sources its rules read beyond the Arr, when only some of those
    /// answering a field suit it: AniList's keywords name no concert.
    pub relies_on: &'static [&'static str],
    rules: &'static [RuleShape],
}

impl Template {
    /// What a rule added from this template records, as `children@1`.
    pub fn marker(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

/// What the person adding a template chose.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    pub category: String,
    pub languages: Vec<String>,
    pub requesters: Vec<Requester>,
    pub quality_profiles: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Requester {
    /// Every tag Seerr gave them, in either of its forms.
    pub tags: Vec<String>,
    pub name: String,
    pub category: String,
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn nothing() -> Vec<Condition> {
    Vec::new()
}

/// In the order their rules go: a title two templates match goes to the first.
pub const CATALOGUE: &[Template] = &[
    // First: a 4K copy belongs to the library a 4K player reads, whatever its genre.
    Template {
        id: "4k",
        version: 1,
        name: "TemplateUhdName",
        description: "TemplateUhdDescription",
        category: "TemplateUhdCategory",
        parameter: Parameter::QualityProfiles,
        relies_on: &[],
        rules: &[RuleShape {
            name: "TemplateRuleUhd",
            media_type: "both",
            match_mode: MatchMode::All,
            conditions: |answers| {
                vec![Condition::QualityProfileIn(answers.quality_profiles.clone())]
            },
            exclusions: nothing,
        }],
    },
    Template {
        id: "concerts",
        version: 1,
        name: "TemplateConcertsName",
        description: "TemplateConcertsDescription",
        category: "TemplateConcertsCategory",
        parameter: Parameter::None,
        relies_on: &[metadata::TMDB],
        // Keywords only: the Music genre holds biopics, and "concert" any concert scene.
        rules: &[RuleShape {
            name: "TemplateRuleConcerts",
            media_type: "both",
            match_mode: MatchMode::All,
            conditions: |_| {
                vec![Condition::KeywordContains(words(&[
                    "concert film",
                    "live concert",
                    "concert documentary",
                    "stand-up comedy",
                    "stand-up specials",
                ]))]
            },
            exclusions: nothing,
        }],
    },
    Template {
        id: "documentaries",
        version: 1,
        name: "TemplateDocumentariesName",
        description: "TemplateDocumentariesDescription",
        category: "TemplateDocumentariesCategory",
        parameter: Parameter::None,
        relies_on: &[],
        rules: &[RuleShape {
            name: "TemplateRuleDocumentaries",
            media_type: "both",
            match_mode: MatchMode::All,
            conditions: |_| vec![Condition::GenreContains(words(&["Documentary"]))],
            exclusions: nothing,
        }],
    },
    Template {
        id: "children",
        version: 1,
        name: "TemplateChildrenName",
        description: "TemplateChildrenDescription",
        category: "TemplateChildrenCategory",
        parameter: Parameter::None,
        relies_on: &[],
        // TMDB's Family and Kids, TheTVDB's Children: Animation alone takes adult animation.
        rules: &[RuleShape {
            name: "TemplateRuleChildren",
            media_type: "both",
            match_mode: MatchMode::All,
            conditions: |_| vec![Condition::GenreContains(words(&["Family", "Kids", "Children"]))],
            exclusions: || {
                vec![
                    Condition::GenreContains(words(&["Horror"])),
                    // From 14 up. A code several systems read apart, as "A" or "C", stays out.
                    Condition::CertificationIn(words(&[
                        "R", "NC-17", "TV-14", "TV-MA", "X", "14", "14+", "14A", "15", "15+",
                        "15A", "16", "-16", "16+", "+16", "18", "-18", "18+", "+18", "18A", "19",
                        "R+", "RX", "R15", "R15+", "R16", "R18", "R18+", "R 18+", "X18+", "X 18+",
                        "MA15+", "MA 15+", "R-16", "R-18", "M/14", "M/16", "M/18", "NC16", "M18",
                        "R21", "VM14", "VM18", "IM-18", "K-15", "K-16", "K-18", "U/A 16+", "IIB",
                        "III",
                    ])),
                ]
            },
        }],
    },
    Template {
        id: "anime",
        version: 1,
        name: "TemplateAnimeName",
        description: "TemplateAnimeDescription",
        category: "TemplateAnimeCategory",
        parameter: Parameter::None,
        relies_on: &[],
        rules: &[
            RuleShape {
                name: "TemplateRuleAnimeSeries",
                media_type: "series",
                match_mode: MatchMode::Any,
                conditions: |_| {
                    vec![
                        Condition::SeriesTypeIs(words(&["anime"])),
                        Condition::GenreContains(words(&["Anime"])),
                    ]
                },
                exclusions: nothing,
            },
            // Radarr has no series type: Japanese animation is what tells.
            RuleShape {
                name: "TemplateRuleAnimeFilms",
                media_type: "movie",
                match_mode: MatchMode::All,
                conditions: |_| {
                    vec![
                        Condition::GenreContains(words(&["Animation"])),
                        Condition::OriginalLanguage(words(&["ja"])),
                    ]
                },
                exclusions: nothing,
            },
        ],
    },
    Template {
        id: "foreign-films",
        version: 1,
        name: "TemplateForeignName",
        description: "TemplateForeignDescription",
        category: "TemplateForeignCategory",
        parameter: Parameter::Languages,
        relies_on: &[],
        // Known language first, or the negative condition takes every undescribed film.
        rules: &[RuleShape {
            name: "TemplateRuleForeign",
            media_type: "movie",
            match_mode: MatchMode::All,
            conditions: |answers| {
                vec![
                    Condition::OriginalLanguageKnown(true),
                    Condition::OriginalLanguageNot(answers.languages.clone()),
                ]
            },
            exclusions: nothing,
        }],
    },
    // Its rules are one per requester, made from the answers.
    Template {
        id: "requesters",
        version: 1,
        name: "TemplateRequestersName",
        description: "TemplateRequestersDescription",
        category: "",
        parameter: Parameter::Requesters,
        relies_on: &[],
        rules: &[],
    },
];

pub fn find(id: &str) -> Option<&'static Template> {
    CATALOGUE.iter().find(|template| template.id == id)
}

/// A template's place in the catalogue, by the marker a rule keeps. A template
/// this build no longer ships goes after every other.
pub fn rank(marker: &str) -> usize {
    let id = marker.split('@').next().unwrap_or_default();
    CATALOGUE.iter().position(|template| template.id == id).unwrap_or(CATALOGUE.len())
}

/// The rules `template` adds for `answers`, named in `localizer`'s language,
/// each placed after every other until the import gives it its place.
pub fn render(
    template: &Template,
    answers: &Answers,
    localizer: &Localizer,
) -> Vec<CreateRuleRequest> {
    let origin = localizer
        .translate("TemplateRuleOrigin", &[("template", &localizer.translate(template.name, &[]))]);
    let rule =
        |name: String, media_type: &str, match_mode, conditions, exclusions, category: &str| {
            CreateRuleRequest {
                name,
                description: Some(origin.clone()),
                priority: None,
                enabled: true,
                media_type: media_type.to_string(),
                conditions,
                exclusions,
                match_mode,
                target_category: category.to_string(),
                instance_ids: None,
            }
        };
    if template.parameter == Parameter::Requesters {
        return answers
            .requesters
            .iter()
            .map(|requester| {
                rule(
                    localizer.translate("TemplateRuleRequester", &[("name", &requester.name)]),
                    "both",
                    MatchMode::All,
                    vec![Condition::TagIn(requester.tags.clone())],
                    Vec::new(),
                    &requester.category,
                )
            })
            .collect();
    }
    template
        .rules
        .iter()
        .map(|shape| {
            rule(
                localizer.translate(shape.name, &[]),
                shape.media_type,
                shape.match_mode,
                (shape.conditions)(answers),
                (shape.exclusions)(),
                &answers.category,
            )
        })
        .collect()
}

/// A rule in place, as the placing of a template reads it.
pub struct Held<'a> {
    pub id: &'a str,
    pub priority: i64,
    pub template: Option<&'a str>,
}

/// Where `adding` rules of the template ranked `rank` go: after the last rule
/// written by hand, before the first template rule ranking after it. Answers the
/// moves of the rules they overtake and the new rules' priorities.
pub fn place(held: &[Held], rank: usize, adding: usize) -> (Vec<(String, i64)>, Vec<i64>) {
    let base = held.iter().filter(|rule| rule.template.is_none()).map(|rule| rule.priority).max();
    let base = base.unwrap_or(0);
    let mut tail: Vec<&Held> =
        held.iter().filter(|rule| rule.template.is_some() && rule.priority > base).collect();
    tail.sort_by_key(|rule| rule.priority);
    let at = tail
        .iter()
        .position(|rule| self::rank(rule.template.unwrap_or_default()) > rank)
        .unwrap_or(tail.len());

    let mut last = at.checked_sub(1).map_or(base, |before| tail[before].priority);
    let new: Vec<i64> = (0..adding)
        .map(|_| {
            last += PRIORITY_STEP;
            last
        })
        .collect();
    let mut moved = Vec::new();
    for rule in &tail[at..] {
        if rule.priority > last {
            break;
        }
        last += PRIORITY_STEP;
        moved.push((rule.id.to_string(), last));
    }
    (moved, new)
}

/// What adding a template does to the rules in place.
#[derive(Debug, Default, PartialEq)]
pub struct Arrangement {
    /// The rules an earlier addition of it left, which go.
    pub removed: Vec<String>,
    /// The rules an earlier addition left that stay as they are.
    pub kept: Vec<String>,
    /// Rules in place given a new priority.
    pub moved: Vec<(String, i64)>,
    /// The rules of `adding` to write, by their index there, each with its
    /// priority.
    pub added: Vec<(usize, i64)>,
}

/// Where the rules `adding` renders for `template` go among `rules`.
pub fn arrange(rules: &[Rule], template: &Template, adding: &[CreateRuleRequest]) -> Arrangement {
    match template.parameter {
        Parameter::Requesters => arrange_requesters(rules, template, adding),
        _ => arrange_replacing(rules, template, adding),
    }
}

/// Every rule of an earlier addition goes, and each one replacing it takes
/// its place, wherever the user moved it. The others go where [`place`] puts
/// them.
fn arrange_replacing(
    rules: &[Rule],
    template: &Template,
    adding: &[CreateRuleRequest],
) -> Arrangement {
    let marker = template.marker();
    let mut earlier: Vec<&Rule> = rules.iter().filter(|rule| is_from(rule, template)).collect();
    earlier.sort_by_key(|rule| rule.priority);
    let mut places: HashMap<String, i64> = HashMap::new();
    for (key, rule) in slot_keys(
        earlier.iter().map(|rule| (rule.media_type.to_string(), rule.conditions.as_slice())),
    )
    .into_iter()
    .zip(&earlier)
    {
        places.entry(key).or_insert(rule.priority);
    }
    let kept: Vec<Option<i64>> = slot_keys(
        adding.iter().map(|rule| (rule.media_type.to_string(), rule.conditions.as_slice())),
    )
    .into_iter()
    .map(|key| places.remove(&key))
    .collect();

    // A replacement holding its place is a template rule like any other there.
    let standing: Vec<String> = (0..adding.len()).map(|index| format!("new:{index}")).collect();
    let mut held: Vec<Held> = rules
        .iter()
        .filter(|rule| !is_from(rule, template))
        .map(|rule| Held {
            id: &rule.id,
            priority: rule.priority,
            template: rule.template.as_deref(),
        })
        .collect();
    for (index, priority) in kept.iter().enumerate() {
        if let Some(priority) = priority {
            held.push(Held { id: &standing[index], priority: *priority, template: Some(&marker) });
        }
    }
    let unplaced = kept.iter().filter(|priority| priority.is_none()).count();
    let (moves, mut fresh) = place(&held, rank(&marker), unplaced);
    fresh.reverse();

    let mut priorities: Vec<i64> = kept
        .iter()
        .map(|priority| priority.unwrap_or_else(|| fresh.pop().unwrap_or_default()))
        .collect();
    let mut moved = Vec::new();
    for (id, priority) in moves {
        match id.strip_prefix("new:").and_then(|index| index.parse::<usize>().ok()) {
            Some(index) => priorities[index] = priority,
            None => moved.push((id, priority)),
        }
    }
    Arrangement {
        removed: earlier.iter().map(|rule| rule.id.clone()).collect(),
        kept: Vec::new(),
        moved,
        added: priorities.into_iter().enumerate().collect(),
    }
}

/// A requester still chosen keeps the rule routing them as it is, edits
/// included, one left out loses it, and only those not routed yet get one,
/// after the template's rules already there.
fn arrange_requesters(
    rules: &[Rule],
    template: &Template,
    adding: &[CreateRuleRequest],
) -> Arrangement {
    let chosen: Vec<Option<String>> =
        adding.iter().map(|rule| routed_requester(&rule.conditions)).collect();
    let mut arranged = Arrangement::default();
    let mut routed: Vec<String> = Vec::new();
    for rule in rules.iter().filter(|rule| is_from(rule, template)) {
        match routed_requester(&rule.conditions) {
            Some(id) if !chosen.contains(&Some(id.clone())) => {
                arranged.removed.push(rule.id.clone())
            }
            Some(id) => {
                arranged.kept.push(rule.id.clone());
                routed.push(id);
            }
            // Edited until it routes nobody Seerr names: the user's own now.
            None => arranged.kept.push(rule.id.clone()),
        }
    }
    let new: Vec<usize> = (0..adding.len())
        .filter(|&index| chosen[index].as_ref().is_none_or(|id| !routed.contains(id)))
        .collect();
    let held: Vec<Held> = rules
        .iter()
        .filter(|rule| !arranged.removed.contains(&rule.id))
        .map(|rule| Held {
            id: &rule.id,
            priority: rule.priority,
            template: rule.template.as_deref(),
        })
        .collect();
    let (moved, priorities) = place(&held, rank(&template.marker()), new.len());
    arranged.moved = moved;
    arranged.added = new.into_iter().zip(priorities).collect();
    arranged
}

/// The Seerr id of the requester a rule's tags name, if any.
fn routed_requester(conditions: &[Condition]) -> Option<String> {
    conditions.iter().find_map(|condition| match condition {
        Condition::TagIn(tags) => tags.iter().find_map(|tag| requester_of(tag)).map(|(id, _)| id),
        _ => None,
    })
}

/// Whether `rule` was added from `template`, at any version.
pub fn is_from(rule: &Rule, template: &Template) -> bool {
    rule.template.as_deref().is_some_and(|marker| marker.split('@').next() == Some(template.id))
}

/// What tells a template's rules apart across two additions: their kind of
/// title and their rank among those.
fn slot_keys<'a>(rules: impl Iterator<Item = (String, &'a [Condition])>) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    rules
        .map(|(media_type, _)| {
            let kind = media_type.to_lowercase();
            let nth = seen.entry(kind.clone()).or_default();
            *nth += 1;
            format!("{kind}:{nth}")
        })
        .collect()
}

/// Whether a quality profile's name reads as 4K, which the 4K template suggests.
pub fn reads_as_4k(name: &str) -> bool {
    let name = name.to_lowercase();
    ["4k", "2160", "uhd", "ultra"].iter().any(|mark| name.contains(mark))
}

/// The languages a household is assumed to watch in: the interface's and
/// English, as ISO 639-1 codes.
pub fn household_languages(interface: &str) -> Vec<String> {
    let own = interface.split(['_', '-']).next().unwrap_or_default().to_lowercase();
    // TMDB and the Arrs write Norwegian `no`, whichever of its forms.
    let own = match own.as_str() {
        "nb" | "nn" => "no".to_string(),
        _ => own,
    };
    let mut languages = vec![own, "en".to_string()];
    languages.retain(|code| !code.is_empty());
    languages.dedup();
    languages
}

/// A language as the engine compares it, an ISO 639-1 code, from what a
/// person typed: `fr`, `FR`, `fr-CA`, `fra` or `French`. `None` for what
/// names no language.
pub fn language_code(raw: &str) -> Option<String> {
    let known = |code: &str| language::LANGUAGES.iter().any(|(listed, _)| *listed == code);
    let base = raw.trim().split(['-', '_']).next().unwrap_or_default().to_lowercase();
    match base.len() {
        2 => Some(base).filter(|code| known(code)),
        3 => language::from_iso_639_3(&base),
        _ => None,
    }
    .or_else(|| language::normalise(raw).filter(|code| known(code)))
}

/// A tag Seerr gave a requester, `3-alice`, or `3 - alice` as an older Seerr
/// writes it: the requester's Seerr id and name, or `None` for any other tag.
pub fn requester_of(tag: &str) -> Option<(String, String)> {
    let (id, name) = tag.split_once('-')?;
    let (id, name) = (id.trim(), name.trim());
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !name.is_empty())
        .then(|| (id.to_string(), name.to_string()))
}

/// The longest category name, as `api::categories::normalise` holds it.
const CATEGORY_LENGTH: usize = 64;

/// A category name for a requester: their name as a category may hold it,
/// or their Seerr id for a name made only of signs.
pub fn requester_category(id: &str, name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = name
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' })
        .collect();
    let joined = folded.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    let name: String = joined.chars().take(CATEGORY_LENGTH).collect();
    match name.trim_end_matches('-') {
        "" => format!("seerr-{id}"),
        name => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RuleMediaType;

    #[test]
    fn a_seerr_tag_names_its_requester_in_either_form() {
        let named = |id: &str, name: &str| Some((id.to_string(), name.to_string()));
        assert_eq!(requester_of("3-alice"), named("3", "alice"));
        assert_eq!(requester_of("12 - bob smith"), named("12", "bob smith"));
        assert_eq!(requester_of("anime"), None);
        assert_eq!(requester_of("4k-remux"), None);
        assert_eq!(requester_of("3-"), None);
    }

    #[test]
    fn the_household_speaks_the_interface_language_and_english() {
        assert_eq!(household_languages("fr"), vec!["fr", "en"]);
        assert_eq!(household_languages("pt_BR"), vec!["pt", "en"]);
        assert_eq!(household_languages("en"), vec!["en"]);
        assert_eq!(household_languages("nb_NO"), vec!["no", "en"], "TMDB writes Norwegian no");
    }

    #[test]
    fn a_language_typed_by_hand_becomes_the_code_the_engine_compares() {
        let code = |raw: &str| language_code(raw);
        for typed in ["fr", "FR", " fr ", "fr-CA", "fra", "French", "french"] {
            assert_eq!(code(typed).as_deref(), Some("fr"), "{typed}");
        }
        assert_eq!(code("pt_BR").as_deref(), Some("pt"));
        assert_eq!(code("jpn").as_deref(), Some("ja"));
        for nothing in ["xx", "klingon", "frr-", "12"] {
            assert_eq!(code(nothing), None, "{nothing}");
        }
    }

    #[test]
    fn the_catalogue_goes_in_the_order_the_owner_chose() {
        let ids: Vec<&str> = CATALOGUE.iter().map(|template| template.id).collect();
        assert_eq!(
            ids,
            ["4k", "concerts", "documentaries", "children", "anime", "foreign-films", "requesters"]
        );
        assert_eq!(rank("children@1"), 3);
        assert_eq!(rank("gone@1"), CATALOGUE.len());
    }

    fn held<'a>(id: &'a str, priority: i64, template: Option<&'a str>) -> Held<'a> {
        Held { id, priority, template }
    }

    #[test]
    fn a_template_goes_after_the_hand_written_rules_and_among_the_templates_by_rank() {
        let held = [
            held("mine", 10, None),
            held("docs", 20, Some("documentaries@1")),
            held("anime", 30, Some("anime@1")),
        ];
        // Children ranks between documentaries and anime.
        let (moved, new) = place(&held, rank("children@1"), 1);
        assert_eq!(new, vec![30]);
        assert_eq!(moved, vec![("anime".to_string(), 40)]);
    }

    #[test]
    fn a_template_rule_moved_above_the_hand_written_ones_stays_there() {
        let held = [held("kids", 5, Some("children@1")), held("mine", 10, None)];
        let (moved, new) = place(&held, rank("concerts@1"), 2);
        assert!(moved.is_empty());
        assert_eq!(new, vec![20, 30]);
    }

    /// The user put anime films before children, so a Japanese family film
    /// goes to anime: adding documentaries keeps that order.
    #[test]
    fn adding_a_template_keeps_the_order_the_user_gave_the_others() {
        let held = [
            held("mine", 10, None),
            held("anime", 20, Some("anime@1")),
            held("kids", 30, Some("children@1")),
        ];
        let (moved, new) = place(&held, rank("documentaries@1"), 1);
        assert_eq!(new, vec![20]);
        assert_eq!(moved, vec![("anime".to_string(), 30), ("kids".to_string(), 40)]);

        let (moved, new) = place(&held, rank("foreign-films@1"), 1);
        assert!(moved.is_empty(), "nothing ranks after foreign films");
        assert_eq!(new, vec![40]);
    }

    /// A rule written by hand after a template added earlier stays ahead of
    /// any template added later, whatever the catalogue's order.
    #[test]
    fn a_template_added_later_never_goes_before_a_rule_written_by_hand() {
        let held = [held("kids", 20, Some("children@1")), held("4k", 30, None)];
        let (moved, new) = place(&held, rank("documentaries@1"), 1);
        assert!(moved.is_empty());
        assert_eq!(new, vec![40]);
    }

    #[test]
    fn only_the_rules_a_new_one_would_overtake_move() {
        let held =
            [held("anime", 10, Some("anime@1")), held("foreign", 100, Some("foreign-films@1"))];
        let (moved, new) = place(&held, rank("children@1"), 1);
        assert_eq!(new, vec![10]);
        assert_eq!(moved, vec![("anime".to_string(), 20)], "foreign films had room after");
        let (moved, new) = place(&[], rank("children@1"), 2);
        assert!(moved.is_empty());
        assert_eq!(new, vec![10, 20]);
    }

    #[test]
    fn a_template_this_build_does_not_ship_stays_after_the_others() {
        let held = [held("gone", 10, Some("gone@3"))];
        let (moved, new) = place(&held, rank("anime@1"), 1);
        assert_eq!(new, vec![10]);
        assert_eq!(moved, vec![("gone".to_string(), 20)]);
    }

    fn rule(
        id: &str,
        priority: i64,
        media_type: &str,
        template: Option<&str>,
        conditions: Vec<Condition>,
    ) -> Rule {
        Rule {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            priority,
            enabled: true,
            media_type: media_type.parse().unwrap_or(RuleMediaType::Both),
            conditions,
            exclusions: Vec::new(),
            match_mode: MatchMode::All,
            target_category: "somewhere".to_string(),
            instance_ids: None,
            template: template.map(str::to_string),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn rendered(template: &Template, answers: &Answers) -> Vec<CreateRuleRequest> {
        render(template, answers, &Localizer::new("en"))
    }

    #[test]
    fn a_replaced_rule_takes_the_place_of_the_one_it_replaces() {
        let anime = find("anime").unwrap();
        let held = [
            rule("films", 5, "movie", Some("anime@1"), Vec::new()),
            rule("mine", 10, "both", None, Vec::new()),
            rule("series", 20, "series", Some("anime@1"), Vec::new()),
        ];
        let arranged = arrange(&held, anime, &rendered(anime, &Answers::default()));
        assert_eq!(arranged.removed, vec!["films", "series"]);
        // Rendered as series then films.
        assert_eq!(arranged.added, vec![(0, 20), (1, 5)]);
        assert!(arranged.moved.is_empty());
    }

    /// Adding a requester leaves the others' rules as they stand, edits
    /// included, and unticking one removes theirs.
    #[test]
    fn a_requester_still_chosen_keeps_their_rule_and_one_left_out_loses_it() {
        let template = find("requesters").unwrap();
        let tag = |tag: &str| vec![Condition::TagIn(vec![tag.to_string()])];
        let held = [
            rule("bob", 10, "both", Some("requesters@1"), tag("4-bob")),
            rule("mine", 20, "both", None, Vec::new()),
            rule("alice", 30, "both", Some("requesters@1"), tag("3 - alice")),
            rule("dave", 35, "both", Some("requesters@1"), tag("6-dave")),
            // From a template this build no longer ships, so after them all.
            rule("gone", 40, "both", Some("gone@1"), Vec::new()),
        ];
        let person = |tags: &[&str], name: &str| Requester {
            tags: words(tags),
            name: name.to_string(),
            category: name.to_string(),
        };
        let answers = Answers {
            requesters: vec![
                person(&["3-alice"], "alice"),
                person(&["7-carol"], "carol"),
                person(&["4-bob"], "bob"),
            ],
            ..Answers::default()
        };

        let arranged = arrange(&held, template, &rendered(template, &answers));

        assert_eq!(arranged.kept, vec!["bob", "alice"]);
        assert_eq!(arranged.removed, vec!["dave"]);
        assert_eq!(arranged.added, vec![(1, 40)], "carol, after alice");
        assert_eq!(arranged.moved, vec![("gone".to_string(), 50)]);
    }
}
