//! Prometheus exposition.
//!
//! Written by hand rather than through a metrics crate. Everything worth
//! exposing is already a row count, so a registry would add a dependency, a
//! parallel source of truth and a drift risk, to serialise five queries.
//!
//! Everything here is a **gauge**: the current state of the library, read at
//! scrape time. Counters would require in-process accumulators that reset on
//! restart and disagree with the database — the database is the truth, and it
//! survives restarts.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

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

pub async fn metrics(State(state): State<AppState>) -> AppResult<Response> {
    let pool = &state.pool;
    let mut families: Vec<Family> = Vec::new();

    // Media per (instance, type). Grouped rather than totalled: "which library
    // grew" is the question someone actually asks of a graph.
    let media: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT i.name, m.media_type, COUNT(*)
           FROM media m JOIN instances i ON i.id = m.instance_id
          GROUP BY i.name, m.media_type",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_media_total",
        help: "Media items Routarr tracks.",
        samples: media
            .into_iter()
            .map(|(instance, kind, count)| {
                (
                    format!("instance=\"{}\",media_type=\"{}\"", label(&instance), label(&kind)),
                    count as f64,
                )
            })
            .collect(),
    });

    // Where the engine currently wants each item. Answers "is the library
    // drifting" without opening the interface.
    let by_category: Vec<(String, i64)> = sqlx::query_as(
        "SELECT target_category, COUNT(*)
           FROM decisions
          WHERE superseded = 0 AND status IN ('pending', 'applied')
          GROUP BY target_category",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_decisions_by_category",
        help: "Current decisions per target category.",
        samples: by_category
            .into_iter()
            .map(|(category, count)| (format!("category=\"{}\"", label(&category)), count as f64))
            .collect(),
    });

    let by_status: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM decisions WHERE superseded = 0 GROUP BY status",
    )
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
    let instances: Vec<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT name, last_sync_status, enabled FROM instances")
            .fetch_all(pool)
            .await?;
    families.push(Family {
        name: "routarr_instance_up",
        help: "1 when the last sync of this instance succeeded, 0 otherwise.",
        samples: instances
            .iter()
            .map(|(name, status, _)| {
                let up = status.as_deref() == Some("success");
                (format!("instance=\"{}\"", label(name)), if up { 1.0 } else { 0.0 })
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_enabled",
        help: "1 when this instance is enabled for syncing and routing.",
        samples: instances
            .iter()
            .map(|(name, _, enabled)| {
                (format!("instance=\"{}\"", label(name)), if *enabled { 1.0 } else { 0.0 })
            })
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
