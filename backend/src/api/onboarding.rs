//! The getting-started guide: where a new installation stands, step by step.
//!
//! Each step is read from the data itself, never from a record of what the
//! guide showed. Leaving halfway and coming back resumes where the installation
//! is, a step done by hand outside the guide counts, and deleting the only
//! instance puts the first step back. The one thing stored is whether the guide
//! is still wanted: the `onboarding` setting, pending when absent.

use super::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::health::UNMAPPED;
use crate::error::{AppError, AppResult};
use crate::services::settings::ONBOARDING_STATES;
use crate::state::AppState;

/// Each step's id. A warning that restates a step names it by the same id
/// (`health::Warning`).
pub mod step {
    pub const INSTANCE: &str = "instance";
    pub const CATEGORIES: &str = "categories";
    pub const METADATA: &str = "metadata";
    pub const RULE: &str = "rule";
    pub const SIMULATION: &str = "simulation";
    pub const LIVE: &str = "live";
}

#[derive(Debug, Serialize)]
pub struct OnboardingStatus {
    /// `pending` (the guide shows), `dismissed` (skipped, can be resumed) or
    /// `done` (finished, or an installation set up without it).
    pub state: String,
    /// Whether every step that is not optional is done.
    pub complete: bool,
    /// In the order the guide walks them.
    pub steps: Vec<OnboardingStep>,
}

#[derive(Debug, Serialize)]
pub struct OnboardingStep {
    pub id: &'static str,
    pub done: bool,
    /// An optional step never holds the guide back from completing.
    pub optional: bool,
}

#[derive(Debug, Deserialize)]
pub struct OnboardingUpdate {
    pub state: String,
}

pub async fn get(State(state): State<AppState>) -> AppResult<Json<OnboardingStatus>> {
    Ok(Json(status(&state).await?))
}

pub async fn update(
    State(state): State<AppState>,
    Json(req): Json<OnboardingUpdate>,
) -> AppResult<Json<OnboardingStatus>> {
    let wanted = req.state.trim();
    if !ONBOARDING_STATES.contains(&wanted) {
        return Err(AppError::BadRequest(format!(
            "'state' must be one of {}",
            ONBOARDING_STATES.join(", ")
        )));
    }
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('onboarding', ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(wanted)
    .execute(&state.pool)
    .await?;
    Ok(Json(status(&state).await?))
}

/// Every step, read in one statement plus the settings the two optional ones
/// depend on.
async fn status(state: &AppState) -> AppResult<OnboardingStatus> {
    // A synced instance, not only a saved one: the next steps need its root
    // folders, which arrive with the first sync. A category mapped on a
    // disabled instance routes nothing. The categories step is done once one
    // folder is mapped and no category reaches no folder, the same predicate as
    // the diagnostics warning, so the guide and the warning never disagree.
    // A simulation counts once someone ran one, read from the job the run
    // records: the scheduler's own pass after each sync supersedes the
    // proposals of a manual one, the purge then deletes them, and a library
    // already in place gets none. The job row lasts `log_retention_days`.
    let (instance, mapped, unmapped, rule, simulation): (bool, bool, bool, bool, bool) =
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT
                EXISTS(SELECT 1 FROM instances WHERE enabled = 1 AND last_sync_at IS NOT NULL),
                EXISTS(SELECT 1 FROM root_folders rf JOIN instances i ON i.id = rf.instance_id
                       WHERE i.enabled = 1 AND rf.category IS NOT NULL AND rf.category <> ''),
                EXISTS(SELECT 1 FROM categories c WHERE {UNMAPPED}),
                EXISTS(SELECT 1 FROM rules WHERE enabled = 1),
                EXISTS(SELECT 1 FROM jobs
                       WHERE kind = ? AND trigger = ? AND status = 'success')"
        )))
        .bind(crate::jobs::JobKind::Simulate.as_str())
        .bind(crate::jobs::TRIGGER_MANUAL)
        .fetch_one(&state.pool)
        .await?;

    // A source beside the Arr's own data that can answer today, which is what
    // makes keyword and country conditions usable.
    let metadata = state.metadata_providers().await.iter().any(|source| source.id != "arr");
    let live = !state.bool_setting("global_dry_run").await;

    let steps = vec![
        OnboardingStep { id: step::INSTANCE, done: instance, optional: false },
        OnboardingStep { id: step::CATEGORIES, done: mapped && !unmapped, optional: false },
        OnboardingStep { id: step::METADATA, done: metadata, optional: true },
        OnboardingStep { id: step::RULE, done: rule, optional: false },
        OnboardingStep { id: step::SIMULATION, done: simulation, optional: false },
        OnboardingStep { id: step::LIVE, done: live, optional: true },
    ];
    let complete = steps.iter().all(|step| step.done || step.optional);

    let stored: String = state.setting::<String>("onboarding").await;
    let current =
        if ONBOARDING_STATES.contains(&stored.as_str()) { stored } else { "pending".to_string() };

    Ok(OnboardingStatus { state: current, complete, steps })
}
