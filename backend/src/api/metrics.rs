//! Prometheus exposition.
//!
//! Written by hand rather than through a metrics crate. Everything worth
//! exposing is already a row count, so a registry would add a dependency, a
//! parallel source of truth and a drift risk, to serialise a few queries.
//!
//! Everything here is a **gauge**: the current state of the library, read at
//! scrape time. Counters would require in-process accumulators that reset on
//! restart and disagree with the database. The database is the truth, and it
//! survives restarts.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use sqlx::AssertSqlSafe;

use crate::error::AppResult;
use crate::state::AppState;

/// One metric family, rendered with its HELP and TYPE preamble.
struct Family<'a> {
    name: &'a str,
    help: &'a str,
    /// (label set already formatted, value). An empty label set writes none.
    samples: Vec<(String, f64)>,
}

impl Family<'_> {
    fn render(&self, out: &mut String) {
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} gauge\n", self.name));
        for (labels, value) in &self.samples {
            if labels.is_empty() {
                out.push_str(&format!("{} {}\n", self.name, value));
            } else {
                out.push_str(&format!("{}{{{}}} {}\n", self.name, labels, value));
            }
        }
    }
}

/// Escape a label value: backslash, quote and newline, per the exposition format.
///
/// Category names are user-supplied. An unescaped quote would produce a line
/// Prometheus rejects, and the whole scrape with it.
fn label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// The labels naming an Arr instance. Not `instance`: Prometheus sets that on
/// every series to the target it scraped, and renames a label of that name
/// `exported_instance`. Keyed by id as well, which a rename leaves alone, so a
/// renamed instance keeps its series.
fn arr_instance(id: &str, name: &str) -> String {
    format!("arr_instance=\"{}\",arr_instance_id=\"{}\"", label(name), label(id))
}

fn source_label(source: &str) -> String {
    format!("source=\"{}\"", label(source))
}

/// Each title's latest standing decision, over `decisions d`: what the
/// engine wants for it now. An older one an apply left standing is history,
/// and counted, it reads as a library still moving.
pub(crate) const LATEST: &str = "d.superseded = 0
    AND NOT EXISTS (SELECT 1 FROM decisions later
                     WHERE later.media_id = d.media_id AND later.superseded = 0
                       AND (later.decided_at > d.decided_at
                            OR (later.decided_at = d.decided_at AND later.rowid > d.rowid)))";

pub async fn metrics(State(state): State<AppState>) -> AppResult<Response> {
    let pool = &state.pool;
    let mut families: Vec<Family> = Vec::new();

    // Media per (instance, type). Grouped rather than totalled: "which library
    // grew" is the question someone actually asks of a graph.
    let media: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT i.id, i.name, m.media_type, COUNT(*)
           FROM media m JOIN instances i ON i.id = m.instance_id
          GROUP BY i.id, i.name, m.media_type",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_media_total",
        help: "Media items Routarr tracks.",
        samples: media
            .into_iter()
            .map(|(id, name, kind, count)| {
                (
                    format!("{},media_type=\"{}\"", arr_instance(&id, &name), label(&kind)),
                    count as f64,
                )
            })
            .collect(),
    });

    // Where the engine currently wants each item, a title already there
    // included. Answers "is the library drifting" without opening the
    // interface.
    let by_category: Vec<(String, i64)> =
        sqlx::query_as("SELECT category, COUNT(*) FROM media_routing GROUP BY category")
            .fetch_all(pool)
            .await?;
    families.push(Family {
        name: "routarr_decisions_by_category",
        help: "Titles per category the last run sends them to.",
        samples: by_category
            .into_iter()
            .map(|(category, count)| (format!("category=\"{}\"", label(&category)), count as f64))
            .collect(),
    });

    let by_status: Vec<(String, i64)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT d.status, COUNT(*) FROM decisions d WHERE {LATEST} GROUP BY d.status"
    )))
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_decisions_by_status",
        help: "Current decisions per status. `pending` is what awaits a human.",
        samples: by_status
            .into_iter()
            .map(|(status, count)| (format!("status=\"{}\"", label(&status)), count as f64))
            .collect(),
    });

    // Instance health as 1/0, from the last sync. The obvious thing to alert on.
    let instances: Vec<(String, String, Option<String>, bool)> =
        sqlx::query_as("SELECT id, name, last_sync_status, enabled FROM instances")
            .fetch_all(pool)
            .await?;
    families.push(Family {
        name: "routarr_instance_up",
        help: "1 when the last sync of this instance succeeded, 0 otherwise.",
        samples: instances
            .iter()
            .map(|(id, name, status, _)| {
                let up = status.as_deref() == Some("success");
                (arr_instance(id, name), if up { 1.0 } else { 0.0 })
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_enabled",
        help: "1 when this instance is enabled for syncing and routing.",
        samples: instances
            .iter()
            .map(|(id, name, _, enabled)| {
                (arr_instance(id, name), if *enabled { 1.0 } else { 0.0 })
            })
            .collect(),
    });

    // The titles nothing describes, by the predicate the diagnostics count and
    // the library column share: a third spelling would disagree with both.
    let known = crate::api::media::metadata_predicate(&state.settings().await);
    let undescribed: Vec<(String, String, String, i64)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT i.id, i.name, m.media_type, COUNT(*)
           FROM media m JOIN instances i ON i.id = m.instance_id
          WHERE NOT ({known})
          GROUP BY i.id, i.name, m.media_type"
    )))
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_media_without_metadata",
        help: "Titles no metadata source describes, which no rule on metadata can match.",
        samples: undescribed
            .into_iter()
            .map(|(id, name, kind, count)| {
                (
                    format!("{},media_type=\"{}\"", arr_instance(&id, &name), label(&kind)),
                    count as f64,
                )
            })
            .collect(),
    });

    // Source health as 1/0, from the last probe `/health` ran. A source never
    // probed has no sample: not asked is not down.
    let probed: Vec<(String, bool)> = sqlx::query_as(
        "SELECT subject, reachable FROM probe_results WHERE subject GLOB 'source:*'",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_metadata_source_up",
        help: "1 when the last probe of this metadata source found it answering, 0 otherwise.",
        samples: probed
            .iter()
            .filter_map(|(subject, up)| Some((subject.strip_prefix("source:")?, up)))
            .map(|(source, up)| (source_label(source), if *up { 1.0 } else { 0.0 }))
            .collect(),
    });

    let cached: Vec<(String, i64)> =
        sqlx::query_as("SELECT source, COUNT(*) FROM metadata_cache GROUP BY source")
            .fetch_all(pool)
            .await?;
    families.push(Family {
        name: "routarr_metadata_cache_entries",
        help: "Answers and misses each metadata source has cached.",
        samples: cached
            .into_iter()
            .map(|(source, count)| (source_label(&source), count as f64))
            .collect(),
    });

    // A spent quota stops a source until the next UTC day, while its probe
    // stood earlier in the day, so the count is what says it.
    let mut spent = Vec::new();
    let mut limits = Vec::new();
    for source in state.metadata_sources().await {
        let Some(quota) = source.daily_quota() else { continue };
        let today: Option<i64> = sqlx::query_scalar(
            "SELECT spent FROM source_requests WHERE source = ? AND day = date('now')",
        )
        .bind(source.id())
        .fetch_optional(pool)
        .await?;
        spent.push((source_label(source.id()), today.unwrap_or(0) as f64));
        limits.push((source_label(source.id()), quota.limit() as f64));
    }
    families.push(Family {
        name: "routarr_metadata_quota_spent",
        help: "Requests a metadata source with a daily quota was sent today, UTC.",
        samples: spent,
    });
    families.push(Family {
        name: "routarr_metadata_quota_limit",
        help: "Requests a metadata source with a daily quota may be sent a day.",
        samples: limits,
    });

    // The date of the newest archive the schedule or the owner took, which a
    // rule alerts on once it is older than the interval: a failed backup says
    // so in the jobs alone, and the log retention trims those.
    families.push(Family {
        name: "routarr_backup_last_success_timestamp_seconds",
        help: "When the newest archive was written, in seconds since the Unix epoch.",
        samples: crate::services::backup::last_taken_at(&state)
            .map(|taken| (String::new(), taken.timestamp() as f64))
            .into_iter()
            .collect(),
    });
    families.push(Family {
        name: "routarr_data_free_bytes",
        help: "Bytes left on the disk holding the database and its backups.",
        samples: fs4::available_space(&state.config.data_dir)
            .map(|free| (String::new(), free as f64))
            .into_iter()
            .collect(),
    });

    let jobs: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT kind, status, COUNT(*) FROM jobs GROUP BY kind, status")
            .fetch_all(pool)
            .await?;
    families.push(Family {
        name: "routarr_jobs_total",
        help: "Recorded background jobs, by kind and outcome.",
        samples: jobs
            .into_iter()
            .map(|(kind, status, count)| {
                (format!("kind=\"{}\",status=\"{}\"", label(&kind), label(&status)), count as f64)
            })
            .collect(),
    });

    // The two switches that decide whether a click writes anything. Worth a
    // graph annotation when someone asks why nothing moved for a week.
    families.push(Family {
        name: "routarr_dry_run",
        help: "1 when global dry-run is blocking every write.",
        samples: vec![(
            String::new(),
            if state.bool_setting("global_dry_run", true).await { 1.0 } else { 0.0 },
        )],
    });
    families.push(Family {
        name: "routarr_auto_apply_enabled",
        help: "1 when routing decisions are applied without confirmation.",
        samples: vec![(
            String::new(),
            if state.bool_setting("auto_apply_enabled", false).await { 1.0 } else { 0.0 },
        )],
    });

    let mut body = String::new();
    for family in &families {
        family.render(&mut body);
    }

    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response())
}
