//! What a filter keeps of a run, and how its question names the filter.

use crate::db::Conditions;
use crate::error::AppResult;
use crate::localization::{Localizer, human_age};
use crate::models::DecisionFilter;
use crate::services::routing;
use crate::state::AppState;

/// Which applied moves a revert in slices undoes. The times are stored ones, as
/// `routing::format_timestamp` writes them.
#[derive(Debug, Default, Clone)]
pub struct RevertScope {
    pub filter: DecisionFilter,
    pub simulation_id: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

impl RevertScope {
    /// The scope as conditions on `decisions d`.
    pub(super) fn conditions(&self) -> Conditions {
        let mut conditions = Conditions::on("d");
        self.filter.narrow(&mut conditions);
        conditions
            .equal("simulation_id", self.simulation_id.as_deref())
            .from("decided_at", self.since.as_deref())
            .before("decided_at", self.until.as_deref());
        conditions
    }

    /// The scope as the question names it.
    pub(super) async fn terms(
        &self,
        state: &AppState,
        localizer: &Localizer,
    ) -> AppResult<Vec<String>> {
        let mut extra = Vec::new();
        if let Some(simulation_id) = &self.simulation_id {
            let run: Option<String> =
                sqlx::query_scalar("SELECT MIN(decided_at) FROM decisions WHERE simulation_id = ?")
                    .bind(simulation_id)
                    .fetch_one(&state.pool)
                    .await?;
            let when = run.as_deref().and_then(routing::parse_timestamp);
            extra.push(term(
                localizer,
                "Simulation",
                &when.map_or_else(|| simulation_id.clone(), |when| ago(when, localizer)),
            ));
        }
        if let Some(since) = self.since.as_deref().and_then(routing::parse_timestamp) {
            extra.push(term(localizer, "DecidedFrom", &ago(since, localizer)));
        }
        // A bound still to come narrows nothing, and its age would read as a past one.
        if let Some(until) = self
            .until
            .as_deref()
            .and_then(routing::parse_timestamp)
            .filter(|until| *until <= chrono::Utc::now())
        {
            extra.push(term(localizer, "DecidedBefore", &ago(until, localizer)));
        }
        terms(state, &self.filter, &extra, localizer).await
    }
}

/// One term per field `filter` sets, named as a reader knows it, then `extra`. Empty when
/// nothing narrows the run.
pub(super) async fn terms(
    state: &AppState,
    filter: &DecisionFilter,
    extra: &[String],
    localizer: &Localizer,
) -> AppResult<Vec<String>> {
    let mut terms = Vec::new();
    if let Some(id) = &filter.instance_id {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM instances WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
        terms.push(term(localizer, "Instance", name.as_deref().unwrap_or(id)));
    }
    if let Some(kind) = &filter.media_type {
        let named = match kind.as_str() {
            "movie" => localizer.translate("Movies", &[]),
            "series" => localizer.translate("Series", &[]),
            other => other.to_string(),
        };
        terms.push(term(localizer, "Type", &named));
    }
    if let Some(category) = &filter.category {
        terms.push(term(localizer, "Category", category));
    }
    if let Some(folder) = &filter.current_root_folder {
        terms.push(term(localizer, "CurrentRootFolder", folder));
    }
    if let Some(id) = &filter.rule_id {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM rules WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
        terms.push(term(localizer, "Rule", name.as_deref().unwrap_or(id)));
    }
    if let Some(search) = &filter.search {
        terms.push(term(localizer, "TitleContains", search));
    }
    terms.extend_from_slice(extra);
    Ok(terms)
}

fn term(localizer: &Localizer, label: &str, value: &str) -> String {
    localizer
        .translate("FilterTerm", &[("label", &localizer.translate(label, &[])), ("value", value)])
}

fn ago(then: chrono::DateTime<chrono::Utc>, localizer: &Localizer) -> String {
    localizer.translate("AgeAgo", &[("age", &human_age(then, localizer))])
}
