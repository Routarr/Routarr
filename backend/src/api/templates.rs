//! Rule templates: the gallery, what one would do, and adding it.

use std::collections::{BTreeSet, HashMap};

use axum::extract::State;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::rules::{environment, insert_rule, judge};
use super::{Created, Json, Path};
use crate::error::{AppError, AppResult};
use crate::localization::Localizer;
use crate::models::{
    Condition, CreateRuleRequest, DecisionAction, Rule, RuleMediaType, ValidationIssue,
};
use crate::services::metadata;
use crate::services::routing::{self, SimulationOptions, load_rules};
use crate::services::rule_engine::normalise_value;
use crate::services::templates::{self, Answers, Parameter, Requester, Template};
use crate::state::AppState;

/// A template as the gallery shows it, with the answers an earlier addition
/// of it gave, or those it suggests.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplateSummary {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub description: String,
    /// The category its rules send titles to. Null for `requesters`, whose
    /// categories are each person's.
    pub category: Option<String>,
    /// What it asks beside the category: `languages`, `requesters`, `quality_profiles`, or
    /// null.
    #[schema(value_type = Option<String>, extensions(("x-extensible-enum" = json!(["languages", "requesters", "quality_profiles"]))))]
    pub parameter: Option<&'static str>,
    /// For `languages`: the languages the household watches in, ISO 639-1
    /// codes.
    pub languages: Vec<String>,
    /// For `requesters`: the requesters the library's tags name.
    pub requesters: Vec<RequesterTag>,
    /// For `quality_profiles`: the profiles its rule names, or those whose name reads as 4K.
    pub quality_profiles: Vec<String>,
    /// The sources it needs, none of them enabled. Empty when it can match.
    pub needs: Vec<NeededSource>,
    /// The version the rules in place were added from. Null when none is.
    pub added_version: Option<u32>,
    /// The rules in place added from it.
    pub rule_ids: Vec<String>,
}

/// A metadata source a template needs.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NeededSource {
    pub id: String,
    pub name: String,
}

/// A requester, by the tag Seerr gives each request they make.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RequesterTag {
    pub tag: String,
    pub name: String,
    /// Titles carrying the tag.
    pub titles: i64,
    /// Where their titles go: the category a rule in place sends them to, or
    /// the one suggested.
    pub category: String,
    /// Whether a rule added from the template routes them.
    pub added: bool,
}

/// What adding a template takes. Each template reads only the fields it
/// asks: `category` all but `requesters`, `languages` only `languages`,
/// `requesters` only `requesters`, `quality_profiles` only `quality_profiles`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateRequest {
    /// The category its rules send titles to, created when missing. Left out,
    /// the one it suggests.
    #[serde(default)]
    pub category: Option<String>,
    /// ISO 639-1 codes. Left out, the ones it assumes.
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub requesters: Option<Vec<RequesterChoice>>,
    /// Quality profile names, as an instance defines them. Left out, those whose name reads as
    /// 4K.
    #[serde(default)]
    pub quality_profiles: Option<Vec<String>>,
    /// Remove the rules an earlier addition of this template left, edited or not, each new
    /// rule taking the place of the one it replaces. For `requesters`, a requester still chosen
    /// keeps their rule, those left out lose theirs, and the others get one.
    #[serde(default)]
    pub replace: bool,
}

/// One requester to route, and where.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RequesterChoice {
    pub tag: String,
    pub category: String,
}

/// A rule the template would add, at the priority it would take, and what
/// the validator says of it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlannedRule {
    pub rule: CreateRuleRequest,
    pub issues: Vec<ValidationIssue>,
}

/// A rule in place that would keep titles the template's rules match.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TakenBy {
    pub rule_id: String,
    pub rule_name: String,
    pub titles: usize,
}

/// The titles a template's rules would decide, by what becomes of them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplateReach {
    pub titles: usize,
    /// To move into their category's folder.
    pub to_move: usize,
    /// Already in that folder.
    pub in_place: usize,
    /// Their category has no folder on their instance yet.
    pub unmapped: usize,
}

/// A title a template's rules would decide.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplateTitle {
    pub media_id: String,
    pub media_title: String,
    pub media_type: String,
    pub instance_name: Option<String>,
    pub category: String,
    pub rule_name: String,
    /// `move`, `none` when it is already in its category's folder, `skip`
    /// when that category has no folder on its instance.
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["move", "none", "skip"]))))]
    pub action: DecisionAction,
}

/// What adding a template would do. Persists nothing.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplatePreview {
    pub rules: Vec<PlannedRule>,
    /// Categories it would create.
    pub creates: Vec<String>,
    pub reach: TemplateReach,
    /// The first titles it would decide, those to move first.
    pub titles: Vec<TemplateTitle>,
    /// The rules in place that would keep titles its rules match, most first.
    pub taken_by: Vec<TakenBy>,
}

/// What adding a template did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplateAdded {
    pub rules: Vec<Rule>,
    /// Categories it created.
    pub created: Vec<String>,
    /// Rules of an earlier addition it removed.
    pub replaced: usize,
}

/// How many titles a preview words.
const SAMPLE: usize = 25;

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<TemplateSummary>>> {
    let localizer = state.localizer().await;
    let household = templates::household_languages(&state.language().await);
    let enabled: Vec<&str> =
        state.metadata_providers().await.iter().map(|provider| provider.id).collect();
    let rules = load_rules(&state.pool).await?;
    let tags = requester_tags(&state).await?;
    let known: Vec<String> =
        sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?;
    let profiles = quality_profiles(&state.pool).await?;

    let summaries = templates::CATALOGUE
        .iter()
        .map(|template| {
            let mut added: Vec<&Rule> =
                rules.iter().filter(|rule| templates::is_from(rule, template)).collect();
            added.sort_by_key(|rule| rule.priority);
            let first = added.first();
            let category = (template.parameter != Parameter::Requesters).then(|| {
                first.map_or_else(
                    || suggested_category(template, &localizer, &known),
                    |rule| rule.target_category.clone(),
                )
            });
            let languages = match template.parameter {
                Parameter::Languages => {
                    first.and_then(|rule| languages_of(rule)).unwrap_or_else(|| household.clone())
                }
                _ => Vec::new(),
            };
            let requesters = match template.parameter {
                Parameter::Requesters => routed(&tags.people, &added),
                _ => Vec::new(),
            };
            let quality_profiles = match template.parameter {
                Parameter::QualityProfiles => first
                    .and_then(|rule| profiles_of(rule))
                    .unwrap_or_else(|| suggested_profiles(&profiles)),
                _ => Vec::new(),
            };
            let answers = Answers {
                category: category.clone().unwrap_or_default(),
                languages: languages.clone(),
                requesters: Vec::new(),
                quality_profiles: quality_profiles.clone(),
            };
            TemplateSummary {
                id: template.id.to_string(),
                version: template.version,
                name: localizer.translate(template.name, &[]),
                description: localizer.translate(template.description, &[]),
                category,
                parameter: template.parameter.name(),
                languages,
                requesters,
                quality_profiles,
                needs: needs(
                    template,
                    &templates::render(template, &answers, &localizer),
                    &enabled,
                ),
                added_version: added
                    .iter()
                    .filter_map(|rule| rule.template.as_deref()?.split_once('@')?.1.parse().ok())
                    .max(),
                rule_ids: added.iter().map(|rule| rule.id.clone()).collect(),
            }
        })
        .collect();
    Ok(Json(summaries))
}

/// Show what adding a template would do.
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TemplateRequest>,
) -> AppResult<Json<TemplatePreview>> {
    let template = known(&id)?;
    let localizer = state.localizer().await;
    let planned = plan(&state, template, &req, &localizer).await?;
    let rules = load_rules(&state.pool).await?;
    refuse_twice(template, &rules, req.replace, &localizer)?;
    let _queued = super::simulation::wait_in_line(&state).await?;

    let rendered: Vec<CreateRuleRequest> =
        planned.rules.iter().map(|planned| planned.rule.clone()).collect();
    let arranged = templates::arrange(&rules, template, &rendered);
    let mut planned_rules: Vec<Option<PlannedRule>> = planned.rules.into_iter().map(Some).collect();
    let writing: Vec<PlannedRule> = arranged
        .added
        .iter()
        .filter_map(|(index, priority)| {
            let mut planned = planned_rules[*index].take()?;
            planned.rule.priority = Some(*priority);
            Some(planned)
        })
        .collect();
    let creates: Vec<String> = planned
        .creates
        .into_iter()
        .filter(|name| writing.iter().any(|planned| &planned.rule.target_category == name))
        .collect();
    let moved: HashMap<&str, i64> =
        arranged.moved.iter().map(|(id, priority)| (id.as_str(), *priority)).collect();
    let mut candidate: Vec<Rule> = rules
        .into_iter()
        .filter(|rule| !arranged.removed.contains(&rule.id))
        .map(|rule| match moved.get(rule.id.as_str()) {
            Some(priority) => Rule { priority: *priority, ..rule },
            None => rule,
        })
        .collect();
    let added: Vec<Rule> = writing
        .iter()
        .enumerate()
        .map(|(index, planned)| as_rule(template, index, &planned.rule))
        .collect();
    let mut watched: Vec<String> = added.iter().map(|rule| rule.id.clone()).collect();
    watched.extend(arranged.kept.iter().cloned());
    candidate.extend(added);
    let names: HashMap<String, String> =
        candidate.iter().map(|rule| (rule.id.clone(), rule.name.clone())).collect();

    let library =
        routing::load_library(&state.pool, &state.library_passes, &SimulationOptions::default())
            .await?;
    let reach =
        routing::reach(&library, candidate, watched, SAMPLE, &state.language().await).await?;
    tracing::debug!(template = %template.marker(), titles = reach.titles, "Template previewed");

    Ok(Json(TemplatePreview {
        rules: writing,
        creates,
        reach: TemplateReach {
            titles: reach.titles,
            to_move: reach.to_move,
            in_place: reach.in_place,
            unmapped: reach.unmapped,
        },
        titles: reach
            .sample
            .into_iter()
            .map(|decision| TemplateTitle {
                media_id: decision.media_id,
                media_title: decision.media_title,
                media_type: decision.media_type,
                instance_name: decision.instance_name,
                category: decision.target_category,
                rule_name: decision.matched_rule_name.unwrap_or_default(),
                action: decision.action,
            })
            .collect(),
        taken_by: reach
            .taken
            .into_iter()
            .map(|(rule_id, titles)| TakenBy {
                rule_name: names.get(&rule_id).cloned().unwrap_or_else(|| rule_id.clone()),
                rule_id,
                titles,
            })
            .collect(),
    }))
}

/// Add a template's rules after the rules written by hand, in the catalogue's
/// order among the templates already added.
pub async fn import(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TemplateRequest>,
) -> AppResult<Created<TemplateAdded>> {
    let template = known(&id)?;
    let localizer = state.localizer().await;
    let planned = plan(&state, template, &req, &localizer).await?;
    let refused: Vec<String> = planned
        .rules
        .iter()
        .flat_map(|planned| planned.issues.iter().filter(|issue| issue.is_error()))
        .map(|issue| issue.message.clone())
        .collect();
    if !refused.is_empty() {
        return Err(AppError::BadRequest(refused.join(" · ")));
    }
    crate::race::checked("templates::import", template.id).await;

    let marker = template.marker();
    let category_note = localizer.translate(
        "TemplateCategoryCreated",
        &[("template", &localizer.translate(template.name, &[]))],
    );
    let rendered: Vec<CreateRuleRequest> =
        planned.rules.into_iter().map(|planned| planned.rule).collect();
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let rules = load_rules(&mut *tx).await?;
    refuse_twice(template, &rules, req.replace, &localizer)?;
    let arranged = templates::arrange(&rules, template, &rendered);
    for id in &arranged.removed {
        sqlx::query("DELETE FROM rules WHERE id = ?").bind(id).execute(&mut *tx).await?;
    }
    let writing: Vec<CreateRuleRequest> = arranged
        .added
        .iter()
        .map(|(index, priority)| CreateRuleRequest {
            priority: Some(*priority),
            ..rendered[*index].clone()
        })
        .collect();

    // In this transaction, or a category removed since the plan leaves a rule naming nothing.
    let targets: BTreeSet<&str> =
        writing.iter().map(|rule| rule.target_category.as_str()).collect();
    let mut created = Vec::new();
    for name in targets {
        let inserted = sqlx::query(
            "INSERT INTO categories (id, name, description) VALUES (?, ?, ?) ON CONFLICT(name) DO NOTHING",
        )
        .bind(format!("cat-{}", Uuid::new_v4()))
        .bind(name)
        .bind(&category_note)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() > 0 {
            created.push(name.to_string());
        }
    }

    for (id, priority) in &arranged.moved {
        sqlx::query("UPDATE rules SET priority = ? WHERE id = ?")
            .bind(priority)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let mut added = Vec::new();
    for rule in &writing {
        let id = Uuid::new_v4().to_string();
        insert_rule(&mut tx, &id, rule, Some(&marker), &localizer).await?;
        added.push(id);
    }
    tx.commit().await?;

    let rules: Vec<Rule> = load_rules(&state.pool)
        .await?
        .into_iter()
        .filter(|rule| added.contains(&rule.id))
        .collect();
    let replaced = arranged.removed.len();
    Ok(Created::at(&state, "/rules", TemplateAdded { rules, created, replaced }))
}

fn known(id: &str) -> AppResult<&'static Template> {
    templates::find(id).ok_or_else(|| AppError::NotFound(format!("No template is called {id}")))
}

/// A template is added once: a second copy would double its rules.
fn refuse_twice(
    template: &Template,
    rules: &[Rule],
    replace: bool,
    localizer: &Localizer,
) -> AppResult<()> {
    if replace || !rules.iter().any(|rule| templates::is_from(rule, template)) {
        return Ok(());
    }
    Err(AppError::Conflict {
        reason: "template_added",
        message: localizer.translate(
            "ErrorTemplateAdded",
            &[("template", &localizer.translate(template.name, &[]))],
        ),
    })
}

struct Plan {
    rules: Vec<PlannedRule>,
    creates: Vec<String>,
}

/// The template rendered for `req`, each rule judged against the categories as
/// the addition would leave them.
async fn plan(
    state: &AppState,
    template: &Template,
    req: &TemplateRequest,
    localizer: &Localizer,
) -> AppResult<Plan> {
    // An answer the template does not read would be dropped without a word.
    let unread = [
        ("category", req.category.is_some() && template.parameter == Parameter::Requesters),
        ("languages", req.languages.is_some() && template.parameter != Parameter::Languages),
        ("requesters", req.requesters.is_some() && template.parameter != Parameter::Requesters),
        (
            "quality_profiles",
            req.quality_profiles.is_some() && template.parameter != Parameter::QualityProfiles,
        ),
    ];
    if let Some((field, _)) = unread.iter().find(|(_, unread)| *unread) {
        return Err(AppError::BadRequest(format!(
            "The template {} does not read {field}",
            template.id
        )));
    }

    let mut env = environment(state).await?;
    let mut answers = Answers::default();
    match template.parameter {
        Parameter::Requesters => {
            let choices = req.requesters.as_deref().unwrap_or_default();
            if choices.is_empty() {
                return Err(AppError::BadRequest(
                    localizer.translate("ErrorTemplateNoRequester", &[]),
                ));
            }
            let tags = requester_tags(state).await?;
            for choice in choices {
                let Some((seerr_id, name)) = templates::requester_of(&choice.tag) else {
                    return Err(AppError::BadRequest(format!(
                        "{} is not a tag Seerr gives a requester",
                        choice.tag
                    )));
                };
                // Every tag the library holds for their Seerr id, a renamed one included.
                let mut alike = vec![choice.tag.trim().to_string()];
                for known in &tags.all {
                    let same = templates::requester_of(known).is_some_and(|(id, _)| id == seerr_id);
                    let folded = normalise_value(known);
                    if same && !alike.iter().any(|tag| normalise_value(tag) == folded) {
                        alike.push(known.clone());
                    }
                }
                let category = super::categories::normalise(&choice.category, localizer)?;
                answers.requesters.push(Requester { tags: alike, name, category });
            }
        }
        _ => {
            let raw = req
                .category
                .clone()
                .unwrap_or_else(|| suggested_category(template, localizer, &env.known));
            answers.category = super::categories::normalise(&raw, localizer)?;
        }
    }
    if template.parameter == Parameter::Languages {
        let given = req
            .languages
            .clone()
            .unwrap_or_else(|| templates::household_languages(localizer.language()));
        let mut languages: Vec<String> = Vec::new();
        for raw in given.iter().filter(|raw| !raw.trim().is_empty()) {
            let Some(code) = templates::language_code(raw) else {
                return Err(AppError::BadRequest(
                    localizer.translate("ErrorTemplateLanguage", &[("value", raw.trim())]),
                ));
            };
            if !languages.contains(&code) {
                languages.push(code);
            }
        }
        if languages.is_empty() {
            return Err(AppError::BadRequest(localizer.translate("ErrorTemplateNoLanguage", &[])));
        }
        answers.languages = languages;
    }
    if template.parameter == Parameter::QualityProfiles {
        let defined = quality_profiles(&state.pool).await?;
        let given = req.quality_profiles.clone().unwrap_or_else(|| suggested_profiles(&defined));
        for raw in given.iter().map(|raw| raw.trim()).filter(|raw| !raw.is_empty()) {
            // Named as an instance defines it, since the rule compares the names.
            let Some(name) =
                defined.iter().find(|name| normalise_value(name) == normalise_value(raw))
            else {
                return Err(AppError::BadRequest(
                    localizer.translate("ErrorTemplateQualityProfile", &[("value", raw)]),
                ));
            };
            if !answers.quality_profiles.contains(name) {
                answers.quality_profiles.push(name.clone());
            }
        }
        if answers.quality_profiles.is_empty() {
            return Err(AppError::BadRequest(
                localizer.translate("ErrorTemplateNoQualityProfile", &[]),
            ));
        }
    }

    let rules = templates::render(template, &answers, localizer);
    let mut creates: Vec<String> = rules
        .iter()
        .map(|rule| rule.target_category.clone())
        .filter(|category| !env.known.contains(category))
        .collect();
    creates.sort();
    creates.dedup();
    env.known.extend(creates.iter().cloned());
    let rules = rules
        .into_iter()
        .map(|rule| {
            // A category it creates has no folder, which the preview says once.
            let created = creates.contains(&rule.target_category);
            let issues = judge(&env, &rule)
                .into_iter()
                .filter(|issue| !(created && issue.key == "ValidationCategoryUnmapped"))
                .collect();
            PlannedRule { issues, rule }
        })
        .collect();
    Ok(Plan { rules, creates })
}

/// A rule the template would add, at the priority it is given, as the engine
/// reads one.
fn as_rule(template: &Template, index: usize, rule: &CreateRuleRequest) -> Rule {
    Rule {
        id: format!("template-{}-{index}", template.id),
        name: rule.name.clone(),
        description: rule.description.clone(),
        priority: rule.priority.unwrap_or_default(),
        enabled: true,
        media_type: rule.media_type.parse().unwrap_or(RuleMediaType::Both),
        conditions: rule.conditions.clone(),
        exclusions: rule.exclusions.clone(),
        match_mode: rule.match_mode,
        target_category: rule.target_category.clone(),
        instance_ids: None,
        template: Some(template.marker()),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// The category a template suggests: its name in the reader's language, or
/// in English when the library already has that one, as `kids` for a French
/// reader whose children already go there.
fn suggested_category(template: &Template, localizer: &Localizer, known: &[String]) -> String {
    let own = localizer.translate(template.category, &[]);
    let english = Localizer::new("en").translate(template.category, &[]);
    if known.contains(&english) && !known.contains(&own) { english } else { own }
}

/// Every quality profile the instances define, by name, each once.
async fn quality_profiles(pool: &sqlx::SqlitePool) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT name FROM arr_quality_profiles ORDER BY name")
        .fetch_all(pool)
        .await?)
}

fn suggested_profiles(defined: &[String]) -> Vec<String> {
    defined.iter().filter(|name| templates::reads_as_4k(name)).cloned().collect()
}

fn profiles_of(rule: &Rule) -> Option<Vec<String>> {
    rule.conditions.iter().find_map(|condition| match condition {
        Condition::QualityProfileIn(profiles) => Some(profiles.clone()),
        _ => None,
    })
}

/// The languages a rule of the foreign films template leaves alone.
fn languages_of(rule: &Rule) -> Option<Vec<String>> {
    rule.conditions.iter().find_map(|condition| match condition {
        Condition::OriginalLanguageNot(languages) => Some(languages.clone()),
        _ => None,
    })
}

/// The sources the template's rules need where no enabled source answers.
fn needs(template: &Template, rules: &[CreateRuleRequest], enabled: &[&str]) -> Vec<NeededSource> {
    let fields: Vec<_> = rules
        .iter()
        .flat_map(|rule| rule.conditions.iter().chain(&rule.exclusions))
        .filter_map(Condition::metadata_field)
        .collect();
    let suits = |provider: &&metadata::ProviderInfo| {
        fields.iter().any(|field| provider.fields.contains(field))
            && (template.relies_on.is_empty() || template.relies_on.contains(&provider.id))
    };
    if metadata::PROVIDERS.iter().filter(suits).any(|provider| enabled.contains(&provider.id)) {
        return Vec::new();
    }
    metadata::PROVIDERS
        .iter()
        .filter(suits)
        .map(|provider| NeededSource {
            id: provider.id.to_string(),
            name: provider.display_name.to_string(),
        })
        .collect()
}

/// The requesters the library's tags name, one per Seerr id, most titles
/// first, and every tag naming one.
struct Requesters {
    people: Vec<RequesterTag>,
    all: Vec<String>,
}

async fn requester_tags(state: &AppState) -> AppResult<Requesters> {
    let mut found = Requesters { people: Vec::new(), all: Vec::new() };
    let mut seen: Vec<String> = Vec::new();
    for facet in super::media::json_facets(&state.pool, "tags").await? {
        let Some((seerr_id, name)) = templates::requester_of(&facet.value) else { continue };
        found.all.push(facet.value.clone());
        if seen.contains(&seerr_id) {
            continue;
        }
        found.people.push(RequesterTag {
            category: templates::requester_category(&seerr_id, &name),
            name,
            titles: facet.count,
            tag: facet.value,
            added: false,
        });
        seen.push(seerr_id);
    }
    Ok(found)
}

/// The requesters, each with the category a rule of `added` sends them to.
fn routed(tags: &[RequesterTag], added: &[&Rule]) -> Vec<RequesterTag> {
    let seerr_id = |tag: &str| templates::requester_of(tag).map(|(id, _)| id);
    tags.iter()
        .map(|known| {
            let rule = added.iter().find(|rule| {
                rule.conditions.iter().any(|condition| match condition {
                    Condition::TagIn(listed) => {
                        listed.iter().any(|tag| seerr_id(tag) == seerr_id(&known.tag))
                    }
                    _ => false,
                })
            });
            match rule {
                Some(rule) => RequesterTag {
                    category: rule.target_category.clone(),
                    added: true,
                    ..known.clone()
                },
                None => known.clone(),
            }
        })
        .collect()
}
