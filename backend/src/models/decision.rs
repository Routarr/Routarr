use serde::{Deserialize, Serialize};

/// What a decision does to its title.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum DecisionAction {
    /// Move it to the folder of its category.
    Move,
    /// Leave it, already there.
    None,
    /// Leave it: its category has no folder on its instance.
    Skip,
}

impl DecisionAction {
    /// The word the API and the database write.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionAction::Move => "move",
            DecisionAction::None => "none",
            DecisionAction::Skip => "skip",
        }
    }
}

/// Where a decision stands.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum DecisionStatus {
    /// Proposed, waiting to be applied.
    Pending,
    /// Asked of the Arr, its move not seen through yet.
    Requested,
    Applied,
    Failed,
    /// Not applied, or rolled back: a reverted move carries `reverted_at`.
    Skipped,
}

/// A routing decision computed by the rule engine.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Decision {
    pub id: String,
    pub media_id: String,
    pub media_title: String,
    pub media_type: String,
    pub instance_id: String,
    pub instance_name: Option<String>,
    pub current_root_folder: Option<String>,
    /// The category the current root folder leads to on the title's instance, null when it
    /// leads to none.
    #[serde(default)]
    pub current_category: Option<String>,
    pub target_root_folder: Option<String>,
    pub target_category: String,
    pub matched_rule_id: Option<String>,
    pub matched_rule_name: Option<String>,
    pub is_override: bool,
    /// Why the title goes where it goes, one line each, in the reader's
    /// language: the texts of `reason_lines`.
    pub reasons: Vec<String>,
    /// The same lines, each with whether it states what decided.
    pub reason_lines: Vec<ReasonLine>,
    pub alternatives: Vec<AlternativeDecision>,
    // An extensible string: a retype breaks clients (`scripts/check-api-breaks.sh`).
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["move", "none", "skip"]))))]
    pub action: DecisionAction,
    #[schema(
        value_type = String,
        extensions(("x-extensible-enum" = json!(["pending", "requested", "applied", "failed", "skipped"])))
    )]
    pub status: DecisionStatus,
    /// 0.0 to 1.0, how many independent signals backed the winning rule.
    #[serde(default)]
    pub confidence: f32,
    /// True once this decision was retired: by a newer simulation, an override
    /// set or removed, a revalidation that found it stale, or the removal of
    /// its media or its instance.
    #[serde(default)]
    pub superseded: bool,
    /// Groups every decision produced by the same simulation run.
    #[serde(default)]
    pub simulation_id: Option<String>,
    pub error_message: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub decided_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub applied_at: Option<String>,
    /// Set when the move was rolled back through `POST /decisions/revert`.
    #[serde(default)]
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub reverted_at: Option<String>,
    /// What set the decision off: `manual`, `schedule`, `webhook`, `api`, or
    /// `auto` for the simulation the automation runs after a sync.
    // Every writer sets it, and a guess at a missing one would read as a fact.
    #[serde(default)]
    pub actor: Option<String>,
    /// Who asked: an application's name, or the person a sign-in mode names.
    /// Null when nobody asked (the scheduler or a webhook), and under a mode
    /// that names nobody. An application key reads its own name and null for
    /// anyone else.
    #[serde(default)]
    pub subject: Option<String>,
    /// The application key that asked, which is what makes `subject` its own.
    #[serde(skip)]
    pub subject_key: Option<String>,
    /// Whether a revert may undo this move now: only the latest standing move
    /// of a title is, while the title is still where it put it and the folder
    /// it came from is still one of its instance's root folders.
    // `executor::REVERTIBLE` decides it.
    #[serde(default)]
    pub revertible: bool,
    // As a run stores them, said in the reader's language (`routing::stored_reasons`).
    #[serde(skip)]
    pub stored_reasons: String,
    #[serde(skip)]
    pub stored_alternatives: String,
}

/// One line of a decision's justification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ReasonLine {
    /// The line, in the reader's language.
    pub text: String,
    /// True for a condition the title meets and for the exception that
    /// decides it, false for a condition it misses, a veto and the fallback.
    /// Null for a line a run stored as a sentence.
    pub matched: Option<bool>,
}

/// An alternative decision that was considered but not selected.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

/// Request to run a simulation.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
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
    /// Cap the payload size. The counters always reflect the full library.
    #[serde(default)]
    pub max_returned: Option<usize>,
}

/// Simulation result summary.
#[derive(Debug, Serialize, utoipa::ToSchema)]
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
    /// How often a rule matched a title and one of its own exclusions set it
    /// aside, over every title, rules below the winner included.
    pub excluded_by_rule: usize,
    /// What the plan would put on each destination, and whether it fits.
    ///
    /// Only destinations that receive something appear, and only when the Arr
    /// reported a free-space figure for them.
    pub capacity: Vec<CapacityForecast>,
    pub elapsed_ms: u64,
}

/// One destination folder, and the weight of what this plan sends to it.
#[derive(Debug, Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct CapacityForecast {
    pub instance_id: String,
    pub instance_name: Option<String>,
    pub path: String,
    /// Bytes arriving from a *different* filesystem, which is the only traffic
    /// that consumes space (see `fits`).
    pub incoming_bytes: i64,
    /// Bytes arriving from a folder that reports the same free space, and so
    /// almost certainly sits on the same filesystem. A move there is a rename
    /// and costs nothing.
    // Counted rather than dropped, since a figure the user cannot see cannot be checked.
    pub same_filesystem_bytes: i64,
    pub free_bytes: i64,
    pub items: usize,
    /// `incoming_bytes` fits in `free_bytes`.
    pub fits: bool,
}

/// Which titles an apply or a revert covers, as the decisions list filters them. Each field
/// set narrows the set, and none set leaves it whole.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionFilter {
    /// Only the decisions about this instance's titles.
    pub instance_id: Option<String>,
    /// `movie` or `series`.
    pub media_type: Option<String>,
    /// Only the decisions sending a title to this category.
    pub category: Option<String>,
    /// Only the titles in this root folder when the decision was made.
    pub current_root_folder: Option<String>,
    /// Only the decisions this rule made.
    pub rule_id: Option<String>,
    /// Part of the title, ASCII letters in any case.
    pub search: Option<String>,
}

impl DecisionFilter {
    /// Narrow `conditions`, written on a `decisions` alias, to the titles this filter keeps.
    pub fn narrow(&self, conditions: &mut crate::db::Conditions) {
        conditions
            .equal("instance_id", self.instance_id.as_deref())
            .equal("media_type", self.media_type.as_deref())
            .equal("target_category", self.category.as_deref())
            .same_path("current_root_folder", self.current_root_folder.as_deref())
            .equal("matched_rule_id", self.rule_id.as_deref())
            .contains("media_title", self.search.as_deref());
    }
}

/// Query parameters for decision listing.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct DecisionQuery {
    /// Only the decisions about this instance's titles.
    pub instance_id: Option<String>,
    /// `movie` or `series`.
    pub media_type: Option<String>,
    /// `pending`, `applied`, `failed` or `skipped`.
    pub status: Option<String>,
    /// Only the decisions sending a title to this category.
    pub category: Option<String>,
    /// Only the titles in this root folder when the decision was made.
    pub current_root_folder: Option<String>,
    /// Only the decisions this rule made.
    pub rule_id: Option<String>,
    /// `move`, `none` or `skip`.
    pub action: Option<String>,
    /// Only what this simulation proposed.
    pub simulation_id: Option<String>,
    /// Part of the title, ASCII letters in any case.
    pub search: Option<String>,
    /// Only the decisions made at this time or later, in RFC 3339.
    #[param(format = DateTime)]
    pub since: Option<String>,
    /// Only the decisions made before this time, in RFC 3339.
    #[param(format = DateTime)]
    pub until: Option<String>,
    /// Only the applied moves a revert may still undo, or with `false` every other decision.
    pub revertible: Option<bool>,
    /// Include the superseded decisions. Defaults to false.
    pub include_superseded: Option<bool>,
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
}

impl DecisionQuery {
    /// The titles this query lists, as an apply or a revert would take them.
    pub fn filter(&self) -> DecisionFilter {
        DecisionFilter {
            instance_id: self.instance_id.clone(),
            media_type: self.media_type.clone(),
            category: self.category.clone(),
            current_root_folder: self.current_root_folder.clone(),
            rule_id: self.rule_id.clone(),
            search: self.search.clone(),
        }
    }
}

fn default_true() -> bool {
    true
}
