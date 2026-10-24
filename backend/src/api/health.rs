//! Health and diagnostics.

use super::{Json, Query};
use axum::extract::State;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;

use sqlx::AssertSqlSafe;

use super::onboarding::step;
use crate::error::{AppError, AppResult};
use crate::localization::Localizer;
use crate::models::{Instance, WebhookCheck, WebhookState};
use crate::services::metadata;
use crate::state::{AppState, Settings};

/// What the liveness probe answers. No version: `/status` gives it to a caller with a key.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Pong {
    /// Always `ok`.
    pub status: &'static str,
}

/// Liveness probe: no database work, no outbound calls, no authentication.
pub async fn ping() -> Json<Pong> {
    Json(Pong { status: "ok" })
}

/// Counts and warnings from the database alone, so an unreachable Arr never holds up the top bar.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StatusResponse {
    pub version: String,
    /// Whether the global dry run holds, in which case nothing is moved.
    pub dry_run: bool,
    pub running_jobs: i64,
    pub pending_decisions: i64,
    /// Failed moves still asking for attention: titles whose latest decision failed and that no
    /// later run has settled.
    pub failed_decisions: i64,
    /// Configuration problems detectable without touching the network.
    pub warnings: Vec<Warning>,
}

/// One warning, in the reader's language.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Warning {
    /// What the warning is about, stable across releases and languages: `api_unauthenticated`,
    /// `api_external_auth`, `source_needs_key`, `source_key_unlisted`, `source_unreachable`,
    /// `source_quota_spent`, `source_rate_limited`, `source_key_refused`, `instance_unreachable`,
    /// `instance_sync_failing`, `unmapped_categories`, `no_enabled_instance`, `missing_metadata`,
    /// `scheduler_panicked`, `backup_failed`, `backup_overdue`, `data_disk_low`,
    /// `setting_above_maximum`, `instance_without_mapping`,
    /// `certification_country_outside_regions`, `certification_country_changed`, `auto_apply_held`,
    /// `arr_below_version`, `oidc_open_to_anyone`, `backup_passphrase_unreadable`,
    /// `rule_tests_failing`, `webhook_never_delivered`, `webhook_misconfigured`,
    /// `backup_copy_failed`, `deprecated_api_called`, `removal_held` or `restore_pending`. The list
    /// may grow.
    pub code: &'static str,
    /// What it means, in the interface language.
    pub message: String,
    /// `error` for a fault that stops part of Routarr working, `warning` for a configuration to
    /// look at, `info` for a state worth knowing. An open list. `/health` reads `degraded` while
    /// one is `error`.
    pub severity: &'static str,
    /// The getting-started step whose banner says the same while the step is open: `instance`,
    /// `categories` or `metadata`. Null for most.
    pub guide_step: Option<&'static str>,
}

impl Warning {
    fn new(code: &'static str, message: String) -> Self {
        Self { code, message, severity: severity_of(code), guide_step: None }
    }

    fn restating(step: &'static str, code: &'static str, message: String) -> Self {
        Self { code, message, severity: severity_of(code), guide_step: Some(step) }
    }
}

/// Every warning code with its severity: `error` for a fault that stops something Routarr does,
/// which turns `/health` `degraded`, `info` for a choice the operator made or a fact about the
/// library. The contract's list and the shipped alert rules are held to it by their tests.
pub(crate) const CODES: &[(&str, &str)] = &[
    ("api_unauthenticated", "warning"),
    ("api_external_auth", "info"),
    ("source_needs_key", "warning"),
    ("source_key_unlisted", "info"),
    ("source_unreachable", "error"),
    ("source_quota_spent", "warning"),
    ("source_rate_limited", "warning"),
    ("source_key_refused", "error"),
    ("instance_unreachable", "error"),
    ("instance_sync_failing", "error"),
    ("unmapped_categories", "warning"),
    ("no_enabled_instance", "warning"),
    ("missing_metadata", "info"),
    ("scheduler_panicked", "error"),
    ("backup_failed", "error"),
    ("backup_overdue", "warning"),
    ("data_disk_low", "error"),
    ("setting_above_maximum", "warning"),
    ("instance_without_mapping", "warning"),
    ("certification_country_outside_regions", "info"),
    ("certification_country_changed", "info"),
    ("auto_apply_held", "warning"),
    ("arr_below_version", "warning"),
    ("oidc_open_to_anyone", "warning"),
    ("backup_passphrase_unreadable", "error"),
    ("rule_tests_failing", "warning"),
    ("webhook_never_delivered", "warning"),
    ("webhook_misconfigured", "warning"),
    ("backup_copy_failed", "warning"),
    ("deprecated_api_called", "info"),
    ("removal_held", "warning"),
    ("restore_pending", "warning"),
];

fn severity_of(code: &str) -> &'static str {
    let severity = CODES.iter().find(|(listed, _)| *listed == code).map(|(_, severity)| *severity);
    debug_assert!(severity.is_some(), "{code} is missing from CODES");
    severity.unwrap_or("warning")
}

/// A failed move still asking for attention, as a condition over `decisions d`: the title's
/// latest standing decision, with no stored run of the title since.
fn open_failure() -> String {
    use crate::api::metrics::{LATEST, SETTLED};
    format!("d.status = 'failed' AND {LATEST} AND NOT ({SETTLED})")
}

/// Cheap status for the persistent chrome of the UI.
pub async fn status(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
) -> AppResult<Json<StatusResponse>> {
    let settings = state.settings().await;
    let localizer = Localizer::new(&AppState::language_from(&settings));
    let row: (i64, i64, i64) = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT
            (SELECT COUNT(*) FROM jobs WHERE status = 'running'),
            (SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0),
            (SELECT COUNT(*) FROM decisions d WHERE {})",
        open_failure()
    )))
    .fetch_one(&state.pool)
    .await?;

    let warnings = offline_warnings(&state, &localizer, &settings, reader(&identity)).await?;

    Ok(Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        dry_run: settings.bool("global_dry_run"),
        running_jobs: row.0,
        pending_decisions: row.1,
        failed_decisions: row.2,
        warnings,
    }))
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// `ok`, `degraded` while a warning of severity `error` stands, or
    /// `failing`, answered 503, when the database does not answer.
    pub status: &'static str,
    /// This server's release.
    pub version: &'static str,
    /// `connected`, or `error` when the database does not answer.
    pub database: &'static str,
    /// Every instance, in name order.
    pub instances: Vec<InstanceHealth>,
    pub metadata: MetadataHealth,
    pub stats: AppStats,
    /// Every standing warning: those `/status` lists, with what this probe found.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InstanceHealth {
    /// The instance's id, as `/instances` lists it.
    pub id: String,
    /// The name it was given in Routarr.
    pub name: String,
    /// `radarr` or `sonarr`.
    pub instance_type: String,
    /// `connected`, `disabled`, `unchecked` when nothing was probed, or `error`.
    pub status: String,
    /// Why the probe failed, in the interface language: what to change, when
    /// the address or the key explains it. Null unless `status` is an error.
    pub detail: Option<String>,
    /// The Arr's release, as a probe that reached it read it.
    pub version: Option<String>,
    /// When a sync last succeeded.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_sync: Option<String>,
    /// How the last sync ended: `success`, or `error: ` and the reason. Deprecated: the reason
    /// alone is `last_sync_error`.
    #[schema(deprecated)]
    pub last_sync_status: Option<String>,
    /// Why the last sync failed, in the interface language. Null when it succeeded or none ran.
    pub last_sync_error: Option<String>,
    /// The titles the last sync read from it.
    pub media_count: i64,
    /// Its root folders mapped to a category.
    pub mapped_root_folders: i64,
    /// When a webhook from it last arrived. Null while none has since it was added or its token
    /// rotated.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_webhook_at: Option<String>,
    /// The event that delivery carried, as `Test` or `MovieAdded`.
    pub last_webhook_event: Option<String>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct MetadataHealth {
    /// Every source in the user's order, whether or not it can answer.
    pub providers: Vec<MetadataProviderHealth>,
    pub cached_items: i64,
    /// Items no source describes at all.
    pub media_missing_metadata: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetadataProviderHealth {
    pub id: String,
    pub display_name: String,
    pub needs_key: bool,
    /// A source that needs a key and has one, or that needs none.
    pub configured: bool,
    /// Probed only for a fetched, configured source. Null for the Arr, whose
    /// instances are probed on their own.
    pub connected: Option<bool>,
    /// Why a probed source did not answer: `key_refused`, `quota_spent`,
    /// `rate_limited` or `unreachable`. An open list. Null otherwise.
    pub reason: Option<&'static str>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct AppStats {
    pub total_instances: i64,
    pub total_media: i64,
    pub total_movies: i64,
    pub total_series: i64,
    pub total_rules: i64,
    pub enabled_rules: i64,
    pub total_overrides: i64,
    pub pending_decisions: i64,
    pub applied_decisions: i64,
    /// Failed moves still asking for attention, as `/status` counts them.
    pub failed_decisions: i64,
    pub unmapped_categories: i64,
    pub running_jobs: i64,
}

/// Whether to reach out to the Arrs and the metadata sources. The dashboard asks `probe=false`:
/// a probe waits a full connect timeout per unreachable Arr.
#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HealthQuery {
    /// `false` reads the database alone: every enabled instance reads `unchecked`, every source
    /// `connected: null`, and `warnings` repeat what the last probe found. Defaults to true for
    /// a caller holding the operate scope, false otherwise and for a request another site set
    /// off. `true` needs the operate scope.
    probe: Option<bool>,
    /// `true` probes again even when a probe of the same configuration ran in the last 30
    /// seconds, whose findings are otherwise answered again.
    fresh: Option<bool>,
}

/// Full diagnostics. The probes run concurrently, so unreachable Arrs cost a single timeout.
pub async fn health_check(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    headers: axum::http::HeaderMap,
    Query(query): Query<HealthQuery>,
) -> AppResult<(StatusCode, Json<HealthResponse>)> {
    // A probe spends a connect timeout per host and records what it found.
    let operates = identity.holds(crate::services::applications::Scope::Operate);
    if query.probe == Some(true) && !operates {
        return Err(AppError::Forbidden(
            "probe=true reaches every Arr and every source now and needs the operate scope. \
             Leave it out to read what the last probe found."
                .into(),
        ));
    }
    // First: a database that does not answer reads `failing`, not a 500 or an empty library.
    if let Err(e) = sqlx::query("SELECT 1").execute(&state.pool).await {
        tracing::error!("The database does not answer the health check: {e}");
        let failing = HealthResponse {
            status: "failing",
            version: env!("CARGO_PKG_VERSION"),
            database: "error",
            ..HealthResponse::default()
        };
        return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(failing)));
    }

    // Another origin, another port of this host included, could set off a probe with the cookie.
    let elsewhere = headers.get("sec-fetch-site").is_some_and(|value| {
        !value.to_str().is_ok_and(|site| {
            site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none")
        })
    });
    let probe = query.probe.unwrap_or(operates) && !elsewhere;
    let pool = &state.pool;
    let settings = state.settings().await;
    let localizer = Localizer::new(&AppState::language_from(&settings));

    let instances = state.instances(false).await?;
    let probed = match probe {
        true => Some(probed(&state, &instances, &settings, query.fresh.unwrap_or(false)).await?),
        false => None,
    };
    let mut instance_health = Vec::with_capacity(instances.len());
    for instance in &instances {
        let reached = match &probed {
            _ if !instance.enabled => Reached::state("disabled"),
            Some(probe) => {
                probe.instances.get(&instance.id).cloned().unwrap_or(Reached::state("unchecked"))
            }
            None => Reached::state("unchecked"),
        };
        instance_health.push(counts_for(&state, instance, reached, &localizer).await?);
    }

    let stats = gather_stats(&state).await?;

    let cached_items: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache").fetch_one(pool).await?;
    let known = crate::api::media::metadata_predicate(&settings);
    let media_missing_metadata: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM media m WHERE NOT ({known})"
    )))
    .fetch_one(pool)
    .await?;

    let none = HashMap::new();
    let connectivity = probed.as_ref().map_or(&none, |probe| &probe.sources);
    let providers = provider_health(&state, connectivity, &settings);

    // After the probe, whose findings reach the warnings through `probe_results`.
    let warnings = offline_warnings(&state, &localizer, &settings, reader(&identity)).await?;
    let faulty = warnings.iter().any(|warning| warning.severity == "error");

    Ok((
        StatusCode::OK,
        Json(HealthResponse {
            status: if faulty { "degraded" } else { "ok" },
            version: env!("CARGO_PKG_VERSION"),
            database: "connected",
            instances: instance_health,
            metadata: MetadataHealth { providers, cached_items, media_missing_metadata },
            stats,
            warnings,
        }),
    ))
}

/// How long a probe's findings answer again for the same configuration.
const PROBE_REUSE: std::time::Duration = std::time::Duration::from_secs(30);

/// What one probe of every Arr and source found, and of which configuration.
pub(crate) struct Probe {
    at: std::time::Instant,
    of: u64,
    instances: HashMap<String, Reached>,
    sources: HashMap<String, Probed>,
}

/// The last probe when younger than [`PROBE_REUSE`] and of the same configuration, or a new one,
/// recorded before it is answered. One at a time: a caller arriving mid-probe waits and reads it.
async fn probed(
    state: &AppState,
    instances: &[Instance],
    settings: &Settings,
    fresh: bool,
) -> AppResult<Arc<Probe>> {
    let of = configuration_of(instances, settings);
    let mut last = state.last_probe.lock().await;
    if let Some(probe) =
        last.as_ref().filter(|probe| !fresh && probe.of == of && probe.at.elapsed() < PROBE_REUSE)
    {
        return Ok(Arc::clone(probe));
    }
    let (reached, sources) =
        futures::join!(probe_instances(state, instances), probe_sources(state));
    let probe = Arc::new(Probe { at: std::time::Instant::now(), of, instances: reached, sources });
    record_probe(state, &probe).await?;
    *last = Some(Arc::clone(&probe));
    Ok(probe)
}

/// What a probe depends on: every instance's address, key and type, and every setting. A probe
/// of another configuration is never reused, or a corrected address would read as still failing.
fn configuration_of(instances: &[Instance], settings: &Settings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for instance in instances {
        let Instance { id, instance_type, base_url, api_key, enabled, updated_at, .. } = instance;
        (id, instance_type, base_url, api_key, enabled, updated_at).hash(&mut hasher);
    }
    settings.hash(&mut hasher);
    hasher.finish()
}

/// A source's probe verdict counts a day, as a condition over `probe_results`: nothing probes on
/// a schedule, and an older verdict says nothing of the source now.
pub(crate) const FRESH_SOURCE_VERDICT: &str = "checked_at >= datetime('now', '-1 day')";

/// The failures the last probe found, for the endpoints that may not probe themselves, and the
/// ids of the instances found unreachable. A row whose instance was deleted since is dropped,
/// never reported as an unnamed failure, and so is one a sync reached the Arr after.
async fn last_probe_warnings(
    state: &AppState,
    localizer: &Localizer,
) -> AppResult<(Vec<Warning>, Vec<String>)> {
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT subject, detail, checked_at FROM probe_results
          WHERE reachable = 0 AND (subject NOT GLOB 'source:*' OR {FRESH_SOURCE_VERDICT})
          ORDER BY subject"
    )))
    .fetch_all(&state.pool)
    .await?;

    let mut warnings = Vec::new();
    let mut unreachable = Vec::new();
    for (subject, detail, checked_at) in rows {
        if let Some(id) = subject.strip_prefix("source:") {
            if let Some(info) = metadata::info(id) {
                let (code, key) =
                    VERDICTS.iter().find(|(said, ..)| detail.as_deref() == Some(*said)).map_or(
                        ("source_unreachable", "WarnProviderUnreachable"),
                        |(_, code, key)| (*code, *key),
                    );
                let provider = [("provider", info.display_name)];
                warnings.push(Warning::new(code, localizer.translate(key, &provider)));
            }
        } else if let Some(id) = subject.strip_prefix("instance:") {
            // A second's resolution: a sync and a probe of the same second count the sync later.
            let instance: Option<(String, String, String)> = sqlx::query_as(
                "SELECT name, instance_type, base_url FROM instances
                  WHERE id = ?
                    AND NOT (COALESCE(last_sync_status, '') = 'success'
                             AND COALESCE(last_sync_at, '') >= ?)",
            )
            .bind(id)
            .bind(&checked_at)
            .fetch_optional(&state.pool)
            .await?;
            if let Some((name, kind, base_url)) = instance {
                unreachable.push(id.to_string());
                let failure = Failure::read(detail.as_deref().unwrap_or_default());
                let told = failure.told(&kind, &base_url, localizer);
                warnings.push(Warning::new(
                    "instance_unreachable",
                    localizer.translate(
                        "WarnInstanceUnreachable",
                        &[("name", &name), ("status", &told)],
                    ),
                ));
            }
        }
    }
    Ok((warnings, unreachable))
}

/// What an Arr older than the oldest release Routarr supports loses, said of
/// the instance `name` when there is one, `None` at or above that release.
pub(crate) fn below_version(
    localizer: &Localizer,
    kind: &str,
    name: Option<&str>,
    version: &str,
) -> Option<String> {
    let minimum = crate::integrations::adapter::below_minimum(kind, version)?;
    let key = match (kind, name) {
        ("radarr", Some(_)) => "WarnRadarrBelowVersion",
        ("radarr", None) => "RadarrBelowVersion",
        (_, Some(_)) => "WarnSonarrBelowVersion",
        (_, None) => "SonarrBelowVersion",
    };
    let params = [("name", name.unwrap_or_default()), ("version", version), ("minimum", minimum)];
    Some(localizer.translate(key, &params))
}

/// Replaces the last probe's verdicts with this one's. A source's counts a day
/// ([`FRESH_SOURCE_VERDICT`]) and an instance's until the next probe, which is why the
/// diagnostics page probes live.
async fn record_probe(state: &AppState, probe: &Probe) -> AppResult<()> {
    let rows: Vec<(String, bool, Option<String>)> = probe
        .sources
        .iter()
        .map(|(id, probed)| (format!("source:{id}"), probed.connected, probed.detail.clone()))
        .chain(probe.instances.iter().map(|(id, reached)| {
            let failure = reached.failure.as_ref().map(Failure::stored);
            (format!("instance:{id}"), failure.is_none(), failure)
        }))
        .collect();

    let mut tx = state.pool.begin().await?;
    // Unpruned, a source switched off or left without a key reports its last failure for ever.
    let live: Vec<String> = rows.iter().map(|(subject, _, _)| subject.clone()).collect();
    let keep = crate::db::placeholders(live.len().max(1));
    let mut prune = sqlx::query(AssertSqlSafe(
        format!("DELETE FROM probe_results WHERE subject NOT IN ({keep})").as_str(),
    ));
    if live.is_empty() {
        prune = prune.bind("");
    }
    for subject in &live {
        prune = prune.bind(subject);
    }
    prune.execute(&mut *tx).await?;

    for (subject, reachable, detail) in rows {
        sqlx::query(
            "INSERT INTO probe_results (subject, reachable, detail, checked_at)
             VALUES (?, ?, ?, datetime('now'))
             ON CONFLICT(subject) DO UPDATE SET
                reachable = excluded.reachable,
                detail = excluded.detail,
                checked_at = excluded.checked_at",
        )
        .bind(&subject)
        .bind(reachable)
        .bind(&detail)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The `probe_results` detail of a source whose daily quota is spent.
const QUOTA_SPENT: &str = "quota spent";
/// The source asked for fewer requests.
const RATE_LIMITED: &str = "rate limited";
/// A source that needs a key refused the one it was given.
const KEY_REFUSED: &str = "key refused";

/// Each `probe_results` detail of a source that did not answer, with its warning code and
/// dictionary key. Any other failure is `source_unreachable`.
const VERDICTS: [(&str, &str, &str); 3] = [
    (QUOTA_SPENT, "source_quota_spent", "WarnProviderQuota"),
    (RATE_LIMITED, "source_rate_limited", "WarnProviderRateLimited"),
    (KEY_REFUSED, "source_key_refused", "WarnProviderKeyRefused"),
];

/// What a probe of one source found.
#[derive(Clone)]
struct Probed {
    connected: bool,
    /// One of [`VERDICTS`], or nothing.
    detail: Option<String>,
}

impl Probed {
    /// Why it did not answer: its warning code without the `source_` prefix.
    fn reason(&self) -> Option<&'static str> {
        if self.connected {
            return None;
        }
        let code = VERDICTS
            .iter()
            .find(|(said, ..)| self.detail.as_deref() == Some(*said))
            .map_or("source_unreachable", |(_, code, _)| *code);
        code.strip_prefix("source_")
    }

    /// The verdict on `outcome`. A refusal blames the key only for a source that takes one, and a
    /// closed day is a spent quota only for a source that `counts` one: Tenrai's is a block.
    fn of(outcome: &crate::error::AppResult<bool>, needs_key: bool, counts: bool) -> Self {
        let detail = match outcome {
            Ok(true) => return Self { connected: true, detail: None },
            Err(error) if counts && crate::integrations::is_quota_spent(error) => QUOTA_SPENT,
            Err(AppError::ExternalApi { status: 429, .. }) => RATE_LIMITED,
            Ok(false) | Err(AppError::ExternalApi { status: 401 | 403, .. }) if needs_key => {
                KEY_REFUSED
            }
            _ => return Self { connected: false, detail: None },
        };
        Self { connected: false, detail: Some(detail.to_string()) }
    }

    fn quota_spent() -> Self {
        Self { connected: false, detail: Some(QUOTA_SPENT.to_string()) }
    }

    fn rate_limited() -> Self {
        Self { connected: false, detail: Some(RATE_LIMITED.to_string()) }
    }
}

/// Reachability of every fetched source that is enabled and configured, probed concurrently so
/// the page never waits for the sum of their timeouts.
async fn probe_sources(state: &AppState) -> HashMap<String, Probed> {
    let sources = state.metadata_sources().await;
    let probes = sources
        .iter()
        .map(|source| async move { (source.id().to_string(), probe_source(state, source).await) });

    join_all(probes).await.into_iter().collect()
}

/// One source's probe, at its pace, or its 429 during a pass would read as the source down. One
/// with a daily quota is asked once a day, again once a key or quota is saved (`*_forgets_probe`).
/// One held past [`PROBE_PACE_WAIT`], or until tomorrow, is reported rate limited, no request sent.
async fn probe_source(state: &AppState, source: &metadata::FetchingSource) -> Probed {
    let needs_key = metadata::info(source.id()).is_some_and(|info| info.needs_key);
    if source.is_held_today(&state.pool).await {
        return Probed::rate_limited();
    }
    let Some(quota) = source.daily_quota() else {
        return match paced_probe(state, source).await {
            Some(outcome) => Probed::of(&outcome, needs_key, false),
            None => Probed::rate_limited(),
        };
    };
    let today: Option<(bool, Option<String>)> = sqlx::query_as(
        "SELECT reachable, detail FROM probe_results
          WHERE subject = ? AND checked_at >= date('now')",
    )
    .bind(format!("source:{}", source.id()))
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    if let Some((connected, detail)) = today {
        return Probed { connected, detail };
    }
    if quota.reserve(&state.pool, 1).await.unwrap_or(0) == 0 {
        return Probed::quota_spent();
    }
    let Some(outcome) = paced_probe(state, source).await else {
        if let Err(e) = quota.give_back(&state.pool, 1).await {
            tracing::warn!("Could not give back a request of {}: {e}", source.id());
        }
        return Probed::rate_limited();
    };
    Probed::of(&outcome, needs_key, true)
}

/// How long a probe waits for its source's pace: a hold past it is what the source said.
const PROBE_PACE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The source's connection test, paced like every other request to it, or `None` when the pace
/// holds it past [`PROBE_PACE_WAIT`]. A refusal for the rest of the day closes it.
async fn paced_probe(
    state: &AppState,
    source: &metadata::FetchingSource,
) -> Option<crate::error::AppResult<bool>> {
    let pace = state.paces.of(source);
    if !pace.acquire_by(tokio::time::Instant::now() + PROBE_PACE_WAIT).await {
        return None;
    }
    let outcome = source.test_connection().await;
    source.paced_after(&pace, &outcome).await;
    if let Err(e) = &outcome {
        tracing::warn!(source = source.id(), "The probe of a metadata source failed: {e}");
    }
    if outcome.as_ref().is_err_and(crate::integrations::is_quota_spent)
        && let Err(e) = source.close_the_day(&state.pool).await
    {
        tracing::warn!("Could not record that {} refuses until tomorrow: {e}", source.id());
    }
    Some(outcome)
}

/// The configured order, annotated with what each source can do right now.
fn provider_health(
    state: &AppState,
    connectivity: &HashMap<String, Probed>,
    settings: &Settings,
) -> Vec<MetadataProviderHealth> {
    let keys = state.provider_keys_from(settings);
    AppState::metadata_order_from(settings)
        .into_iter()
        .map(|provider| MetadataProviderHealth {
            id: provider.id.to_string(),
            display_name: provider.display_name.to_string(),
            needs_key: provider.needs_key,
            configured: metadata::is_usable(provider, &keys),
            connected: connectivity.get(provider.id).map(|probed| probed.connected),
            reason: connectivity.get(provider.id).and_then(Probed::reason),
        })
        .collect()
}

/// "No root folder is mapped to category `c`", the one predicate every count of unmapped
/// categories uses.
pub(crate) const UNMAPPED: &str =
    "NOT EXISTS (SELECT 1 FROM root_folders rf WHERE rf.category = c.name)";

/// The code and severity of each warning `/status` shows the owner, which the metrics count.
pub(crate) async fn warning_codes(
    state: &AppState,
) -> AppResult<Vec<(&'static str, &'static str)>> {
    let settings = state.settings().await;
    let warnings = offline_warnings(state, &Localizer::new("en"), &settings, None).await?;
    Ok(warnings.into_iter().map(|warning| (warning.code, warning.severity)).collect())
}

/// The application key reading the warnings, which sees no other key named.
fn reader(identity: &crate::api::auth::Identity) -> Option<&str> {
    identity.application.as_ref().map(|grant| grant.id.as_str())
}

/// Every warning known without the network, for the top bar and the diagnostics page alike,
/// so the badge counts what the page lists. Probe findings are read back from `probe_results`.
/// `reader`, an application key's id, keeps every other caller of an old path out.
async fn offline_warnings(
    state: &AppState,
    localizer: &Localizer,
    settings: &Settings,
    reader: Option<&str>,
) -> AppResult<Vec<Warning>> {
    let mut warnings = Vec::new();

    // `external` has its own `info` line: a proxy in front is not an unauthenticated API.
    match state.config.auth_mode {
        crate::config::AuthMode::None => {
            warnings.push(Warning::new(
                "api_unauthenticated",
                localizer.translate("WarnApiUnauthenticated", &[]),
            ));
        }
        crate::config::AuthMode::External => {
            warnings.push(Warning::new(
                "api_external_auth",
                localizer.translate("WarnApiExternalAuth", &[]),
            ));
        }
        crate::config::AuthMode::Oidc if state.config.oidc_allow_anyone => {
            warnings.push(Warning::new(
                "oidc_open_to_anyone",
                localizer.translate("WarnOidcOpenToAnyone", &[]),
            ));
        }
        crate::config::AuthMode::ApiKey
        | crate::config::AuthMode::Forms
        | crate::config::AuthMode::Oidc => {}
    }

    warnings.extend(metadata_warnings(state, localizer, settings));
    let (probed, unreachable) = last_probe_warnings(state, localizer).await?;
    warnings.extend(probed);

    // A sync the scheduler runs reaches each Arr unasked. One found unreachable is said once.
    let synced: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, last_sync_status FROM instances WHERE enabled = 1 ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;
    for (_, name, status) in synced.into_iter().filter(|(id, ..)| !unreachable.contains(id)) {
        if let Some(why) = crate::models::instance::sync_error_said(status.as_deref(), localizer) {
            let said = [("name", name.as_str()), ("status", why.as_str())];
            let message = localizer.translate("WarnInstanceSyncFailing", &said);
            warnings.push(Warning::new("instance_sync_failing", message));
        }
    }

    let versions: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT name, instance_type, arr_version FROM instances
          WHERE enabled = 1 AND arr_version IS NOT NULL ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;
    for (name, kind, version) in versions {
        if let Some(message) = below_version(localizer, &kind, Some(&name), &version) {
            warnings.push(Warning::new("arr_below_version", message));
        }
    }

    let (failed, total) = crate::services::rule_tests::failing(&state.pool).await?;
    if failed > 0 {
        let holding = settings.bool("auto_apply_enabled")
            && !settings.bool("global_dry_run")
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM instances WHERE enabled = 1 AND auto_apply = 1)",
            )
            .fetch_one(&state.pool)
            .await?;
        let key = if holding { "WarnRuleTestsFailingHeld" } else { "WarnRuleTestsFailing" };
        let counts = [("failed", failed.to_string()), ("total", total.to_string())];
        let counts: Vec<(&str, &str)> = counts.iter().map(|(k, v)| (*k, v.as_str())).collect();
        warnings.push(Warning::new("rule_tests_failing", localizer.translate(key, &counts)));
    }

    let kept: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, removal_held FROM instances WHERE enabled = 1 AND removal_held > 0
          ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;
    for (instance, count) in kept {
        let said = [("instance", instance.as_str()), ("count", &count.to_string())];
        warnings.push(Warning::new("removal_held", localizer.translate("WarnRemovalHeld", &said)));
    }

    let held: Vec<_> = state
        .auto_apply_held
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .cloned()
        .collect();
    for over in held {
        warnings.push(Warning::new(
            "auto_apply_held",
            localizer.translate(
                "WarnAutoApplyHeld",
                &[
                    ("instance", &over.instance),
                    ("count", &over.candidates.to_string()),
                    ("limit", &over.cap.to_string()),
                ],
            ),
        ));
    }

    let known = crate::api::media::metadata_predicate(settings);
    let row: (i64, i64, i64) = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT
            (SELECT COUNT(*) FROM categories c WHERE {UNMAPPED}),
            (SELECT COUNT(*) FROM instances WHERE enabled = 1),
            (SELECT COUNT(*) FROM media m WHERE NOT ({known}))"
    )))
    .fetch_one(&state.pool)
    .await?;

    if row.0 > 0 {
        warnings.push(Warning::restating(
            step::CATEGORIES,
            "unmapped_categories",
            localizer.translate("WarnUnmappedCategories", &[("count", &row.0.to_string())]),
        ));
    }
    if row.1 == 0 {
        warnings.push(Warning::restating(
            step::INSTANCE,
            "no_enabled_instance",
            localizer.translate("WarnNoEnabledInstance", &[]),
        ));
    }
    if row.2 > 0 {
        warnings.push(Warning::new(
            "missing_metadata",
            localizer.translate("WarnMissingMetadata", &[("count", &row.2.to_string())]),
        ));
    }

    // Scheduler panics in the last 24 hours, so a restart cannot hide them.
    let panicked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs
         WHERE kind = 'scheduler' AND status = 'failed'
           AND started_at > datetime('now', '-1 day')",
    )
    .fetch_one(&state.pool)
    .await?;
    if panicked > 0 {
        warnings.push(Warning::new(
            "scheduler_panicked",
            localizer.translate("WarnSchedulerPanicked", &[("count", &panicked.to_string())]),
        ));
    }

    let disk = on_disk(state).await;
    if settings.bool("backup_enabled") {
        let last: Option<String> = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE kind = 'backup'
              ORDER BY started_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_optional(&state.pool)
        .await?;
        let hours: i64 = settings.get::<i64>("backup_interval_hours").clamp(1, 24 * 7);
        if last.as_deref() == Some("failed") {
            warnings
                .push(Warning::new("backup_failed", localizer.translate("WarnBackupFailed", &[])));
        } else if backup_overdue(state, disk.last_backup, hours).await? {
            let message = localizer.translate("WarnBackupOverdue", &[]);
            warnings.push(Warning::new("backup_overdue", message));
        }
    }
    if let Some(folder) = &state.config.backup_copy_dir {
        let last: Option<String> = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE kind = 'backup_copy'
              ORDER BY started_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_optional(&state.pool)
        .await?;
        if last.as_deref() == Some("failed") {
            let folder = folder.display().to_string();
            let message = localizer.translate("WarnBackupCopyFailed", &[("folder", &folder)]);
            warnings.push(Warning::new("backup_copy_failed", message));
        }
    }
    if let Some(pending) = &disk.pending_restore {
        let message = localizer.translate("WarnRestorePending", &[("name", &pending.name)]);
        warnings.push(Warning::new("restore_pending", message));
    }
    if let Some(free) = disk.free_when_low {
        let message = disk_low_message(free, disk.replaced_bytes, localizer);
        warnings.push(Warning::new("data_disk_low", message));
    }

    // Never clamped: lowering a retention count deletes, which only the operator's save may do.
    for (key, label, max) in crate::services::settings::retention_counts() {
        let stored: i64 = settings.get(key);
        if stored > max {
            let setting = localizer.translate(label, &[]);
            warnings.push(Warning::new(
                "setting_above_maximum",
                localizer.translate(
                    "WarnSettingAboveMaximum",
                    &[("key", &setting), ("value", &stored.to_string()), ("max", &max.to_string())],
                ),
            ));
        }
    }

    // No backup is taken while the passphrase set is one the master key cannot open.
    if let Some(sealed) = settings
        .raw(crate::services::backup::PASSPHRASE_SETTING)
        .filter(|value| !value.trim().is_empty())
        && state.secrets.open(sealed).is_err()
    {
        warnings.push(Warning::new(
            "backup_passphrase_unreadable",
            localizer.translate("WarnBackupPassphraseUnreadable", &[]),
        ));
    }

    // Reported whether or not the instance answers: the two are separate things to fix.
    let unmapped: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM instances i
         WHERE i.enabled = 1
           AND NOT EXISTS (SELECT 1 FROM root_folders rf
                           WHERE rf.instance_id = i.id
                             AND rf.category IS NOT NULL AND rf.category != '')
         ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;

    for (name,) in unmapped {
        warnings.push(Warning::restating(
            step::CATEGORIES,
            "instance_without_mapping",
            localizer.translate("WarnInstanceNoMapping", &[("name", &name)]),
        ));
    }

    // An armed instance its Arr cannot reach leaves each new title to the next sync, unsaid. An
    // absent connection may hide behind a proxy rewriting the path, so only silence warns of it.
    if settings.bool("auto_apply_enabled") && !settings.bool("global_dry_run") {
        let armed: Vec<(String, bool, Option<String>)> = sqlx::query_as(
            "SELECT name,
                    last_webhook_at IS NULL
                      AND COALESCE(webhook_token_at, created_at) <= datetime('now', '-1 hour'),
                    webhook_check
               FROM instances
              WHERE enabled = 1 AND auto_apply = 1
              ORDER BY name",
        )
        .fetch_all(&state.pool)
        .await?;
        for (name, silent, check) in armed {
            let check = check.and_then(|check| serde_json::from_str::<WebhookCheck>(&check).ok());
            match check.filter(|check| check.state == WebhookState::Misconfigured) {
                Some(check) => warnings.extend(
                    check
                        .problems(&name, localizer)
                        .into_iter()
                        .map(|problem| Warning::new("webhook_misconfigured", problem)),
                ),
                None if silent => warnings.push(Warning::new(
                    "webhook_never_delivered",
                    localizer.translate("WarnWebhookNeverDelivered", &[("instance", &name)]),
                )),
                None => {}
            }
        }
    }

    let separator = localizer.translate("ListSeparator", &[]);
    let callers = crate::api::deprecated::recent_callers(&state.pool).await?;
    for caller in callers.into_iter().filter(|caller| reader.is_none_or(|id| caller.is(id))) {
        let paths = caller.routes.join(&separator);
        let day = crate::services::routing::parse_timestamp(&caller.last_called_at)
            .map(|at| crate::localization::human_date(at, localizer))
            .unwrap_or(caller.last_called_at);
        let said = [("paths", paths.as_str()), ("date", day.as_str())];
        let message = match &caller.application {
            Some(name) => localizer
                .translate("WarnDeprecatedApiCalled", &[said[0], said[1], ("name", name.as_str())]),
            None => localizer.translate("WarnDeprecatedApiCalledPerson", &said),
        };
        warnings.push(Warning::new("deprecated_api_called", message));
    }

    let regions = AppState::certification_regions_from(settings);
    let rating: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, certification_country FROM instances
          WHERE enabled = 1 AND certification_country IS NOT NULL
          ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;
    for (name, country) in rating.into_iter().filter(|(_, country)| !regions.contains(country)) {
        warnings.push(Warning::new(
            "certification_country_outside_regions",
            localizer.translate(
                "WarnCertificationCountry",
                &[("name", &name), ("country", &country), ("regions", &regions.join(", "))],
            ),
        ));
    }

    // Radarr refreshes each film's rating within 180 days of a change of country.
    let changed: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, certification_country FROM instances
          WHERE enabled = 1 AND certification_country IS NOT NULL
            AND certification_country_changed_at > datetime('now', '-180 days')
          ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;
    for (name, country) in changed {
        warnings.push(Warning::new(
            "certification_country_changed",
            localizer.translate(
                "WarnCertificationCountryChanged",
                &[("name", &name), ("country", &country)],
            ),
        ));
    }

    Ok(warnings)
}

/// What is wrong with the metadata configuration, in the user's language. A TMDB key is no
/// requirement: the Arr answers genre, language and certification rules without one.
fn metadata_warnings(state: &AppState, localizer: &Localizer, settings: &Settings) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let order = AppState::metadata_order_from(settings);
    let keys = state.provider_keys_from(settings);

    for provider in &order {
        if provider.needs_key && !metadata::is_usable(provider, &keys) {
            warnings.push(Warning::restating(
                step::METADATA,
                "source_needs_key",
                localizer.translate("WarnProviderNeedsKey", &[("provider", provider.display_name)]),
            ));
        }
    }

    // A key set in the environment for a source the stored list leaves out is never used.
    for provider in metadata::PROVIDERS {
        let listed = order.iter().any(|entry| entry.id == provider.id);
        if let (false, Some(variable)) = (listed, provider.key_env)
            && state.environment_key(provider.id).is_some()
        {
            warnings.push(Warning::restating(
                step::METADATA,
                "source_key_unlisted",
                localizer.translate(
                    "WarnProviderKeyUnlisted",
                    &[
                        ("provider", provider.display_name),
                        ("variable", variable),
                        ("screen", &localizer.translate("MetadataSources", &[])),
                    ],
                ),
            ));
        }
    }

    warnings
}

/// What a probe of one Arr found, or `unchecked` and `disabled`, which no probe asks.
#[derive(Clone)]
struct Reached {
    /// `connected`, `disabled`, `unchecked` or `error`.
    status: &'static str,
    version: Option<String>,
    failure: Option<Failure>,
}

impl Reached {
    fn state(status: &'static str) -> Self {
        Self { status, version: None, failure: None }
    }
}

/// Why a probe of an Arr failed, as `probe_results` keeps it: the answer, which
/// [`Failure::told`] words in the reader's language, or a sentence already worded.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Failure {
    Upstream { status: u16, upstream: crate::error::Upstream, message: String },
    Told { sentence: String },
}

impl Failure {
    fn of(error: &crate::error::AppError) -> Self {
        match error {
            crate::error::AppError::ExternalApi { status, kind, message, .. } => {
                let upstream = kind.clone();
                Failure::Upstream { status: *status, upstream, message: message.clone() }
            }
            other => Failure::Told { sentence: other.public_message() },
        }
    }

    fn stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// A row holding anything else holds a sentence.
    fn read(stored: &str) -> Self {
        serde_json::from_str(stored)
            .unwrap_or_else(|_| Failure::Told { sentence: stored.to_string() })
    }

    /// What to change, when the address or the key explains it, as
    /// `POST /instances/{id}/test` says it.
    fn told(&self, kind: &str, base_url: &str, localizer: &Localizer) -> String {
        match self {
            Failure::Upstream { status, upstream: failed, message } => {
                let error = crate::integrations::upstream(
                    crate::services::connection::service_name(kind),
                    *status,
                    failed.clone(),
                    message.clone(),
                );
                crate::services::connection::explained(error, kind, base_url, localizer)
                    .public_message()
            }
            Failure::Told { sentence } => sentence.clone(),
        }
    }
}

async fn probe_instances(state: &AppState, instances: &[Instance]) -> HashMap<String, Reached> {
    let probes = instances.iter().map(|instance| async move {
        (instance.id.clone(), probe_instance(state, instance).await)
    });
    join_all(probes).await.into_iter().collect()
}

async fn probe_instance(state: &AppState, instance: &Instance) -> Reached {
    if !instance.enabled {
        return Reached::state("disabled");
    }
    let reached = match state.adapter(instance) {
        Ok(adapter) => adapter.test_connection().await.map(|answered| answered.version),
        Err(e) => Err(e),
    };
    match reached {
        Ok(version) => Reached { status: "connected", version: Some(version), failure: None },
        Err(e) => {
            // Any key reads this through `/status`, so an internal failure's text goes to the log.
            if e.is_internal() {
                tracing::warn!(instance = %instance.name, "The probe failed: {e}");
            }
            Reached { status: "error", version: None, failure: Some(Failure::of(&e)) }
        }
    }
}

/// One instance as `/health` answers it, the probe's failure told in the reader's language.
async fn counts_for(
    state: &AppState,
    instance: &Instance,
    reached: Reached,
    localizer: &Localizer,
) -> AppResult<InstanceHealth> {
    let (media_count, mapped_root_folders): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM media WHERE instance_id = ?1),
                (SELECT COUNT(*) FROM root_folders
                  WHERE instance_id = ?1 AND category IS NOT NULL AND category != '')",
    )
    .bind(&instance.id)
    .fetch_one(&state.pool)
    .await?;
    let detail = reached
        .failure
        .as_ref()
        .map(|failure| failure.told(&instance.instance_type, &instance.base_url, localizer));
    let status = if detail.is_some() { "error" } else { reached.status }.to_string();

    Ok(InstanceHealth {
        id: instance.id.clone(),
        name: instance.name.clone(),
        instance_type: instance.instance_type.clone(),
        status,
        detail,
        version: reached.version,
        last_sync: instance.last_sync_at.clone(),
        last_sync_status: crate::models::instance::sync_status_said(
            instance.last_sync_status.clone(),
            localizer,
        ),
        last_sync_error: crate::models::instance::sync_error_said(
            instance.last_sync_status.as_deref(),
            localizer,
        ),
        media_count,
        mapped_root_folders,
        last_webhook_at: instance.last_webhook_at.clone(),
        last_webhook_event: instance.last_webhook_event.clone(),
    })
}

async fn gather_stats(state: &AppState) -> AppResult<AppStats> {
    let open = open_failure();
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT
            (SELECT COUNT(*) FROM instances),
            (SELECT COUNT(*) FROM media),
            (SELECT COUNT(*) FROM media WHERE media_type = 'movie'),
            (SELECT COUNT(*) FROM media WHERE media_type = 'series'),
            (SELECT COUNT(*) FROM rules),
            (SELECT COUNT(*) FROM rules WHERE enabled = 1),
            (SELECT COUNT(*) FROM overrides),
            (SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0),
            (SELECT COUNT(*) FROM decisions WHERE status = 'applied'),
            (SELECT COUNT(*) FROM decisions d WHERE {open}),
            (SELECT COUNT(*) FROM categories c WHERE {UNMAPPED}),
            (SELECT COUNT(*) FROM jobs WHERE status = 'running')"
        )))
        .fetch_one(&state.pool)
        .await?;

    Ok(AppStats {
        total_instances: row.0,
        total_media: row.1,
        total_movies: row.2,
        total_series: row.3,
        total_rules: row.4,
        enabled_rules: row.5,
        total_overrides: row.6,
        pending_decisions: row.7,
        applied_decisions: row.8,
        failed_decisions: row.9,
        unmapped_categories: row.10,
        running_jobs: row.11,
    })
}

/// What the warnings read off the data folder, gathered off the async workers in one go.
#[derive(Default)]
struct OnDisk {
    last_backup: Option<chrono::DateTime<chrono::Utc>>,
    pending_restore: Option<crate::services::backup::PendingRestore>,
    /// The bytes left on the data disk when too few, as [`data_disk_low`] weighs them.
    free_when_low: Option<u64>,
    replaced_bytes: Option<u64>,
}

async fn on_disk(state: &AppState) -> OnDisk {
    let state = state.clone();
    let read = move || OnDisk {
        last_backup: crate::services::backup::last_taken_at(&state),
        pending_restore: crate::services::backup::pending_restore(&state.config),
        free_when_low: data_disk_low(&state.config),
        replaced_bytes: crate::services::backup::replaced_database(&state.config)
            .map(|kept| kept.size_bytes),
    };
    tokio::task::spawn_blocking(read).await.unwrap_or_default()
}

/// No archive for twice the interval since `last`. An installation younger than that has not
/// missed one yet.
async fn backup_overdue(
    state: &AppState,
    last: Option<chrono::DateTime<chrono::Utc>>,
    hours: i64,
) -> AppResult<bool> {
    let limit = chrono::Duration::hours(2 * hours);
    let since = match last {
        Some(taken) => taken,
        None => {
            let installed: Option<String> =
                sqlx::query_scalar("SELECT MIN(applied_at) FROM _migrations")
                    .fetch_one(&state.pool)
                    .await?;
            match installed.as_deref().and_then(crate::services::routing::parse_timestamp) {
                Some(installed) => installed,
                None => return Ok(false),
            }
        }
    };
    Ok(chrono::Utc::now() - since > limit)
}

/// The bytes left on the data folder's disk, when too few for the next backup and the database's
/// growth. `None` on a filesystem that cannot say.
fn data_disk_low(config: &crate::config::Config) -> Option<u64> {
    let free = fs4::available_space(&config.data_dir).ok()?;
    let database = std::fs::metadata(&config.db_path).map(|meta| meta.len()).unwrap_or(0);
    crate::services::backup::disk_is_low(free, database).then_some(free)
}

/// What the low disk warning says, `replaced` the size of the database a restore set aside.
pub(crate) fn disk_low_message(free: u64, replaced: Option<u64>, localizer: &Localizer) -> String {
    let bytes =
        |n: u64| crate::localization::human_bytes(i64::try_from(n).unwrap_or(i64::MAX), localizer);
    match replaced {
        Some(size) => localizer.translate(
            "WarnDataDiskLowReplaced",
            &[("free", &bytes(free)), ("size", &bytes(size))],
        ),
        None => localizer.translate("WarnDataDiskLow", &[("free", &bytes(free))]),
    }
}
