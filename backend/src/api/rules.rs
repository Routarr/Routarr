//! Rule administration: CRUD, validation, import/export and impact preview.

use super::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use sqlx::AssertSqlSafe;
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
/// rule needs to read it back; without this route, `GET /rules/{id}` is a 405.
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
    insert_rule(&state, &id, &req).await?;
    fetch_rule(&state, &id).await.map(Json)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateRuleRequest>,
) -> AppResult<Json<Rule>> {
    let issues = check(&state, &req).await?;
    reject_on_error(&issues)?;

    let result = sqlx::query(
        "UPDATE rules SET name = ?, description = ?, priority = ?, enabled = ?,
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
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule {id} not found")));
    }

    // Read back rather than echoing the request: the response then carries the
    // real `created_at` instead of an empty string.
    fetch_rule(&state, &id).await.map(Json)
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let result =
        sqlx::query("DELETE FROM rules WHERE id = ?").bind(&id).execute(&state.pool).await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Rule {id} not found")));
    }

    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// Copy a rule, disabled, so it can be edited before being switched on.
pub async fn duplicate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Rule>> {
    let source = fetch_rule(&state, &id).await?;
    let new_id = Uuid::new_v4().to_string();

    let copy = CreateRuleRequest {
        name: format!("{} (copy)", source.name),
        description: source.description.clone(),
        priority: source.priority,
        // A duplicate that fires immediately would double-classify the library.
        enabled: false,
        media_type: source.media_type.clone(),
        conditions: source.conditions.clone(),
        exclusions: source.exclusions.clone(),
        match_mode: source.match_mode,
        target_category: source.target_category.clone(),
        instance_ids: source.instance_ids.clone(),
    };

    insert_rule(&state, &new_id, &copy).await?;
    fetch_rule(&state, &new_id).await.map(Json)
}

pub async fn reorder(
    State(state): State<AppState>,
    Json(req): Json<ReorderRulesRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // The list has to name every rule: reordering a subset writes priorities in
    // the 10, 20, 30 band beside rules whose priorities were never touched, and
    // the result is an order nobody chose.
    let mut known: Vec<String> =
        sqlx::query_scalar("SELECT id FROM rules").fetch_all(&state.pool).await?;
    known.sort();
    // Compared *before* deduplication as well: folded first, `[a, a, b]` read
    // as `[a, b]`, and the loop below then wrote `a` twice — ending at 20,
    // beside `b` at 30 — and answered `reordered: 3`.
    let mut given = req.rule_ids.clone();
    given.sort();
    let listed = given.len();
    given.dedup();
    if listed != given.len() || given != known {
        return Err(AppError::BadRequest("rule_ids must list every rule exactly once".to_string()));
    }

    // One transaction: a partial reorder would leave two rules sharing a
    // priority, making the winner depend on row order.
    let mut tx = state.pool.begin().await?;
    for (index, rule_id) in req.rule_ids.iter().enumerate() {
        sqlx::query("UPDATE rules SET priority = ?, updated_at = datetime('now') WHERE id = ?")
            .bind((index as i64 + 1) * 10)
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(Json(serde_json::json!({ "reordered": req.rule_ids.len() })))
}

/// Validate a candidate rule without storing it.
pub async fn validate(
    State(state): State<AppState>,
    Json(req): Json<CreateRuleRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let issues = check(&state, &req).await?;
    Ok(Json(serde_json::json!({
        "valid": !issues.iter().any(ValidationIssue::is_error),
        "issues": issues,
    })))
}

#[derive(Debug, serde::Deserialize)]
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

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub issues: Vec<ValidationIssue>,
    /// What the library looks like with the candidate rule applied.
    pub after: serde_json::Value,
    /// What it looks like today.
    pub before: serde_json::Value,
    /// Media whose target category changes because of this rule.
    pub changed: Vec<PreviewChange>,
    pub changed_total: usize,
}

#[derive(Debug, Serialize)]
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
    let candidate =
        to_rule(req.rule_id.clone().unwrap_or_else(|| "preview".to_string()), &req.rule);

    let mut candidate_rules: Vec<Rule> =
        baseline_rules.iter().filter(|r| Some(&r.id) != req.rule_id.as_ref()).cloned().collect();
    candidate_rules.push(candidate);
    candidate_rules.sort_by_key(|r| r.priority);

    let base_options = SimulationOptions {
        instance_ids: req.instance_ids.clone().unwrap_or_default(),
        persist: false,
        persist_unchanged: true,
        language: state.language().await,
        ..Default::default()
    };

    // One load for both rule sets: everything the two evaluations read is the
    // same library, and the second costs no query.
    let library = routing::load_library(&state.pool, &base_options).await?;
    let before = routing::simulate_loaded(
        &state.pool,
        &library,
        SimulationOptions { rules_override: Some(baseline_rules), ..base_options.clone() },
    )
    .await?;
    let after = routing::simulate_loaded(
        &state.pool,
        &library,
        SimulationOptions { rules_override: Some(candidate_rules), ..base_options },
    )
    .await?;

    let before_by_media: std::collections::HashMap<&str, &Decision> =
        before.decisions.iter().map(|d| (d.media_id.as_str(), d)).collect();

    let mut changed = Vec::new();
    for decision in &after.decisions {
        let Some(previous) = before_by_media.get(decision.media_id.as_str()) else {
            continue;
        };
        if previous.target_category == decision.target_category {
            continue;
        }
        changed.push(PreviewChange {
            media_id: decision.media_id.clone(),
            media_title: decision.media_title.clone(),
            media_type: decision.media_type.clone(),
            instance_name: decision.instance_name.clone(),
            from_category: previous.target_category.clone(),
            to_category: decision.target_category.clone(),
            current_root_folder: decision.current_root_folder.clone(),
            target_root_folder: decision.target_root_folder.clone(),
            reasons: decision.reasons.clone(),
            confidence: decision.confidence,
        });
    }

    let changed_total = changed.len();
    changed.truncate(req.sample_size.clamp(1, 500));

    Ok(Json(PreviewResponse {
        issues,
        before: summarize(&before),
        after: summarize(&after),
        changed,
        changed_total,
    }))
}

fn summarize(result: &SimulationResult) -> serde_json::Value {
    serde_json::json!({
        "total_media": result.total_media,
        "moves_required": result.moves_required,
        "already_correct": result.already_correct,
        "no_category_match": result.no_category_match,
        "skipped_unmapped": result.skipped_unmapped,
        "excluded_by_rule": result.excluded_by_rule,
    })
}

/// Export every rule as a portable bundle.
pub async fn export(State(state): State<AppState>) -> AppResult<Json<RuleBundle>> {
    let rules = load_rules(&state.pool).await?;
    let mut categories: Vec<String> = rules.iter().map(|r| r.target_category.clone()).collect();
    categories.sort();
    categories.dedup();

    Ok(Json(RuleBundle {
        version: 1,
        exported_at: Some(routing::format_timestamp(chrono::Utc::now())),
        // Instance ids are host-specific; a bundle imported elsewhere would
        // silently scope its rules to instances that do not exist there.
        rules: rules.into_iter().map(|r| to_request(r, false)).collect(),
        categories,
    }))
}

/// Import a bundle, optionally replacing the current rule set.
pub async fn import(
    State(state): State<AppState>,
    Json(req): Json<ImportRulesRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if req.bundle.version != 1 {
        return Err(AppError::BadRequest(format!(
            "Unsupported bundle version {}; this Routarr understands version 1",
            req.bundle.version
        )));
    }
    if req.bundle.rules.is_empty() {
        return Err(AppError::BadRequest("The bundle contains no rule".into()));
    }

    let mut skipped = Vec::new();

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
            .map(|r| r.target_category.trim())
            .chain(req.bundle.categories.iter().map(|c| c.trim()))
            .filter(|c| !c.is_empty())
            .collect();
        for raw in referenced {
            match super::categories::normalise(raw) {
                Ok(name) => {
                    creating.insert(name);
                }
                Err(e) => skipped.push(format!("category {raw:?}: {e}")),
            }
        }
    }

    // The same validator every other write path runs. Three ad-hoc checks stood
    // in the loop below, so a bundle could carry what the editor refuses: an
    // inverted year range, a negative day count, a condition with no operand.
    // Read before the transaction opens: a pool of one connection, which is
    // what the tests run on, cannot serve a query while a transaction holds it.
    let mut env = environment(&state).await?;

    // Judged against the table as it will be once this commits, not as it was
    // read: a rule targeting a category the bundle brings is otherwise refused
    // as naming one that does not exist. Only the names the table lacks are
    // written — a round-trip import names every category it already has.
    let created: Vec<String> =
        creating.into_iter().filter(|name| !env.known.contains(name)).collect();
    env.known.extend(created.iter().cloned());

    let mut tx = state.pool.begin().await?;

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

    let mut imported = 0usize;
    for rule in &req.bundle.rules {
        let errors: Vec<String> = judge(&env, rule)
            .into_iter()
            .filter(ValidationIssue::is_error)
            .map(|issue| issue.message)
            .collect();
        if !errors.is_empty() {
            skipped.push(format!("'{}': {}", rule.name, errors.join("; ")));
            continue;
        }

        sqlx::query(
            "INSERT INTO rules (id, name, description, priority, enabled, media_type, conditions,
             exclusions, match_mode, target_category, instance_ids)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(rule.name.trim())
        .bind(&rule.description)
        .bind(rule.priority)
        .bind(rule.enabled)
        .bind(rule.media_type.to_lowercase())
        .bind(serde_json::to_string(&rule.conditions)?)
        .bind(serde_json::to_string(&rule.exclusions)?)
        .bind(rule.match_mode.to_string())
        .bind(rule.target_category.trim().to_lowercase())
        .bind(encode_instance_ids(&rule.instance_ids)?)
        .execute(&mut *tx)
        .await?;

        imported += 1;
    }

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "imported": imported,
        "replaced": req.replace,
        "skipped": skipped,
    })))
}

/// Everything a rule is judged against, read once.
///
/// Loaded apart from the judging because an import validates a whole bundle:
/// reading the categories, the mappings and the source coverage per rule would
/// be a query per item on a path that already has all of them.
struct Environment {
    known: Vec<String>,
    mapped: Vec<String>,
    covered_fields: Vec<MetadataField>,
    localizer: crate::localization::Localizer,
    current_year: i64,
}

async fn environment(state: &AppState) -> AppResult<Environment> {
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
    })
}

/// Run the shared validator against one loaded environment.
fn judge(env: &Environment, req: &CreateRuleRequest) -> Vec<ValidationIssue> {
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
        let params: Vec<(&str, &str)> =
            issue.params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        issue.message = env.localizer.translate(&issue.key, &params);
        issue
    })
    .collect()
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
        errors.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; "),
    ))
}

async fn insert_rule(state: &AppState, id: &str, req: &CreateRuleRequest) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO rules (id, name, description, priority, enabled, media_type, conditions,
         exclusions, match_mode, target_category, instance_ids)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
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
    .execute(&state.pool)
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

/// `None` and `Some([])` both mean "all instances"; store NULL for both so the
/// loader does not have to special-case an empty array.
fn encode_instance_ids(ids: &Option<Vec<String>>) -> AppResult<Option<String>> {
    match ids {
        Some(ids) if !ids.is_empty() => Ok(Some(serde_json::to_string(ids)?)),
        _ => Ok(None),
    }
}

fn to_rule(id: String, req: &CreateRuleRequest) -> Rule {
    Rule {
        id,
        name: req.name.clone(),
        description: req.description.clone(),
        priority: req.priority,
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

fn to_request(rule: Rule, keep_instance_ids: bool) -> CreateRuleRequest {
    CreateRuleRequest {
        name: rule.name,
        description: rule.description,
        priority: rule.priority,
        enabled: rule.enabled,
        media_type: rule.media_type,
        conditions: rule.conditions,
        exclusions: rule.exclusions,
        match_mode: rule.match_mode,
        target_category: rule.target_category,
        instance_ids: if keep_instance_ids { rule.instance_ids } else { None },
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
