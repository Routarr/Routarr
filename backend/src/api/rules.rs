//! Rule administration: CRUD, validation, import/export and impact preview.

use super::{Deleted, Json};
use axum::extract::State;

use super::Path;
use serde::Serialize;
use sqlx::AssertSqlSafe;
use std::collections::HashMap;
use uuid::Uuid;

use chrono::Datelike;

use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::services::metadata;
use crate::services::routing::{self, RULE_COLUMNS, SimulationOptions, load_rules, rule_from_row};
use crate::services::rule_engine;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> AppResult<Json<Vec<Rule>>> {
    Ok(Json(load_rules(&state.pool).await?))
}

/// Fetch one rule.
///
/// The list endpoint covers the UI, but an external script that just created a
/// rule needs to read it back. Without this route, `GET /rules/{id}` is a 405.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Rule>> {
    fetch_rule(&state, &id).await.map(Json)
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateRuleRequest>,
) -> AppResult<Json<Rule>> {
    let issues = check(&state, &req).await?;
    reject_on_error(&issues)?;

    let id = Uuid::new_v4().to_string();
    let mut tx = target_held(&state, &req).await?;
    insert_rule(&mut tx, &id, &req).await?;
    tx.commit().await?;
    fetch_rule(&state, &id).await.map(Json)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateRuleRequest>,
) -> AppResult<Json<Rule>> {
    let issues = check(&state, &req).await?;
    reject_on_error(&issues)?;

    let mut tx = target_held(&state, &req).await?;
    let result = sqlx::query(
        "UPDATE rules SET name = ?, description = ?, priority = COALESCE(?, priority), enabled = ?,
         media_type = ?, conditions = ?, exclusions = ?, match_mode = ?,
         target_category = ?, instance_ids = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(req.name.trim())
    .bind(&req.description)
    .bind(req.priority)
    .bind(req.enabled)
    .bind(req.media_type.to_lowercase())
    .bind(serde_json::to_string(&req.conditions)?)
    .bind(serde_json::to_string(&req.exclusions)?)
    .bind(req.match_mode.to_string())
    .bind(req.target_category.trim().to_lowercase())
    .bind(encode_instance_ids(&req.instance_ids)?)
    .bind(&id)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule {id} not found")));
    }
    tx.commit().await?;

    // Read back rather than echoing the request: the response then carries the
    // real `created_at` instead of an empty string.
    fetch_rule(&state, &id).await.map(Json)
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Deleted>> {
    let result =
        sqlx::query("DELETE FROM rules WHERE id = ?").bind(&id).execute(&state.pool).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule {id} not found")));
    }

    Ok(Json(Deleted { deleted: true }))
}

/// Copy a rule, disabled and last, so it can be edited before being switched
/// on. The copy is checked as any rule written is.
pub async fn duplicate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Rule>> {
    let source = fetch_rule(&state, &id).await?;
    let new_id = Uuid::new_v4().to_string();

    let copy = CreateRuleRequest {
        name: copy_name(&state.localizer().await, &source.name),
        // A duplicate that fires immediately would double-classify the library.
        enabled: false,
        priority: None,
        ..to_request(source)
    };
    reject_on_error(&check(&state, &copy).await?)?;

    let mut tx = target_held(&state, &copy).await?;
    insert_rule(&mut tx, &new_id, &copy).await?;
    tx.commit().await?;
    fetch_rule(&state, &new_id).await.map(Json)
}

/// A copy's name in the reader's words, its source's name cut short enough
/// for the whole to stay a name a rule may carry.
fn copy_name(localizer: &crate::localization::Localizer, name: &str) -> String {
    let suffix = localizer.translate("RuleCopyName", &[("name", "")]).chars().count();
    let kept: String =
        name.chars().take(rule_engine::MAX_NAME_LENGTH.saturating_sub(suffix)).collect();
    localizer.translate("RuleCopyName", &[("name", kept.trim_end())])
}

pub async fn reorder(
    State(state): State<AppState>,
    Json(req): Json<ReorderRulesRequest>,
) -> AppResult<Json<Reordered>> {
    // The list has to name every rule: reordering a subset writes priorities in
    // the 10, 20, 30 band beside rules whose priorities were never touched, and
    // the result is an order nobody chose. Read under the write lock, with the
    // writes: a rule created or deleted in between would make the list wrong.
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    let mut known: Vec<String> =
        sqlx::query_scalar("SELECT id FROM rules").fetch_all(&mut *tx).await?;
    known.sort();
    // Compared *before* deduplication as well: folded first, `[a, a, b]` would
    // read as `[a, b]`, and the loop below would write `a` twice (ending at 20,
    // beside `b` at 30) and answer `reordered: 3`.
    let mut given = req.rule_ids.clone();
    given.sort();
    let listed = given.len();
    given.dedup();
    if listed != given.len() || given != known {
        return Err(AppError::BadRequest("rule_ids must list every rule exactly once".to_string()));
    }

    crate::race::checked("rules::reorder", "").await;
    // One transaction: a partial reorder would leave two rules sharing a
    // priority, making the winner depend on row order.
    for (index, rule_id) in req.rule_ids.iter().enumerate() {
        sqlx::query("UPDATE rules SET priority = ?, updated_at = datetime('now') WHERE id = ?")
            .bind((index as i64 + 1) * PRIORITY_STEP)
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(Json(Reordered { reordered: req.rule_ids.len() }))
}

/// How many rules a reorder numbered.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Reordered {
    pub reordered: usize,
}

/// What the server makes of a draft rule.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleVerdict {
    /// False when an issue is an error: the rule would be refused.
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

/// Validate a candidate rule without storing it.
pub async fn validate(
    State(state): State<AppState>,
    Json(req): Json<CreateRuleRequest>,
) -> AppResult<Json<RuleVerdict>> {
    let issues = check(&state, &req).await?;
    Ok(Json(RuleVerdict { valid: !issues.iter().any(ValidationIssue::is_error), issues }))
}

/// What an import did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleImportReport {
    pub imported: usize,
    /// The rules in place were removed first.
    pub replaced: bool,
    /// Each rule left out, with why.
    pub skipped: Vec<String>,
    /// Each rule imported otherwise than the bundle has it, with how: limited
    /// to fewer instances, or switched off until its scope is checked.
    pub adjusted: Vec<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct PreviewRequest {
    /// The rule being edited. When `rule_id` is set it replaces that rule,
    /// otherwise it is appended to the current set.
    pub rule: CreateRuleRequest,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub instance_ids: Option<Vec<String>>,
    #[serde(default = "default_sample")]
    pub sample_size: usize,
}

fn default_sample() -> usize {
    25
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewResponse {
    pub issues: Vec<ValidationIssue>,
    /// What the library looks like with the candidate rule applied.
    pub after: PreviewSummary,
    /// What it looks like today.
    pub before: PreviewSummary,
    /// Media whose target category changes because of this rule.
    pub changed: Vec<PreviewChange>,
    pub changed_total: usize,
}

/// What one rule set decides over the library, counted.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewSummary {
    pub total_media: usize,
    pub moves_required: usize,
    pub already_correct: usize,
    pub no_category_match: usize,
    pub skipped_unmapped: usize,
    pub excluded_by_rule: usize,
}

impl PreviewSummary {
    fn of(total_media: usize, counted: &routing::Counters) -> Self {
        Self {
            total_media,
            moves_required: counted.moves_required,
            already_correct: counted.already_correct,
            no_category_match: counted.no_category_match,
            skipped_unmapped: counted.skipped_unmapped,
            excluded_by_rule: counted.excluded_by_rule,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewChange {
    pub media_id: String,
    pub media_title: String,
    pub media_type: String,
    pub instance_name: Option<String>,
    pub from_category: String,
    pub to_category: String,
    pub current_root_folder: Option<String>,
    pub target_root_folder: Option<String>,
    pub reasons: Vec<String>,
    pub confidence: f32,
}

/// Show the impact of a rule change before it is saved. Persists nothing.
pub async fn preview(
    State(state): State<AppState>,
    Json(req): Json<PreviewRequest>,
) -> AppResult<Json<PreviewResponse>> {
    let issues = check(&state, &req.rule).await?;

    let baseline_rules = load_rules(&state.pool).await?;
    // Placed where saving it would place it: an edited rule where it stands,
    // a new one after every other.
    let priority = req.rule.priority.unwrap_or_else(|| {
        let edited = baseline_rules.iter().find(|r| Some(&r.id) == req.rule_id.as_ref());
        edited.map_or_else(
            || baseline_rules.iter().map(|r| r.priority).max().unwrap_or(0) + PRIORITY_STEP,
            |rule| rule.priority,
        )
    });
    let candidate =
        to_rule(req.rule_id.clone().unwrap_or_else(|| "preview".to_string()), &req.rule, priority);

    let mut candidate_rules: Vec<Rule> =
        baseline_rules.iter().filter(|r| Some(&r.id) != req.rule_id.as_ref()).cloned().collect();
    candidate_rules.push(candidate);

    let scope = SimulationOptions {
        instance_ids: req.instance_ids.clone().unwrap_or_default(),
        ..Default::default()
    };
    // One load for both rule sets: everything the two evaluations read is the
    // same library, and the second costs no query.
    let library = routing::load_library(&state.pool, &scope).await?;
    let compared = routing::compare(
        &library,
        baseline_rules,
        candidate_rules,
        req.sample_size.clamp(1, 500),
        &state.language().await,
    )
    .await?;

    let changed = compared
        .changed
        .into_iter()
        .map(|(from_category, decision)| PreviewChange {
            media_id: decision.media_id,
            media_title: decision.media_title,
            media_type: decision.media_type,
            instance_name: decision.instance_name,
            from_category,
            to_category: decision.target_category,
            current_root_folder: decision.current_root_folder,
            target_root_folder: decision.target_root_folder,
            reasons: decision.reasons,
            confidence: decision.confidence,
        })
        .collect();

    Ok(Json(PreviewResponse {
        issues,
        before: PreviewSummary::of(compared.total_media, &compared.before),
        after: PreviewSummary::of(compared.total_media, &compared.after),
        changed,
        changed_total: compared.changed_total,
    }))
}

/// The gap a reorder leaves between two rules, and a new rule after the last.
const PRIORITY_STEP: i64 = 10;

/// The bundle format this build writes. It reads 1 as well.
const BUNDLE_VERSION: u32 = 2;

/// Export every rule as a portable bundle.
pub async fn export(State(state): State<AppState>) -> AppResult<Json<RuleBundle>> {
    let rules = bundled_rules(&state.pool).await?;
    let mut categories: Vec<String> =
        rules.iter().map(|r| r.rule.target_category.clone()).collect();
    categories.sort();
    categories.dedup();

    Ok(Json(RuleBundle {
        version: BUNDLE_VERSION,
        exported_at: Some(exported_now()),
        rules,
        categories,
    }))
}

/// When a bundle is written, as another installation reads it: in RFC 3339,
/// its zone stated.
pub(crate) fn exported_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Every rule as a bundle carries it, its scope by instance name.
pub(crate) async fn bundled_rules(pool: &sqlx::SqlitePool) -> AppResult<Vec<BundledRule>> {
    let names: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT id, name FROM instances")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    Ok(load_rules(pool)
        .await?
        .into_iter()
        .map(|rule| {
            let instance_names = rule
                .instance_ids
                .as_ref()
                .map(|ids| ids.iter().filter_map(|id| names.get(id).cloned()).collect());
            let rule = CreateRuleRequest { instance_ids: None, ..to_request(rule) };
            BundledRule { rule, instance_names }
        })
        .collect())
}

/// A bundled rule as this installation would store it, or why it cannot be.
pub(crate) enum Scoped {
    /// To store, with what was changed on the way, if anything.
    Kept(CreateRuleRequest, Option<String>),
    Refused(String),
}

/// Give a bundled rule its scope on this installation, from `ids`, each
/// instance's id by its name. A rule limited to instances none of which is
/// here is refused: unscoped, it would route every instance. A format that
/// does not carry the names leaves a rule with no ids switched off until
/// someone checks its scope.
pub(crate) fn scope_here(
    bundled: &BundledRule,
    names_carried: bool,
    ids: &HashMap<String, String>,
    localizer: &crate::localization::Localizer,
) -> Scoped {
    let rule = &bundled.rule;
    let listed = |names: &[&String]| {
        let names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
        names.join(&localizer.translate("ListSeparator", &[]))
    };
    match &bundled.instance_names {
        Some(names) => {
            let (found, missing): (Vec<&String>, Vec<&String>) =
                names.iter().partition(|name| ids.contains_key(*name));
            if found.is_empty() {
                return Scoped::Refused(localizer.translate(
                    "RuleImportInstancesMissing",
                    &[("name", &rule.name), ("instances", &listed(&missing))],
                ));
            }
            let narrowed = (!missing.is_empty()).then(|| {
                localizer.translate(
                    "RuleImportInstancesNarrowed",
                    &[("name", &rule.name), ("instances", &listed(&missing))],
                )
            });
            let scope = found.into_iter().map(|name| ids[name].clone()).collect();
            Scoped::Kept(CreateRuleRequest { instance_ids: Some(scope), ..rule.clone() }, narrowed)
        }
        None if !names_carried
            && rule.enabled
            && rule.instance_ids.as_ref().is_none_or(Vec::is_empty) =>
        {
            let note = localizer.translate("RuleImportScopeUnknown", &[("name", &rule.name)]);
            Scoped::Kept(CreateRuleRequest { enabled: false, ..rule.clone() }, Some(note))
        }
        None => Scoped::Kept(rule.clone(), None),
    }
}

/// Import a bundle, optionally replacing the current rule set.
pub async fn import(
    State(state): State<AppState>,
    Json(req): Json<ImportRulesRequest>,
) -> AppResult<Json<RuleImportReport>> {
    let version = req.bundle.version;
    if !(1..=BUNDLE_VERSION).contains(&version) {
        return Err(AppError::BadRequest(format!(
            "Unsupported bundle version {version}. This Routarr understands versions 1 to \
             {BUNDLE_VERSION}"
        )));
    }
    if req.bundle.rules.is_empty() {
        return Err(AppError::BadRequest("The bundle contains no rule".into()));
    }

    let mut skipped = Vec::new();
    let mut adjusted = Vec::new();
    let localizer = state.localizer().await;

    // Through the gate `POST /categories` applies: a name that skips it lands
    // in the table out of `rename`'s reach. A refused name is reported here,
    // and the rule targeting it falls to the validator below, which does not
    // know it.
    let mut creating = std::collections::BTreeSet::new();
    if req.create_missing_categories {
        let referenced: std::collections::BTreeSet<&str> = req
            .bundle
            .rules
            .iter()
            .map(|r| r.rule.target_category.trim())
            .chain(req.bundle.categories.iter().map(|c| c.trim()))
            .filter(|c| !c.is_empty())
            .collect();
        for raw in referenced {
            match super::categories::normalise(raw, &localizer) {
                Ok(name) => {
                    creating.insert(name);
                }
                Err(e) => skipped.push(format!("category {raw:?}: {e}")),
            }
        }
    }

    // The same validator every other write path runs, so a bundle cannot carry
    // what the editor refuses: an inverted year range, a negative day count, a
    // condition with no operand.
    // Read before the transaction opens: a pool of one connection, which is
    // what the tests run on, cannot serve a query while a transaction holds it.
    let mut env = environment(&state).await?;
    let ids: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT name, id FROM instances")
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect();

    // Judged against the table as it will be once this commits, not as it was
    // read: a rule targeting a category the bundle brings is otherwise refused
    // as naming one that does not exist. Only the names the table lacks are
    // written: a round-trip import names every category it already has.
    let created: Vec<String> =
        creating.into_iter().filter(|name| !env.known.contains(name)).collect();
    env.known.extend(created.iter().cloned());

    // Judged whole before anything is written: a replace in which no rule
    // survives would delete every rule and add none, and the next pass would
    // route the whole library to the fallback category.
    let mut importable = Vec::new();
    for bundled in &req.bundle.rules {
        let (rule, note) = match scope_here(bundled, version >= 2, &ids, &localizer) {
            Scoped::Kept(rule, note) => (rule, note),
            Scoped::Refused(why) => {
                skipped.push(why);
                continue;
            }
        };
        let errors: Vec<String> = judge(&env, &rule)
            .into_iter()
            .filter(ValidationIssue::is_error)
            .map(|issue| issue.message)
            .collect();
        if errors.is_empty() {
            adjusted.extend(note);
            importable.push(rule);
        } else {
            skipped.push(format!("'{}': {}", rule.name, errors.join(" · ")));
        }
    }
    if req.replace && importable.is_empty() {
        return Err(AppError::BadRequest(format!(
            "No rule of the bundle can be imported, so the rules in place are kept. {}",
            skipped.join(" · ")
        )));
    }

    let mut tx = crate::db::write_transaction(&state.pool).await?;

    for name in &created {
        sqlx::query(
            "INSERT INTO categories (id, name, description) VALUES (?, ?, 'Imported with a rule bundle')
             ON CONFLICT(name) DO NOTHING",
        )
        .bind(format!("cat-{}", Uuid::new_v4()))
        .bind(name)
        .execute(&mut *tx)
        .await?;
    }

    if req.replace {
        sqlx::query("DELETE FROM rules").execute(&mut *tx).await?;
    }

    // Each target checked again under the write lock: one read before it could
    // have been removed since, and the rule would name nothing.
    let mut imported = 0;
    for rule in &importable {
        let target = rule.target_category.trim().to_lowercase();
        if let Err(refused) = super::categories::ensure_exists(&mut tx, &target, &localizer).await {
            skipped.push(format!("'{}': {refused}", rule.name));
            continue;
        }
        insert_rule(&mut tx, &Uuid::new_v4().to_string(), rule).await?;
        imported += 1;
    }

    tx.commit().await?;

    Ok(Json(RuleImportReport { imported, replaced: req.replace, skipped, adjusted }))
}

/// Everything a rule is judged against, read once.
///
/// Loaded apart from the judging because an import validates a whole bundle:
/// reading the categories, the mappings and the source coverage per rule would
/// be a query per item on a path that already has all of them.
pub(crate) struct Environment {
    pub(crate) known: Vec<String>,
    mapped: Vec<String>,
    covered_fields: Vec<MetadataField>,
    localizer: crate::localization::Localizer,
    current_year: i64,
    /// The ids a rule's scope may name.
    instances: Vec<String>,
}

pub(crate) async fn environment(state: &AppState) -> AppResult<Environment> {
    Ok(Environment {
        known: sqlx::query_scalar("SELECT name FROM categories").fetch_all(&state.pool).await?,
        mapped: sqlx::query_scalar(
            "SELECT DISTINCT category FROM root_folders
             WHERE category IS NOT NULL AND category != ''",
        )
        .fetch_all(&state.pool)
        .await?,
        // What the enabled sources can answer between them: a keyword rule with
        // Radarr alone is unanswerable, a genre rule is not, and telling them
        // apart is the difference between a useful warning and a superstition.
        covered_fields: metadata::covered_fields(&state.metadata_providers().await),
        localizer: state.localizer().await,
        current_year: i64::from(chrono::Utc::now().year()),
        instances: sqlx::query_scalar("SELECT id FROM instances").fetch_all(&state.pool).await?,
    })
}

/// Run the shared validator against one loaded environment.
pub(crate) fn judge(env: &Environment, req: &CreateRuleRequest) -> Vec<ValidationIssue> {
    let target_category = req.target_category.trim().to_lowercase();

    rule_engine::validate_rule(
        rule_engine::RuleDraft {
            name: &req.name,
            media_type: &req.media_type,
            match_mode: req.match_mode,
            conditions: &req.conditions,
            exclusions: &req.exclusions,
            target_category: &target_category,
        },
        rule_engine::ValidationEnv {
            known_categories: &env.known,
            mapped_categories: &env.mapped,
            covered_fields: &env.covered_fields,
            current_year: env.current_year,
        },
    )
    .into_iter()
    .map(|mut issue| {
        issue.message = describe_issue(&env.localizer, &issue.key, &issue.params);
        issue
    })
    // An id the interface sent, never one somebody typed, so in English.
    .chain(req.instance_ids.iter().flatten().filter(|id| !env.instances.contains(id)).map(|id| {
        ValidationIssue {
            severity: "error".into(),
            field: "instance_ids".into(),
            key: "UnknownInstance".into(),
            params: std::collections::BTreeMap::new(),
            message: format!("No instance has the id {id}."),
        }
    }))
    .collect()
}

/// An issue in the reader's words: a condition is named by its caption, and by
/// its section and place when it has one, never by the engine's identifier.
fn describe_issue(
    localizer: &crate::localization::Localizer,
    key: &str,
    raw: &std::collections::BTreeMap<String, String>,
) -> String {
    let caption = |kind: &str| super::conditions::caption(localizer, kind);
    let mut params: Vec<(String, String)> = raw
        .iter()
        .map(|(name, value)| match name.as_str() {
            "kind" | "first" | "second" => (name.clone(), caption(value)),
            _ => (name.clone(), value.clone()),
        })
        .collect();
    if let (Some(index), Some(kind)) = (raw.get("index"), raw.get("kind")) {
        let reference = match raw.get("section").map(String::as_str) {
            Some("exclusions") => "ExclusionReference",
            _ => "ConditionReference",
        };
        let named = localizer.translate(reference, &[("index", index), ("label", &caption(kind))]);
        params.push(("condition".to_string(), named));
    }
    let params: Vec<(&str, &str)> =
        params.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
    localizer.translate(key, &params)
}

/// The write transaction a rule is stored in, its target category checked
/// again inside it: validated before, the category could be removed or
/// renamed before the rule names it.
async fn target_held(
    state: &AppState,
    req: &CreateRuleRequest,
) -> AppResult<sqlx::Transaction<'static, sqlx::Sqlite>> {
    let target = req.target_category.trim().to_lowercase();
    // Before the transaction: the language is a read of its own.
    let localizer = state.localizer().await;
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    super::categories::ensure_exists(&mut tx, &target, &localizer).await?;
    crate::race::checked("rules::write", &target).await;
    Ok(tx)
}

/// Run the shared validator against the live categories and mappings.
async fn check(state: &AppState, req: &CreateRuleRequest) -> AppResult<Vec<ValidationIssue>> {
    Ok(judge(&environment(state).await?, req))
}

fn reject_on_error(issues: &[ValidationIssue]) -> AppResult<()> {
    let errors: Vec<&ValidationIssue> = issues.iter().filter(|i| i.is_error()).collect();
    if errors.is_empty() {
        return Ok(());
    }
    Err(AppError::BadRequest(
        errors.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join(" · "),
    ))
}

pub(crate) async fn insert_rule(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
    req: &CreateRuleRequest,
) -> AppResult<()> {
    // A rule given no priority goes after every other: one at a fixed default
    // would tie with others, and the name would pick the winner.
    sqlx::query(
        "INSERT INTO rules (id, name, description, priority, enabled, media_type, conditions,
         exclusions, match_mode, target_category, instance_ids)
         VALUES (?, ?, ?, COALESCE(?, (SELECT COALESCE(MAX(priority), 0) + ? FROM rules)),
                 ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(req.name.trim())
    .bind(&req.description)
    .bind(req.priority)
    .bind(PRIORITY_STEP)
    .bind(req.enabled)
    .bind(req.media_type.to_lowercase())
    .bind(serde_json::to_string(&req.conditions)?)
    .bind(serde_json::to_string(&req.exclusions)?)
    .bind(req.match_mode.to_string())
    .bind(req.target_category.trim().to_lowercase())
    .bind(encode_instance_ids(&req.instance_ids)?)
    .execute(connection)
    .await?;

    Ok(())
}

async fn fetch_rule(state: &AppState, id: &str) -> AppResult<Rule> {
    sqlx::query_as(AssertSqlSafe(format!("SELECT {RULE_COLUMNS} FROM rules WHERE id = ?")))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .map(rule_from_row)
        .ok_or_else(|| AppError::NotFound(format!("Rule {id} not found")))
}

/// `None` and `Some([])` both mean "all instances". NULL is stored for both so
/// the loader does not have to special-case an empty array.
fn encode_instance_ids(ids: &Option<Vec<String>>) -> AppResult<Option<String>> {
    match ids {
        Some(ids) if !ids.is_empty() => Ok(Some(serde_json::to_string(ids)?)),
        _ => Ok(None),
    }
}

fn to_rule(id: String, req: &CreateRuleRequest, priority: i64) -> Rule {
    Rule {
        id,
        name: req.name.clone(),
        description: req.description.clone(),
        priority,
        enabled: req.enabled,
        media_type: req.media_type.to_lowercase(),
        conditions: req.conditions.clone(),
        exclusions: req.exclusions.clone(),
        match_mode: req.match_mode,
        target_category: req.target_category.trim().to_lowercase(),
        instance_ids: req.instance_ids.clone(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

pub(crate) fn to_request(rule: Rule) -> CreateRuleRequest {
    CreateRuleRequest {
        name: rule.name,
        description: rule.description,
        priority: Some(rule.priority),
        enabled: rule.enabled,
        media_type: rule.media_type,
        conditions: rule.conditions,
        exclusions: rule.exclusions,
        match_mode: rule.match_mode,
        target_category: rule.target_category,
        instance_ids: rule.instance_ids,
    }
}

/// Which rules are deciding anything, and which are unreachable.
///
/// A read-only pass: it evaluates, counts, and writes nothing.
pub async fn health(
    State(state): State<AppState>,
) -> AppResult<Json<crate::services::rule_health::RuleHealthReport>> {
    Ok(Json(crate::services::rule_health::report(&state.pool).await?))
}
