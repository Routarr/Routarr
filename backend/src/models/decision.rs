use serde::{Deserialize, Serialize};

/// A routing decision computed by the rule engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub media_id: String,
    pub media_title: String,
    pub media_type: String,
    pub instance_id: String,
    pub instance_name: Option<String>,
    pub current_root_folder: Option<String>,
    pub target_root_folder: Option<String>,
    pub target_category: String,
    pub matched_rule_id: Option<String>,
    pub matched_rule_name: Option<String>,
    pub is_override: bool,
    pub reasons: Vec<String>,
    pub alternatives: Vec<AlternativeDecision>,
    /// `move`, `none` or `skip`.
    pub action: String,
    /// `pending`, `applied`, `failed` or `skipped` — a reverted move is `skipped` with `reverted_at` set.
    pub status: String,
    /// 0.0–1.0, how many independent signals backed the winning rule.
    #[serde(default)]
    pub confidence: f32,
    /// True once a newer simulation replaced this proposal.
    #[serde(default)]
    pub superseded: bool,
    /// Groups every decision produced by the same simulation run.
    #[serde(default)]
    pub simulation_id: Option<String>,
    pub error_message: Option<String>,
    pub decided_at: String,
    pub applied_at: Option<String>,
    /// Set when the move was rolled back through `POST /decisions/revert`.
    #[serde(default)]
    pub reverted_at: Option<String>,
    /// What caused this decision: `manual`, `schedule` or `webhook`.
    ///
    /// Null on rows written before the column existed — attributing them after
    /// the fact would be a guess, and a guess here reads as a fact.
    #[serde(default)]
    pub actor: Option<String>,
    /// Who asked, when the mode vouched for a name.
    ///
    /// Null for the scheduler, which nobody asked; for the modes that let
    /// everyone through under one anonymous subject; and for rows written
    /// before the column existed.
    #[serde(default)]
    pub subject: Option<String>,
}

/// An alternative decision that was considered but not selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeDecision {
    pub rule_name: String,
    pub category: String,
    pub reason: String,
    /// Set when this rule matched but an exclusion vetoed it.
    #[serde(default)]
    pub excluded_by: Option<String>,
    #[serde(default)]
    pub confidence: f32,
}

/// Request to apply selected decisions.
#[derive(Debug, Deserialize)]
pub struct ApplyDecisionsRequest {
    pub decision_ids: Vec<String>,
    #[serde(default)]
    pub move_files: bool,
    /// The guardrails the caller has looked at, by name. Three of them ask,
    /// and a boolean here meant answering one answered all three.
    #[serde(default)]
    pub confirm: crate::services::executor::Confirmed,
}

/// Apply everything one simulation proposed, in slices.
#[derive(Debug, Deserialize)]
pub struct ApplyAllRequest {
    pub simulation_id: String,
    #[serde(default)]
    pub move_files: bool,
    /// Always required: this is a mass operation by definition, so the
    /// confirmation threshold has nothing to say about it. Its one question
    /// states both the count and any capacity shortfall, and it is answered
    /// under the single name `batch`.
    #[serde(default)]
    pub confirm: crate::services::executor::Confirmed,
}

/// Request to roll a previously applied decision back to its original folder.
#[derive(Debug, Deserialize)]
pub struct RevertDecisionsRequest {
    pub decision_ids: Vec<String>,
    #[serde(default)]
    pub move_files: bool,
}

/// Request to run a simulation.
#[derive(Debug, Default, Deserialize)]
pub struct SimulationRequest {
    #[serde(default)]
    pub instance_ids: Option<Vec<String>>,
    #[serde(default)]
    pub media_type: Option<String>,
    /// Store the decisions so they can be applied. `false` is a pure preview.
    #[serde(default = "default_true")]
    pub persist: bool,
    /// Also keep decisions for media already in the right place.
    #[serde(default)]
    pub persist_unchanged: bool,
    /// Cap the payload size; the counters always reflect the full library.
    #[serde(default)]
    pub max_returned: Option<usize>,
}

/// Simulation result summary.
#[derive(Debug, Serialize)]
pub struct SimulationResult {
    pub simulation_id: String,
    pub total_media: usize,
    /// How many decisions are included in `decisions` after truncation.
    pub returned: usize,
    pub decisions: Vec<Decision>,
    pub moves_required: usize,
    pub already_correct: usize,
    pub no_category_match: usize,
    pub overrides_applied: usize,
    /// Matched a category with no root folder mapped on that instance.
    pub skipped_unmapped: usize,
    /// Rules that matched but were vetoed by one of their exclusions.
    pub excluded_by_rule: usize,
    /// What the plan would put on each destination, and whether it fits.
    ///
    /// Only destinations that receive something appear, and only when the Arr
    /// reported a free-space figure for them.
    pub capacity: Vec<CapacityForecast>,
    pub elapsed_ms: u64,
}

/// One destination folder, and the weight of what this plan sends to it.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapacityForecast {
    pub instance_id: String,
    pub instance_name: Option<String>,
    pub path: String,
    /// Bytes arriving from a *different* filesystem, which is the only traffic
    /// that consumes space — see `fits`.
    pub incoming_bytes: i64,
    /// Bytes arriving from a folder that reports the same free space, and so
    /// almost certainly sits on the same filesystem. A move there is a rename
    /// and costs nothing; counted separately rather than dropped, because a
    /// figure the user cannot see is a figure they cannot check.
    pub same_filesystem_bytes: i64,
    pub free_bytes: i64,
    pub items: usize,
    /// `incoming_bytes` fits in `free_bytes`.
    pub fits: bool,
}

/// Query parameters for decision listing.
#[derive(Debug, Default, Deserialize)]
pub struct DecisionQuery {
    pub instance_id: Option<String>,
    pub media_type: Option<String>,
    pub status: Option<String>,
    pub category: Option<String>,
    pub action: Option<String>,
    pub simulation_id: Option<String>,
    pub search: Option<String>,
    /// Hide proposals replaced by a newer simulation. Defaults to true.
    pub include_superseded: Option<bool>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

fn default_true() -> bool {
    true
}
