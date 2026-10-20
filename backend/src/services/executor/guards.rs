//! The guardrails an apply and a revert ask before anything is written.

use sqlx::AssertSqlSafe;

use super::load::current_for_run;
use crate::db::Conditions;
use crate::error::{AppError, AppResult};
use crate::models::DecisionFilter;
use crate::state::AppState;

/// The names a guardrail asks under. A caller sends back the name of the refusal it read, and
/// only that one is lifted. Constants, since a typo on either side of the wire is a guardrail
/// nobody can answer.
pub mod confirm {
    /// The destination cannot hold the plan.
    pub const CAPACITY: &str = "capacity";
    /// More decisions than the configured threshold.
    pub const THRESHOLD: &str = "threshold";
    /// A whole simulation, whose one question already states both facts above.
    pub const BATCH: &str = "batch";
    /// The destination is not answering right now.
    pub const UNREACHABLE: &str = "unreachable";

    /// Every name above: what an application key may be allowed to answer, and what
    /// `Confirmed::all()` answers in tests. A name missing here is one no key or test answers.
    pub const ALL: &[&str] = &[CAPACITY, THRESHOLD, BATCH, UNREACHABLE];
}

/// The guardrails the caller has looked at and accepts: `capacity`,
/// `threshold`, `batch` or `unreachable`.
// Names, never a boolean: one flag would let confirming one guardrail wave the others through.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct Confirmed(Vec<String>);

impl Confirmed {
    /// Nothing answered yet, for tests that assert the first question asked.
    #[cfg(test)]
    pub fn none() -> Self {
        Self::default()
    }

    pub fn has(&self, kind: &str) -> bool {
        self.0.iter().any(|answered| answered == kind)
    }

    /// The answers `keep` accepts, the others dropped as if never sent.
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self(self.0.iter().filter(|answered| keep(answered)).cloned().collect())
    }

    /// Every question answered, for tests of what follows the asking. Test-only, since in the
    /// application it would be the blanket flag this type exists to prevent.
    #[cfg(test)]
    pub fn all() -> Self {
        Self(confirm::ALL.iter().map(|kind| (*kind).to_string()).collect())
    }
}

/// Refuse everything while the global dry-run switch is on.
pub(super) async fn guard_dry_run(state: &AppState) -> AppResult<()> {
    if state.bool_setting("global_dry_run").await {
        let localizer = state.localizer().await;
        let message = localizer.translate("ErrorDryRunEnabled", &[]);
        return Err(AppError::Conflict { reason: "dry_run", message });
    }
    Ok(())
}

/// Refuse an empty selection or one above `batch_limit`, before any guard that binds the ids:
/// the limit is capped at a thousand and SQLite binds 32 766 values at most, so a list this
/// passes fits one statement.
pub(super) async fn guard_batch_limit(state: &AppState, count: usize) -> AppResult<()> {
    let localizer = state.localizer().await;

    if count == 0 {
        return Err(AppError::BadRequest(localizer.translate("ErrorNoSelection", &[])));
    }

    let batch_limit: usize = state.bounding_setting::<usize>("batch_limit").await?;
    if count > batch_limit {
        // Named by its label and where it is edited, since nobody can find its storage key.
        return Err(AppError::BadRequest(localizer.translate(
            "ErrorBatchLimit",
            &[
                ("count", &count.to_string()),
                ("limit", &batch_limit.to_string()),
                ("setting", &localizer.translate("SettingBatchLimit", &[])),
                ("settings", &localizer.translate("Settings", &[])),
                ("tab", &localizer.translate("SettingsTabGuardrails", &[])),
            ],
        )));
    }
    Ok(())
}

/// Ask a person to look before a batch above the threshold moves anything.
pub(super) async fn guard_confirmation(
    state: &AppState,
    count: usize,
    confirmed: &Confirmed,
) -> AppResult<()> {
    let localizer = state.localizer().await;
    let threshold: usize = state.bounding_setting::<usize>("confirmation_threshold").await?;
    if count > threshold && !confirmed.has(confirm::THRESHOLD) {
        return Err(AppError::ConfirmationRequired {
            includes: Vec::new(),
            kind: confirm::THRESHOLD,
            message: localizer.translate(
                "ErrorConfirmationRequired",
                &[("count", &count.to_string()), ("threshold", &threshold.to_string())],
            ),
        });
    }

    Ok(())
}

/// Ask before writing into a destination the Arr reports unreachable. A sleeping NAS stays in
/// the routing map, unknown rather than gone, and cannot be told from a dead disk, so the
/// question is asked here, at the one moment it can be answered.
pub(super) async fn guard_reachable(
    state: &AppState,
    scope: CapacityScope<'_>,
    confirmed: &Confirmed,
) -> AppResult<()> {
    if confirmed.has(confirm::UNREACHABLE) {
        return Ok(());
    }
    let Some(Weighed { rows, binds, to, .. }) = scope.weighed() else {
        return Ok(());
    };

    // A folder stored as asleep may have woken since the last sync: its instance is asked again.
    let sleeping = format!(
        "SELECT DISTINCT tgt.instance_id
         FROM decisions d
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND tgt.path = {to} COLLATE path
         WHERE {rows}
           AND tgt.accessible = 0"
    );
    let mut query = sqlx::query_scalar::<_, String>(AssertSqlSafe(sleeping.as_str()));
    for bind in &binds {
        query = query.bind(bind);
    }
    for instance_id in query.fetch_all(&state.pool).await? {
        if let Err(e) = crate::services::sync::refresh_root_folders(state, &instance_id).await {
            tracing::debug!(instance = %instance_id, "The root folders could not be read again: {e}");
        }
    }

    let sql = format!(
        "SELECT DISTINCT tgt.path, tgt.last_accessible_at
         FROM decisions d
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND tgt.path = {to} COLLATE path
         WHERE {rows}
           AND tgt.accessible = 0
         ORDER BY tgt.path
         LIMIT 1"
    );
    let mut query = sqlx::query_as::<_, (String, Option<String>)>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        query = query.bind(bind);
    }

    if let Some((path, last_seen)) = query.fetch_optional(&state.pool).await? {
        let localizer = state.localizer().await;
        // An age rather than a verdict: only the operator knows if it reads as a nap or a fault.
        let since = last_seen.as_deref().and_then(crate::services::routing::parse_timestamp);
        let message = match since {
            Some(then) => localizer.translate(
                "ErrorTargetUnreachable",
                &[("path", &path), ("age", &crate::localization::human_age(then, &localizer))],
            ),
            None => localizer.translate("ErrorTargetNeverReached", &[("path", &path)]),
        };
        return Err(AppError::ConfirmationRequired {
            includes: Vec::new(),
            kind: confirm::UNREACHABLE,
            message,
        });
    }
    Ok(())
}

/// What a guard weighs: a person's selection, a run or what a filter keeps of it, or moves
/// undone, listed or matched.
#[derive(Clone, Copy)]
pub(super) enum CapacityScope<'a> {
    Decisions(&'a [String]),
    Simulation(&'a str, &'a DecisionFilter),
    Reverting(&'a [String]),
    RevertingMatching(&'a Conditions),
}

/// The moves a guard weighs, as SQL over `decisions d`.
pub(super) struct Weighed {
    /// Which decisions, narrowed to the moves still to write.
    rows: String,
    /// The values `rows` binds, in order.
    binds: Vec<String>,
    /// The folder each move writes into.
    to: &'static str,
    /// The folder each move leaves.
    from: &'static str,
}

/// The proposals an apply writes.
pub(super) const PROPOSED: &str = "d.status = 'pending' AND d.superseded = 0 AND d.action = 'move'";

/// The moves a revert may undo: applied and not undone, the title's latest (an older one skips
/// those between), the title still where it was put (not in a folder chosen since), and its
/// origin still a root folder, whose row the guards weigh. The Revert button reads it too.
pub(crate) const REVERTIBLE: &str =
    "d.status = 'applied' AND d.reverted_at IS NULL AND d.current_root_folder IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM decisions later
                      WHERE later.media_id = d.media_id AND later.id <> d.id
                        AND later.status = 'applied' AND later.reverted_at IS NULL
                        AND later.applied_at > d.applied_at)
     AND EXISTS (SELECT 1 FROM media here
                  WHERE here.id = d.media_id
                    AND here.current_root_folder = d.target_root_folder COLLATE path)
     AND EXISTS (SELECT 1 FROM root_folders back
                  WHERE back.instance_id = d.instance_id
                    AND back.path = d.current_root_folder COLLATE path)";

impl CapacityScope<'_> {
    /// `None` for an empty selection, which there is nothing to weigh in.
    fn weighed(self) -> Option<Weighed> {
        let listed = |ids: &[String]| format!("d.id IN ({})", crate::db::placeholders(ids.len()));
        let forward = |rows: String, binds: Vec<String>| Weighed {
            rows: format!("{rows} AND {PROPOSED}"),
            binds,
            to: "d.target_root_folder",
            from: "d.current_root_folder",
        };
        let back = |rows: String, binds: Vec<String>| Weighed {
            rows,
            binds,
            to: "d.current_root_folder",
            from: "d.target_root_folder",
        };
        match self {
            CapacityScope::Decisions([]) | CapacityScope::Reverting([]) => None,
            CapacityScope::Decisions(ids) => Some(forward(listed(ids), ids.to_vec())),
            // By id and filter: a library-sized run has more decisions than a statement binds.
            CapacityScope::Simulation(id, filter) => {
                let (current, binds) = current_for_run(id, filter);
                Some(forward(format!("d.id IN ({current})"), binds))
            }
            CapacityScope::Reverting(ids) => {
                Some(back(format!("{} AND {REVERTIBLE}", listed(ids)), ids.to_vec()))
            }
            CapacityScope::RevertingMatching(matching) => {
                Some(back(format!("{REVERTIBLE}{}", matching.sql()), matching.binds().to_vec()))
            }
        }
    }
}

/// Ask before a plan a destination cannot hold: the Arr fails it partway and records the new
/// paths anyway. Folders with the same free space, as the last sync stored it, count as one
/// volume, where a move is a rename. That is evidence, not proof, hence a question.
pub(super) async fn guard_capacity(
    state: &AppState,
    scope: CapacityScope<'_>,
    move_files: bool,
    confirmed: &Confirmed,
) -> AppResult<()> {
    // No byte moves with the files in place, and a question with no stake teaches a blind yes.
    if !move_files || confirmed.has(confirm::CAPACITY) {
        return Ok(());
    }
    let Some(Weighed { rows, binds, to, from }) = scope.weighed() else {
        return Ok(());
    };
    let sql = format!(
        "SELECT d.instance_id, {to},
                SUM(CASE WHEN tgt.free_space IS NOT NULL AND tgt.free_space = src.free_space
                         THEN 0 ELSE COALESCE(m.size_on_disk, 0) END),
                MAX(tgt.free_space),
                MAX(CASE WHEN tgt.origin = 'declared' THEN 1 ELSE 0 END)
         FROM decisions d
         JOIN media m ON m.id = d.media_id
         JOIN root_folders tgt
              ON tgt.instance_id = d.instance_id
             AND tgt.path = {to} COLLATE path
         LEFT JOIN root_folders src
              ON src.instance_id = d.instance_id
             AND src.path = {from} COLLATE path
         WHERE {rows}
         GROUP BY d.instance_id, {to}"
    );
    let mut query =
        sqlx::query_as::<_, (String, String, i64, Option<i64>, i64)>(AssertSqlSafe(sql.as_str()));
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(&state.pool).await?;
    // Folders of one instance with the same free space are one volume: together, they must fit.
    let mut volumes: Vec<((&str, i64), Vec<&str>, i64)> = Vec::new();
    for (instance, path, incoming, free, _) in &rows {
        let Some(free) = *free else { continue };
        match volumes.iter_mut().find(|(volume, _, _)| *volume == (instance.as_str(), free)) {
            Some((_, paths, total)) => {
                paths.push(path);
                *total += incoming;
            }
            None => volumes.push(((instance, free), vec![path], *incoming)),
        }
    }
    for &(_, ref path, incoming, free, declared) in &rows {
        let Some(free) = free else {
            // Only a declared folder is asked about a missing figure: no Arr watches its disk.
            if declared == 1 && incoming > 0 {
                let localizer = state.localizer().await;
                return Err(AppError::ConfirmationRequired {
                    includes: Vec::new(),
                    kind: confirm::CAPACITY,
                    message: localizer.translate(
                        "ErrorCapacityUnknown",
                        &[
                            ("path", path),
                            ("needed", &crate::localization::human_bytes(incoming, &localizer)),
                        ],
                    ),
                });
            }
            continue;
        };
        if incoming > free {
            let localizer = state.localizer().await;
            return Err(AppError::ConfirmationRequired {
                includes: Vec::new(),
                kind: confirm::CAPACITY,
                message: localizer.translate(
                    "ErrorNotEnoughSpace",
                    &[
                        ("path", path),
                        ("needed", &crate::localization::human_bytes(incoming, &localizer)),
                        ("free", &crate::localization::human_bytes(free, &localizer)),
                    ],
                ),
            });
        }
    }
    // Each folder fits alone by now, so a volume still short is short for several together.
    if let Some(((_, free), paths, needed)) =
        volumes.into_iter().find(|((_, free), _, needed)| needed > free)
    {
        let localizer = state.localizer().await;
        let paths = paths.join(&localizer.translate("ListSeparator", &[]));
        return Err(AppError::ConfirmationRequired {
            includes: Vec::new(),
            kind: confirm::CAPACITY,
            message: localizer.translate(
                "ErrorNotEnoughSpaceShared",
                &[
                    ("paths", &paths),
                    ("needed", &crate::localization::human_bytes(needed, &localizer)),
                    ("free", &crate::localization::human_bytes(free, &localizer)),
                ],
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::localization::Localizer;
    use crate::localization::human_bytes;

    /// The server names a size as the interface's `formatBytes` does, since a capacity refusal
    /// and the root-folders table show one figure. `byte_cases.json` holds both sides to the
    /// same names, spaces read as plain ones.
    #[test]
    fn a_size_is_named_as_the_interface_names_it() {
        let cases: Vec<(i64, String, String)> =
            serde_json::from_str(include_str!("../../byte_cases.json")).unwrap();
        assert!(cases.len() > 10, "{cases:?}");
        let parted: Vec<String> = cases
            .into_iter()
            .filter_map(|(bytes, language, named)| {
                let said = human_bytes(bytes, &Localizer::new(&language))
                    .replace(['\u{202f}', '\u{a0}'], " ");
                (said != named)
                    .then(|| format!("{bytes} bytes in {language}: {said:?}, not {named:?}"))
            })
            .collect();
        assert!(parted.is_empty(), "the server names sizes otherwise:\n{}", parted.join("\n"));
    }

    /// Every guardrail name reaches `confirm::ALL`, read out of this file: one forgotten in the
    /// slice refuses a caller that answered every question, and nothing else says so.
    #[test]
    fn every_guardrail_name_is_in_the_list() {
        const SOURCE: &str = include_str!("guards.rs");
        let module = SOURCE
            .split_once("pub mod confirm {")
            .expect("the confirm module")
            .1
            .split_once("\n}")
            .expect("its closing brace")
            .0;

        let names: Vec<&str> = module
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub const "))
            .filter_map(|rest| rest.split_once(':'))
            .map(|(name, _)| name.trim())
            .filter(|name| *name != "ALL")
            .collect();

        // The guard on the parser: matching nothing, every assertion below would pass.
        assert!(names.len() >= 4, "only {} name(s) parsed: {names:?}", names.len());

        // Compared whole: `REACH` inside `UNREACHABLE` is not listed.
        let listed: Vec<&str> = module
            .split_once("pub const ALL: &[&str] = &[")
            .expect("the list")
            .1
            .split_once(']')
            .expect("its bracket")
            .0
            .split(',')
            .map(str::trim)
            .collect();
        for name in names {
            assert!(listed.contains(&name), "{name} is a guardrail nobody can answer");
        }
    }
}
