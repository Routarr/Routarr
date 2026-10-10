//! The guardrails an apply and a revert ask before anything is written.

use sqlx::AssertSqlSafe;

use super::load::CURRENT_FOR_RUN;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The names a guardrail asks under.
///
/// A caller that has looked at one refusal sends its name back, and only that
/// one is lifted. Written as constants rather than as literals at each site
/// because the string crosses the wire and comes back: a typo on one side is a
/// guardrail that can never be answered.
pub mod confirm {
    /// The destination cannot hold the plan.
    pub const CAPACITY: &str = "capacity";
    /// More decisions than the configured threshold.
    pub const THRESHOLD: &str = "threshold";
    /// A whole simulation, whose one question already states both facts above.
    pub const BATCH: &str = "batch";
    /// The destination is not answering right now.
    pub const UNREACHABLE: &str = "unreachable";

    /// Every name above: what an application key may be allowed to answer,
    /// and what `Confirmed::all()` builds from in tests. Written once, because
    /// a guardrail added to the module and forgotten in a second list is one
    /// no key can ever be given, and one the tests never answer.
    pub const ALL: &[&str] = &[CAPACITY, THRESHOLD, BATCH, UNREACHABLE];
}

/// The guardrails the caller has looked at and accepts: `capacity`,
/// `threshold`, `batch` or `unreachable`.
// A list of names, never a boolean: with one flag read by every guardrail
// that asks, answering one question answers them all, and confirming a
// capacity shortfall silently waves the batch threshold through as well.
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

    /// Every question answered, for tests, which assert what happens *after*
    /// the asking. Not available to the application, where it would be the
    /// blanket flag this type exists to prevent.
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

/// Enforce the batch ceiling: nothing, and never more than `batch_limit`.
///
/// A count, so it runs before any guard that spells the ids out in a query:
/// `batch_limit` is capped at a thousand, and SQLite binds 32 766 parameters
/// at most, so a list this has passed always fits one statement.
pub(super) async fn guard_batch_limit(state: &AppState, count: usize) -> AppResult<()> {
    let localizer = state.localizer().await;

    if count == 0 {
        return Err(AppError::BadRequest(localizer.translate("ErrorNoSelection", &[])));
    }

    let batch_limit: usize = state.bounding_setting::<usize>("batch_limit").await?;
    if count > batch_limit {
        // The setting by its label and where it is edited: its storage key
        // names no field anybody can find.
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

/// Refuse to write into a destination the Arr says it cannot reach.
///
/// A root folder on a NAS that spins down is reported inaccessible, and the
/// routing map deliberately keeps it: unknown is not gone, and a plan built
/// while the disk was awake must survive the nap. The question is asked here
/// instead, at the one moment it can be answered. It is asked rather than
/// refused, because a NAS that wakes on access cannot be told from a dead disk,
/// and refusing outright would make the product unusable for those
/// installations.
pub(super) async fn guard_reachable(
    state: &AppState,
    scope: CapacityScope<'_>,
    confirmed: &Confirmed,
) -> AppResult<()> {
    if confirmed.has(confirm::UNREACHABLE) {
        return Ok(());
    }
    let Some(Weighed { rows, to, .. }) = scope.weighed() else {
        return Ok(());
    };

    // A folder stored as not answering may have woken since a scheduled sync
    // last listed it, once a day: its instance is asked again before the
    // question is.
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
    match scope {
        CapacityScope::Decisions(ids) | CapacityScope::Reverting(ids) => {
            for id in ids {
                query = query.bind(id);
            }
        }
        CapacityScope::Simulation(id) => query = query.bind(id),
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
    match scope {
        CapacityScope::Decisions(ids) | CapacityScope::Reverting(ids) => {
            for id in ids {
                query = query.bind(id);
            }
        }
        CapacityScope::Simulation(id) => query = query.bind(id),
    }

    if let Some((path, last_seen)) = query.fetch_optional(&state.pool).await? {
        let localizer = state.localizer().await;
        // How long rather than a verdict: twenty minutes reads as a nap and
        // three days as a fault, and the operator is the one who knows their
        // hardware.
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

/// What a guard weighs: a person's selection, a whole run, or moves undone.
#[derive(Clone, Copy)]
pub(super) enum CapacityScope<'a> {
    Decisions(&'a [String]),
    Simulation(&'a str),
    Reverting(&'a [String]),
}

/// The moves a guard weighs, as SQL over `decisions d`.
pub(super) struct Weighed {
    /// Which decisions, narrowed to the moves still to write.
    rows: String,
    /// The folder each move writes into.
    to: &'static str,
    /// The folder each move leaves.
    from: &'static str,
}

/// The proposals an apply writes.
pub(super) const PROPOSED: &str = "d.status = 'pending' AND d.superseded = 0 AND d.action = 'move'";

/// The moves a revert may undo: applied, not undone yet, knowing where they
/// came from, and the latest of their title's standing moves. Undoing an
/// older one would send the title back to its first folder and skip the ones
/// between. The title must still be where the move put it, or the revert
/// pulls it out of a folder somebody chose since. The folder it goes back to
/// must still be one of its instance's, since the guards weigh a destination
/// through its `root_folders` row and ask nothing about a folder without one.
/// The decisions list reads it too, to draw a Revert button on exactly the
/// rows it lets through.
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
        let forward = |rows: String| Weighed {
            rows: format!("{rows} AND {PROPOSED}"),
            to: "d.target_root_folder",
            from: "d.current_root_folder",
        };
        match self {
            CapacityScope::Decisions([]) | CapacityScope::Reverting([]) => None,
            CapacityScope::Decisions(ids) => Some(forward(listed(ids))),
            // A whole run is selected by its id rather than by listing its
            // decisions: a library-sized run has more of them than one
            // statement can bind.
            CapacityScope::Simulation(_) => Some(forward(format!("d.id IN ({CURRENT_FOR_RUN})"))),
            // Back to the folder each move came from.
            CapacityScope::Reverting(ids) => Some(Weighed {
                rows: format!("{} AND {REVERTIBLE}", listed(ids)),
                to: "d.current_root_folder",
                from: "d.target_root_folder",
            }),
        }
    }
}

/// Refuse a plan a destination cannot hold.
///
/// A batch that overruns its destination fails partway through at the Arr and
/// leaves the library half-moved, and hard to notice, since the Arr records
/// the new path whether or not the file arrived.
///
/// Only bytes that cross a filesystem count: two folders reporting the same
/// free space are almost certainly one volume, where a move is a rename. Two
/// *different* volumes that happen to report the same figure (two full disks,
/// say) read as one and are waved through. The same-volume test is evidence,
/// not proof, which is what the confirmation below is for.
///
/// The figure is `root_folders.free_space` as the last sync stored it, not as
/// the disk stands now. A download since then makes it optimistic, so the guard
/// bounds a plan against a recent past rather than the present.
/// Joined through the path collation because the source path comes from the
/// Arr's payload and the destination from our table, each closed or not by a
/// separator, and on Windows in either case.
///
/// `ConfirmationRequired`, not a refusal: the same-volume test is evidence
/// rather than proof, so being wrong costs one click.
pub(super) async fn guard_capacity(
    state: &AppState,
    scope: CapacityScope<'_>,
    move_files: bool,
    confirmed: &Confirmed,
) -> AppResult<()> {
    // With the files left where they are no byte moves, and a question with
    // no stake teaches people to answer yes to the ones that have one.
    if !move_files || confirmed.has(confirm::CAPACITY) {
        return Ok(());
    }
    let Some(Weighed { rows, to, from }) = scope.weighed() else {
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
    match scope {
        CapacityScope::Decisions(ids) | CapacityScope::Reverting(ids) => {
            for id in ids {
                query = query.bind(id);
            }
        }
        CapacityScope::Simulation(id) => query = query.bind(id),
    }

    let rows = query.fetch_all(&state.pool).await?;
    // Folders of one instance reporting the same free space are one volume,
    // on the evidence the same-filesystem test above reads: what they receive
    // together has to fit.
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
            // Nothing to weigh. A folder an Arr reports has simply not
            // published a figure, and asking about that on every apply would
            // make the question meaningless. A *declared* one with no figure is
            // a different case: it sits under no known root, so nothing
            // upstream will catch a full disk either, and silence there is the
            // guard failing quietly rather than passing.
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
    if let Some(((_, free), paths, needed)) =
        volumes.into_iter().find(|((_, free), paths, needed)| paths.len() > 1 && needed > free)
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

    /// The two halves of the application name a size the same way.
    ///
    /// The interface's `formatBytes` prints the reader's own symbols (`2.2 TB`
    /// in English, `2,2 To` in French) off a division by 1024. A capacity
    /// refusal and the root-folders table name the same figure, so a label that
    /// differs describes one figure in two vocabularies. The unit comes from
    /// the dictionary, and `frontend/src/api/format.test.ts` pins the other
    /// side of the same claim.
    #[test]
    fn a_size_is_named_as_the_interface_names_it() {
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("en")), "2.2 TB");
        assert_eq!(human_bytes(2_400_000_000_000, &Localizer::new("fr")), "2,2 To");
        assert_eq!(human_bytes(5_368_709_120, &Localizer::new("ru")), "5,0 ГБ");
        assert_eq!(human_bytes(512, &Localizer::new("fr")), "512 o");
        // Where `Intl` answers with a word, the symbol is derived. The interface
        // applies the same rule, so neither side says `512 byte`.
        assert_eq!(human_bytes(512, &Localizer::new("nl")), "512 B");
    }

    /// Every guardrail name reaches `confirm::ALL`.
    ///
    /// Read out of this file rather than listed again: a name added to the
    /// module and forgotten in the slice refuses a caller that answered every
    /// question, and nothing else in the build says so.
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

        // The count is the guard on the parser: stop matching and every
        // assertion below passes having read nothing.
        assert!(names.len() >= 4, "only {} name(s) parsed: {names:?}", names.len());

        let listed = module
            .split_once("pub const ALL: &[&str] = &[")
            .expect("the list")
            .1
            .split_once(']')
            .expect("its bracket")
            .0;
        for name in names {
            assert!(listed.contains(name), "{name} is a guardrail nobody can answer");
        }
    }
}
