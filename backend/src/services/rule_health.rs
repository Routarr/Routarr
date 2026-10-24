//! Which rules decide anything, and which are unreachable.
//!
//! Validation looks inside one rule. This looks between rules: a rule below a broader one
//! never fires, and the report names the rule that took its items, counted off
//! `Evaluation.alternatives`, the rules that matched an item and lost.

use std::cmp::Reverse;
use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppResult;
use crate::models::Rule;
use crate::services::routing;

/// One rule, and what the library says about it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleHealth {
    pub rule_id: String,
    pub rule_name: String,
    pub priority: i64,
    pub enabled: bool,
    /// Items this rule decided.
    pub won: usize,
    /// Items it matched and lost on priority.
    pub shadowed: usize,
    /// Items it matched and had vetoed by one of its own exclusions.
    pub vetoed: usize,
    /// The rule that took the most of what this one matched, when it never won.
    /// Of two that took as many, the one tried first.
    pub shadowed_by: Option<String>,
    /// The id of the rule `shadowed_by` names, since two rules may share a name.
    pub shadowed_by_id: Option<String>,
    /// It matched nothing at all, usually a condition too narrow. Never said of a disabled rule,
    /// which no run reads.
    pub matched_nothing: bool,
    /// Another rule with the same priority and the same name, so that which
    /// of the two wins is no decision anybody made.
    // The engine breaks a tie on priority and name by the id, which is random.
    pub ambiguous_with: Option<String>,
    /// The id of the rule `ambiguous_with` names, which shares its name.
    pub ambiguous_with_id: Option<String>,
    /// Another rule with the same conditions and the same target. One of the
    /// two decides nothing whatever the priorities are.
    pub duplicate_of: Option<String>,
    /// The id of the rule `duplicate_of` names.
    pub duplicate_of_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleHealthReport {
    pub total_media: usize,
    pub rules: Vec<RuleHealth>,
}

/// Evaluate the whole library and report per rule. Writes nothing.
pub async fn report(
    pool: &SqlitePool,
    passes: &routing::LibraryPasses,
) -> AppResult<RuleHealthReport> {
    let outcomes = routing::evaluate_library(pool, passes).await?;

    let mut won: HashMap<String, usize> = HashMap::new();
    let mut shadowed: HashMap<String, usize> = HashMap::new();
    let mut vetoed: HashMap<String, usize> = HashMap::new();
    // loser -> winner -> how often.
    let mut beaten_by: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for outcome in &outcomes {
        if let Some(winner) = &outcome.winner {
            *won.entry(winner.clone()).or_default() += 1;
            for loser in &outcome.alternatives {
                *shadowed.entry(loser.clone()).or_default() += 1;
                let by_winner = beaten_by.entry(loser.clone()).or_default();
                *by_winner.entry(winner.clone()).or_default() += 1;
            }
        }
        for excluded in &outcome.excluded {
            *vetoed.entry(excluded.clone()).or_default() += 1;
        }
    }

    let rules = routing::load_rules(pool).await?;

    let mut report: Vec<RuleHealth> = rules
        .iter()
        .map(|rule| {
            let won_count = won.get(&rule.id).copied().unwrap_or(0);
            let shadowed_count = shadowed.get(&rule.id).copied().unwrap_or(0);
            let vetoed_count = vetoed.get(&rule.id).copied().unwrap_or(0);
            // Named only when the rule never won: one that wins some items is doing its job.
            let shadowed_by = (won_count == 0 && shadowed_count > 0)
                .then(|| culprit(&rules, beaten_by.get(&rule.id)?))
                .flatten();
            let ambiguous_with = rules.iter().find(|other| {
                other.id != rule.id && other.name == rule.name && other.priority == rule.priority
            });
            let duplicate_of = rules.iter().find(|other| {
                other.id != rule.id
                    && other.target_category == rule.target_category
                    && same_set(&other.conditions, &rule.conditions)
                    && same_set(&other.exclusions, &rule.exclusions)
                    && other.match_mode == rule.match_mode
                    && other.media_type == rule.media_type
                    && same_scope(&other.instance_ids, &rule.instance_ids)
            });
            let name = |other: Option<&Rule>| other.map(|other| other.name.clone());
            let id = |other: Option<&Rule>| other.map(|other| other.id.clone());
            RuleHealth {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                priority: rule.priority,
                enabled: rule.enabled,
                won: won_count,
                shadowed: shadowed_count,
                vetoed: vetoed_count,
                shadowed_by: name(shadowed_by),
                shadowed_by_id: id(shadowed_by),
                matched_nothing: rule.enabled
                    && won_count == 0
                    && shadowed_count == 0
                    && vetoed_count == 0,
                ambiguous_with: name(ambiguous_with),
                ambiguous_with_id: id(ambiguous_with),
                duplicate_of: name(duplicate_of),
                duplicate_of_id: id(duplicate_of),
            }
        })
        .collect();

    report.sort_by_key(|r| (r.won, r.priority));

    Ok(RuleHealthReport { total_media: outcomes.len(), rules: report })
}

/// The rule that beat a loser most often. A tie goes to the first of `rules`, in the engine's
/// order: read off the `HashMap`, the name would change from one report to the next.
fn culprit<'a>(rules: &'a [Rule], beaten_by: &HashMap<String, usize>) -> Option<&'a Rule> {
    rules
        .iter()
        .filter_map(|winner| Some((Reverse(*beaten_by.get(&winner.id)?), winner)))
        .min_by_key(|(count, _)| *count)
        .map(|(_, winner)| winner)
}

/// Order-insensitive equality of two condition lists, so a reordered twin is still a duplicate.
fn same_set<T: serde::Serialize>(a: &[T], b: &[T]) -> bool {
    let key = |items: &[T]| -> Vec<String> {
        let mut keys: Vec<String> =
            items.iter().map(|item| serde_json::to_string(item).unwrap_or_default()).collect();
        keys.sort();
        keys
    };
    a.len() == b.len() && key(a) == key(b)
}

/// The same instance scope, whatever the order of its ids.
fn same_scope(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let (mut a, mut b) = (a.clone(), b.clone());
            a.sort();
            b.sort();
            a == b
        }
        _ => false,
    }
}
