//! Prometheus exposition. Every family is read from the database at scrape time, so a restart
//! loses nothing: a gauge, or a counter named `_total` whose rows are never deleted.

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
        let kind = if self.name.ends_with("_total") { "counter" } else { "gauge" };
        out.push_str(&format!("# TYPE {} {kind}\n", self.name));
        for (labels, value) in &self.samples {
            if labels.is_empty() {
                out.push_str(&format!("{} {}\n", self.name, value));
            } else {
                out.push_str(&format!("{}{{{}}} {}\n", self.name, labels, value));
            }
        }
    }
}

/// Escape a label value: one unescaped quote in a user's name fails the whole scrape.
fn label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// The labels naming an Arr instance. `instance` belongs to Prometheus, and the id keeps a
/// renamed instance on its series.
fn arr_instance(id: &str, name: &str) -> String {
    format!("arr_instance=\"{}\",arr_instance_id=\"{}\"", label(name), label(id))
}

fn source_label(source: &str) -> String {
    format!("source=\"{}\"", label(source))
}

/// Each title's latest standing decision, over `decisions d`. Counting an older one an apply left
/// standing would show a library still moving.
pub(crate) const LATEST: &str = "d.superseded = 0
    AND NOT EXISTS (SELECT 1 FROM decisions later
                     WHERE later.media_id = d.media_id AND later.superseded = 0
                       AND (later.decided_at > d.decided_at
                            OR (later.decided_at = d.decided_at AND later.rowid > d.rowid)))";

/// An instance's id, name, last sync outcome, enabled, last success, last try, auto-apply, last
/// webhook and sync interval in minutes.
type Synced = (
    String,
    String,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    i64,
);

pub async fn metrics(State(state): State<AppState>) -> AppResult<Response> {
    let pool = &state.pool;
    let mut families: Vec<Family> = Vec::new();

    families.push(Family {
        name: "routarr_build_info",
        help: "Always 1, labelled with the release running.",
        samples: vec![(format!("version=\"{}\"", label(env!("CARGO_PKG_VERSION"))), 1.0)],
    });
    let mut warnings: std::collections::BTreeMap<(&str, &str), u32> = Default::default();
    for warning in crate::api::health::warning_codes(&state).await? {
        *warnings.entry(warning).or_default() += 1;
    }
    families.push(Family {
        name: "routarr_warnings",
        help: "The warnings the interface shows, by code and severity, at its own thresholds.",
        samples: warnings
            .into_iter()
            .map(|((code, severity), count)| {
                (format!("code=\"{code}\",severity=\"{severity}\""), f64::from(count))
            })
            .collect(),
    });

    let media: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT i.id, i.name, m.media_type, COUNT(*)
           FROM media m JOIN instances i ON i.id = m.instance_id
          GROUP BY i.id, i.name, m.media_type",
    )
    .fetch_all(pool)
    .await?;
    let media: Vec<(String, f64)> = media
        .into_iter()
        .map(|(id, name, kind, count)| {
            (format!("{},media_type=\"{}\"", arr_instance(&id, &name), label(&kind)), count as f64)
        })
        .collect();
    families.push(Family {
        name: "routarr_media_items",
        help: "Media items Routarr tracks.",
        samples: media,
    });

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

    // Without the sync timestamps, a sync that stopped running would read as up for ever.
    let instances: Vec<Synced> = sqlx::query_as(
        "SELECT id, name, last_sync_status, enabled, last_sync_at, last_sync_attempt_at, auto_apply,
                last_webhook_at, sync_interval_minutes
           FROM instances",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_instance_up",
        help: "1 when the last sync of this instance succeeded, 0 otherwise.",
        samples: instances
            .iter()
            .map(|(id, name, status, ..)| {
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
            .map(|(id, name, _, enabled, ..)| {
                (arr_instance(id, name), if *enabled { 1.0 } else { 0.0 })
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_auto_apply",
        help: "1 when automatic application may write to this instance, under the global switch.",
        samples: instances
            .iter()
            .map(|(id, name, .., auto_apply, _, _)| {
                (arr_instance(id, name), if *auto_apply { 1.0 } else { 0.0 })
            })
            .collect(),
    });
    let at = |stamp: &Option<String>| {
        stamp.as_deref().and_then(crate::services::routing::parse_timestamp)
    };
    families.push(Family {
        name: "routarr_instance_last_sync_timestamp_seconds",
        help: "When a sync of this instance last succeeded, in seconds since the Unix epoch.",
        samples: instances
            .iter()
            .filter_map(|(id, name, _, _, synced, ..)| {
                Some((arr_instance(id, name), at(synced)?.timestamp() as f64))
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_sync_interval_seconds",
        help: "How often this instance is synced, in seconds: a sync older than twice it is late.",
        samples: instances
            .iter()
            .map(|(id, name, .., minutes)| (arr_instance(id, name), (*minutes * 60) as f64))
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_last_webhook_timestamp_seconds",
        help: "When a webhook from this instance last arrived, in seconds since the Unix epoch.",
        samples: instances
            .iter()
            .filter_map(|(id, name, .., heard, _)| {
                Some((arr_instance(id, name), at(heard)?.timestamp() as f64))
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_instance_last_sync_attempt_timestamp_seconds",
        help: "When a sync of this instance was last tried, in seconds since the Unix epoch.",
        samples: instances
            .iter()
            .filter_map(|(id, name, _, _, _, tried, ..)| {
                Some((arr_instance(id, name), at(tried)?.timestamp() as f64))
            })
            .collect(),
    });

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

    // A source never probed has no sample: not asked is not down.
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

    // A probe's verdict stands all day, so only this count shows a quota spent since.
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

    families.push(Family {
        name: "routarr_backup_last_success_timestamp_seconds",
        help: "When the newest archive was written, in seconds since the Unix epoch.",
        samples: crate::services::backup::last_taken_at(&state)
            .map(|taken| (String::new(), taken.timestamp() as f64))
            .into_iter()
            .collect(),
    });
    families.push(Family {
        name: "routarr_backup_copy_last_success_timestamp_seconds",
        help: "When a copy into ROUTARR_BACKUP_COPY_DIR last ended well, in seconds since the \
               Unix epoch. Absent without the variable.",
        samples: crate::services::backup::last_copied_at(&state)
            .await
            .map(|copied| (String::new(), copied.timestamp() as f64))
            .into_iter()
            .collect(),
    });
    let deprecated: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT method, path, SUM(calls) FROM deprecated_calls
          GROUP BY method, path ORDER BY path, method",
    )
    .fetch_all(pool)
    .await?;
    families.push(Family {
        name: "routarr_deprecated_api_requests_total",
        help: "Calls to each deprecated API path, by every caller, since the path was deprecated.",
        samples: deprecated
            .into_iter()
            .map(|(method, path, calls)| {
                (format!("method=\"{}\",path=\"{}\"", label(&method), label(&path)), calls as f64)
            })
            .collect(),
    });
    families.push(Family {
        name: "routarr_database_bytes",
        help: "The database file's size, which a backup and the free space are weighed against.",
        samples: std::fs::metadata(&state.config.db_path)
            .map(|meta| (String::new(), meta.len() as f64))
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
    let jobs: Vec<(String, f64)> = jobs
        .into_iter()
        .map(|(kind, status, count)| {
            (format!("kind=\"{}\",status=\"{}\"", label(&kind), label(&status)), count as f64)
        })
        .collect();
    families.push(Family {
        name: "routarr_jobs",
        help: "Background jobs the retention keeps, by kind and outcome.",
        samples: jobs,
    });

    families.push(Family {
        name: "routarr_dry_run",
        help: "1 when global dry-run is blocking every write.",
        samples: vec![(
            String::new(),
            if state.bool_setting("global_dry_run").await { 1.0 } else { 0.0 },
        )],
    });
    families.push(Family {
        name: "routarr_auto_apply_enabled",
        help: "1 when routing decisions are applied without confirmation.",
        samples: vec![(
            String::new(),
            if state.bool_setting("auto_apply_enabled").await { 1.0 } else { 0.0 },
        )],
    });

    let (failing, tests) = crate::services::rule_tests::failing(pool).await?;
    families.push(Family {
        name: "routarr_rule_tests",
        help: "Rule tests pinned.",
        samples: vec![(String::new(), tests as f64)],
    });
    families.push(Family {
        name: "routarr_rule_tests_failing",
        help: "Rule tests the last run found failing. Auto-apply holds while one does.",
        samples: vec![(String::new(), failing as f64)],
    });

    let mut body = String::new();
    for family in &families {
        family.render(&mut body);
    }

    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response())
}
