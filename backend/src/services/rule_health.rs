//! Which rules are actually deciding anything, and which are unreachable.
//!
//! A first-match-by-priority engine invites exactly one mistake: a rule placed
//! below a broader one can never fire. Validation catches contradictions
//! *within* a rule — an empty condition, one no source can answer, two that
//! cannot both hold — and nothing looks between rules at all.
//!
//! The preview reports such a rule as "0 changes", which is the same thing it
//! reports for a rule that correctly changes nothing. This says which rule took
//! the items instead, which is the part a user can act on.
//!
//! Cheap, because the engine already computes it: `Evaluation.alternatives` is
//! the list of rules that matched and lost, per item. Nothing aggregated it.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppResult;
use crate::services::routing;

/// One rule, and what the library says about it.
#[derive(Debug, Serialize)]
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
    pub shadowed_by: Option<String>,
    /// It matched nothing at all — a different problem from being shadowed, and
    /// usually a condition that is too narrow rather than a priority that is
    /// too low.
    pub matched_nothing: bool,
    /// Another rule this one cannot be told apart from by the ordering.
    ///
    /// The engine breaks ties on priority, then name, then id — the last step
    /// exists because names are not unique here. Two rules sharing both leave
    /// the winner to whichever id SQLite returns first, which is not a
    /// decision anybody made, and they may target different categories.
    pub ambiguous_with: Option<String>,
    /// Another rule with the same conditions and the same target. One of the
    /// two decides nothing whatever the priorities are.
    pub duplicate_of: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RuleHealthReport {
    pub total_media: usize,
    pub rules: Vec<RuleHealth>,
}

/// Evaluate the whole library and report per rule. Writes nothing.
pub async fn report(pool: &SqlitePool) -> AppResult<RuleHealthReport> {
    // One pass over the library, evaluating exactly what the simulation would
    // and persisting nothing.
    let outcomes = routing::evaluate_library(pool).await?;

    let mut won: HashMap<String, usize> = HashMap::new();
    let mut shadowed: HashMap<String, usize> = HashMap::new();
    let mut vetoed: HashMap<String, usize> = HashMap::new();
    // (loser, winner) -> how often. The culprit is whoever took the most.
    let mut beaten_by: HashMap<(String, String), usize> = HashMap::new();

    for outcome in &outcomes {
        if let Some(winner) = &outcome.winner {
            *won.entry(winner.clone()).or_default() += 1;
            for loser in &outcome.alternatives {
                *shadowed.entry(loser.clone()).or_default() += 1;
                *beaten_by.entry((loser.clone(), winner.clone())).or_default() += 1;
            }
        } else {
            for loser in &outcome.alternatives {
                *shadowed.entry(loser.clone()).or_default() += 1;
            }
        }
        for excluded in &outcome.excluded {
            *vetoed.entry(excluded.clone()).or_default() += 1;
        }
    }

    let rules = routing::load_rules(pool).await?;
    let names: HashMap<&str, &str> =
        rules.iter().map(|r| (r.id.as_str(), r.name.as_str())).collect();

    let mut report: Vec<RuleHealth> = rules
        .iter()
        .map(|rule| {
            let won_count = won.get(&rule.id).copied().unwrap_or(0);
            let shadowed_count = shadowed.get(&rule.id).copied().unwrap_or(0);
            let vetoed_count = vetoed.get(&rule.id).copied().unwrap_or(0);
            RuleHealth {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                priority: rule.priority,
                enabled: rule.enabled,
                won: won_count,
                shadowed: shadowed_count,
                vetoed: vetoed_count,
                // Named only when the rule never won: a rule that decides some
                // items and loses others is doing its job, not being smothered.
                shadowed_by: (won_count == 0 && shadowed_count > 0)
                    .then(|| {
                        beaten_by
                            .iter()
                            .filter(|((loser, _), _)| loser == &rule.id)
                            .max_by_key(|(_, count)| **count)
                            .and_then(|((_, winner), _)| names.get(winner.as_str()).copied())
                            .map(str::to_string)
                    })
                    .flatten(),
                matched_nothing: won_count == 0 && shadowed_count == 0 && vetoed_count == 0,
                ambiguous_with: rules
                    .iter()
                    .find(|other| {
                        other.id != rule.id
                            && other.name == rule.name
                            && other.priority == rule.priority
                    })
                    .map(|other| other.name.clone()),
                duplicate_of: rules
                    .iter()
                    .find(|other| {
                        other.id != rule.id
                            && other.target_category == rule.target_category
                            && same_set(&other.conditions, &rule.conditions)
                            && same_set(&other.exclusions, &rule.exclusions)
                            && other.match_mode == rule.match_mode
                            && other.media_type == rule.media_type
                            && same_scope(&other.instance_ids, &rule.instance_ids)
                    })
                    .map(|other| other.name.clone()),
            }
        })
        .collect();

    report.sort_by_key(|r| (r.won, r.priority));

    Ok(RuleHealthReport { total_media: outcomes.len(), rules: report })
}

/// Order-insensitive equality of two condition lists: the same conditions
/// decide the same thing whatever order they were typed in, and compared as
/// lists a reordered twin went unreported.
fn same_set<T: serde::Serialize>(a: &[T], b: &[T]) -> bool {
    let key = |items: &[T]| -> Vec<String> {
        let mut keys: Vec<String> =
            items.iter().map(|item| serde_json::to_string(item).unwrap_or_default()).collect();
        keys.sort();
        keys
    };
    a.len() == b.len() && key(a) == key(b)
}

/// Two rules restricted to different instances decide different things, and
/// only one of them may be restricted at all; compared without this, twins
/// limited to two instances were called duplicates of each other.
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
