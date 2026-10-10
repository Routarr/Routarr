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
use crate::error::AppResult;
use crate::localization::Localizer;
use crate::models::Instance;
use crate::services::metadata;
use crate::state::{AppState, Settings};

/// What the liveness probe answers.
// No version: the probe needs no key, and `/status` gives the version to one.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Pong {
    /// Always `ok`: an answer at all is the news.
    pub status: &'static str,
}

/// Liveness probe: no database work, no outbound calls, no authentication.
pub async fn ping() -> Json<Pong> {
    Json(Pong { status: "ok" })
}

/// Counts and warnings, read from the database without probing anything.
// The top bar reads this rather than `/health`, which probes every Arr
// instance and would block the whole UI on an unreachable server.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StatusResponse {
    pub version: String,
    /// Whether the global dry run holds, in which case nothing is moved.
    pub dry_run: bool,
    pub running_jobs: i64,
    pub pending_decisions: i64,
    /// Failed moves still asking for attention: each its title's latest
    /// decision, which no later run has settled.
    pub failed_decisions: i64,
    /// Configuration problems detectable without touching the network.
    pub warnings: Vec<Warning>,
}

/// One warning, in the reader's language.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Warning {
    /// What the warning is about, stable across releases and languages:
    /// `api_unauthenticated`, `api_external_auth`, `source_needs_key`,
    /// `source_key_unlisted`, `source_unreachable`, `source_quota_spent`,
    /// `source_rate_limited`, `source_key_refused`,
    /// `instance_unreachable`,
    /// `unmapped_categories`, `no_enabled_instance`, `missing_metadata`,
    /// `scheduler_panicked`, `backup_failed`, `backup_overdue`,
    /// `data_disk_low`, `setting_above_maximum`,
    /// `instance_without_mapping`, `certification_country_outside_regions`,
    /// `certification_country_changed`,
    /// `auto_apply_held`, `arr_below_version`, `oidc_open_to_anyone` or
    /// `backup_passphrase_unreadable`. The list may grow.
    pub code: &'static str,
    /// What it means, in the interface language.
    pub message: String,
    /// `error` for a fault that stops part of Routarr working, `warning` for
    /// a configuration that needs looking at, `info` for a state worth
    /// knowing. An open list. `/health` reads `degraded` while one is `error`.
    pub severity: &'static str,
    /// The getting-started step this warning restates, whose banner says the
    /// same thing while the step is open: `instance`, `categories` or
    /// `metadata`. Null for most.
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

/// How much a warning weighs. A fault stops something Routarr does: an Arr
/// or a source it cannot reach or that refuses its key, a pass that panics,
/// backups that fail or cannot be taken. A choice the operator made, or a
/// fact about the library, is information, and keeps `/health` at `ok`.
fn severity_of(code: &str) -> &'static str {
    match code {
        "instance_unreachable"
        | "source_unreachable"
        | "source_key_refused"
        | "scheduler_panicked"
        | "backup_failed"
        | "backup_passphrase_unreadable"
        | "data_disk_low" => "error",
        "api_external_auth"
        | "source_key_unlisted"
        | "missing_metadata"
        | "certification_country_outside_regions"
        | "certification_country_changed" => "info",
        _ => "warning",
    }
}

/// A failed move still asking for attention, as a condition over `decisions
/// d`: the title's latest standing decision, and no stored run has read the
/// title since. A later proposal, a move that went through, or a run finding
/// the title where it belongs settles it.
fn open_failure() -> String {
    format!(
        "d.status = 'failed' AND {}
         AND NOT EXISTS (SELECT 1 FROM media_routing r
                          WHERE r.media_id = d.media_id AND r.evaluated_at > d.decided_at)",
        crate::api::metrics::LATEST
    )
}

/// Cheap status for the persistent chrome of the UI.
pub async fn status(State(state): State<AppState>) -> AppResult<Json<StatusResponse>> {
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

    // The same list the diagnostics page shows, minus what needs a probe: the
    // badge counts warnings the user will actually find when they click it.
    let warnings = offline_warnings(&state, &localizer, &settings).await?;

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
    /// Every warning standing, those `/status` lists, with what this probe
    /// found.
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
    /// `connected`, `disabled`, `unchecked` when nothing was probed, or
    /// `error: ` and the sentence `detail` holds. The sentence after the
    /// colon is kept for the clients that read it there, and goes in a
    /// coming release: read `detail`.
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
    /// How the last sync ended: `success`, or `error: ` and the reason.
    pub last_sync_status: Option<String>,
    /// The titles the last sync read from it.
    pub media_count: i64,
    /// Its root folders mapped to a category.
    pub mapped_root_folders: i64,
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

/// Whether to reach out to the Arrs and the metadata sources.
///
/// The dashboard asks for `probe=false` and gets what the database can answer
/// immediately. The probe costs a full connect timeout per unreachable Arr,
/// which is exactly when somebody is looking at the dashboard to find out why.
#[derive(Debug, Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HealthQuery {
    /// `false` reads the database alone: every enabled instance reads
    /// `unchecked` and every source `connected: null`, while `warnings` repeat
    /// what the last probe found. Defaults to true, except for a request
    /// another site set off.
    probe: Option<bool>,
    /// `true` probes again even when a probe of the same configuration ran in
    /// the last 30 seconds, whose findings are otherwise answered again.
    fresh: Option<bool>,
}

/// Full diagnostics. Instances are probed concurrently and each probe is bounded
/// by the shared HTTP timeout, so one unreachable Arr does not cost the sum of
/// every connection attempt.
pub async fn health_check(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<HealthQuery>,
) -> AppResult<(StatusCode, Json<HealthResponse>)> {
    // First, and alone: every other figure below is read from the database,
    // and a database that does not answer would otherwise read as a 500 the
    // first of them raises, or as an empty library.
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

    // A page of another site can send a browser here with its cookie, and a
    // probe reaches every Arr and source and writes what it found. Such a
    // request reads what the last probe recorded instead.
    let cross_site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site.eq_ignore_ascii_case("cross-site"));
    let probe = query.probe.unwrap_or(true) && !cross_site;
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
    // "Nothing at all is known about this item": no genres from its Arr and no
    // fetched answer either. An item the `arr` source alone describes is not
    // missing metadata, which is the whole point of that source.
    let known = crate::api::media::metadata_predicate(&settings);
    let media_missing_metadata: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM media m WHERE NOT ({known})"
    )))
    .fetch_one(pool)
    .await?;

    let none = HashMap::new();
    let connectivity = probed.as_ref().map_or(&none, |probe| &probe.sources);
    let providers = provider_health(&state, connectivity, &settings);

    // What a probe found is in `probe_results` by now, so `offline_warnings`
    // is the single source for every warning and not merely for most.
    let warnings = offline_warnings(&state, &localizer, &settings).await?;
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

/// The last probe when it is younger than [`PROBE_REUSE`] and looked at the
/// configuration in place, or a new one, written to `probe_results` before it
/// is answered. One at a time: a caller arriving while a probe runs waits for
/// it and reads it, rather than asking every Arr and source again, at a pace
/// each source keeps for every other request too.
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

/// What a probe depends on: each instance's address, key and type, and every
/// setting, among them the sources, their order and their keys. A probe of
/// another configuration is never answered again: an address just corrected
/// would read as still failing.
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

/// What the last probe found, for the endpoints that may not probe themselves.
///
/// A subject that answered is not reported, and neither is one nothing has
/// looked at yet: the table holds verdicts, not a roster. An instance deleted
/// since the probe leaves a row nothing can name, which is dropped rather than
/// reported as an unnamed failure.
async fn last_probe_warnings(state: &AppState, localizer: &Localizer) -> AppResult<Vec<Warning>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT subject, detail FROM probe_results WHERE reachable = 0 ORDER BY subject",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut warnings = Vec::new();
    for (subject, detail) in rows {
        if let Some(id) = subject.strip_prefix("source:") {
            // Named from the catalogue rather than stored beside the verdict:
            // the display name belongs to the build, not to the observation.
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
            let instance: Option<(String, String, String)> =
                sqlx::query_as("SELECT name, instance_type, base_url FROM instances WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&state.pool)
                    .await?;
            if let Some((name, kind, base_url)) = instance {
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
    Ok(warnings)
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

/// Write down what the probe found, for the endpoints that may not probe.
///
/// Replaces rather than accumulates: the question is what the last look saw,
/// and a source that answers again has to stop being reported. A verdict is
/// only as fresh as the last probe, and the dashboard is the default route, so
/// in practice it is rewritten on every visit, but nothing here expires it,
/// which is why the diagnostics page states its own findings from a live probe
/// rather than from this table.
async fn record_probe(state: &AppState, probe: &Probe) -> AppResult<()> {
    // The sources probed, which leaves out the Arr, reached through the
    // instance probes instead, and a source that could not be built at all:
    // neither was looked at, so neither has a verdict to record.
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
    // A verdict outlives its subject otherwise. A source switched off, or one
    // whose key was cleared, is not probed (`connected` is `None` and nothing
    // above writes a row for it), so its last failure would be reported for
    // ever, with no screen able to clear it.
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

/// Each `probe_results` detail of a source that did not answer, with the
/// warning code and the dictionary key that say it. A source with none of
/// these did not answer at all (`source_unreachable`).
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
    /// Why it did not answer, as `MetadataProviderHealth.reason` names it:
    /// its warning's code without the `source_` the warnings share.
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

    /// The verdict on `outcome`. A refusal names the key only where the
    /// source takes one: a keyless source refusing is one not answering.
    fn of(outcome: &crate::error::AppResult<bool>, needs_key: bool) -> Self {
        use crate::error::AppError;
        let detail = match outcome {
            Ok(true) => return Self { connected: true, detail: None },
            Err(error) if crate::integrations::is_quota_spent(error) => QUOTA_SPENT,
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
}

/// Reachability of every fetched source that is enabled and configured.
///
/// Probed concurrently and bounded by the shared HTTP timeout, like the Arr
/// instances: probed one after another, the sources would make this page as
/// slow as the sum of their timeouts.
async fn probe_sources(state: &AppState) -> HashMap<String, Probed> {
    let sources = state.metadata_sources().await;
    let probes = sources
        .iter()
        .map(|source| async move { (source.id().to_string(), probe_source(state, source).await) });

    join_all(probes).await.into_iter().collect()
}

/// One source's probe, at the pace every other request to the source keeps:
/// unpaced during a pass, it earns the source's 429 and reports it as the
/// source down. A source with a daily quota is asked once a day, the request
/// taken from its quota, and the day's verdict stands until the day ends or a
/// new key or quota is saved, which forgets it (migration 033).
async fn probe_source(state: &AppState, source: &metadata::FetchingSource) -> Probed {
    let needs_key = metadata::info(source.id()).is_some_and(|info| info.needs_key);
    let Some(quota) = source.daily_quota() else {
        return Probed::of(&paced_probe(state, source).await, needs_key);
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
    let outcome = paced_probe(state, source).await;
    if outcome.as_ref().is_err_and(crate::integrations::is_quota_spent)
        && let Err(e) = quota.exhaust(&state.pool).await
    {
        tracing::warn!("Could not record that {} spent its quota: {e}", source.id());
    }
    Probed::of(&outcome, needs_key)
}

/// The source's connection test, waiting on its pace and holding the pace
/// back as the answer asks.
async fn paced_probe(
    state: &AppState,
    source: &metadata::FetchingSource,
) -> crate::error::AppResult<bool> {
    let pace = state.paces.of(source);
    pace.acquire().await;
    let outcome = source.test_connection().await;
    source.paced_after(&pace, &outcome).await;
    if let Err(e) = &outcome {
        tracing::warn!(source = source.id(), "The probe of a metadata source failed: {e}");
    }
    outcome
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
            // `None` for the Arr, which is reached through the instance probes
            // above, and for a source that cannot be built at all.
            connected: connectivity.get(provider.id).map(|probed| probed.connected),
            reason: connectivity.get(provider.id).and_then(Probed::reason),
        })
        .collect()
}

/// "No root folder is mapped to category `c`", for every count of unmapped
/// categories: the badge, the dashboard and the guide read one answer.
pub(crate) const UNMAPPED: &str =
    "NOT EXISTS (SELECT 1 FROM root_folders rf WHERE rf.category = c.name)";

/// Every warning that can be reached without touching the network.
///
/// Shared by the top bar and the diagnostics page: two hand-built lists drift in
/// both directions, and the badge then reads "1 warning" over a page listing
/// three.
///
/// Everything here is a fact about the database or the configuration. The
/// findings that need a probe are written to `probe_results` by `health_check`,
/// the only caller allowed to wait on the network, and read back here.
async fn offline_warnings(
    state: &AppState,
    localizer: &Localizer,
    settings: &Settings,
) -> AppResult<Vec<Warning>> {
    let mut warnings = Vec::new();

    // `external` is a decision, not an omission, so it is stated as its own
    // line: telling an operator who put Authelia in front that their API is
    // unauthenticated is how a diagnostic gets ignored.
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
    warnings.extend(last_probe_warnings(state, localizer).await?);

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

    let held = *state.auto_apply_held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((candidates, cap)) = held {
        warnings.push(Warning::new(
            "auto_apply_held",
            localizer.translate(
                "WarnAutoApplyHeld",
                &[("count", &candidates.to_string()), ("limit", &cap.to_string())],
            ),
        ));
    }

    // The same predicate the library column and the diagnostics count splice:
    // three spellings of one question is how a badge ends up contradicting the
    // number above it.
    let known = crate::api::media::metadata_predicate(&state.settings().await);
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

    // An unattended pass that panicked. The loop catches it and carries on
    // (stopping would silently end every sync, backup and purge), but carrying
    // on quietly is its own failure, because nothing gives an operator a reason
    // to open the log of an application that looks well. Counted over 24 hours
    // rather than since startup: what matters is whether it is still happening,
    // and a restart must not be a way of hiding it.
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

    // A backup that fails does so again at every interval, and nothing else
    // shows it until the day an archive is needed.
    if state.bool_setting("backup_enabled").await {
        let last: Option<String> = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE kind = 'backup'
              ORDER BY started_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_optional(&state.pool)
        .await?;
        let hours: i64 = state.setting::<i64>("backup_interval_hours").await.clamp(1, 24 * 7);
        if last.as_deref() == Some("failed") {
            warnings
                .push(Warning::new("backup_failed", localizer.translate("WarnBackupFailed", &[])));
        } else if backup_overdue(state, hours).await? {
            let message = localizer.translate("WarnBackupOverdue", &[]);
            warnings.push(Warning::new("backup_overdue", message));
        }
    }
    if let Some(free) = data_disk_low(&state.config) {
        let free =
            crate::localization::human_bytes(i64::try_from(free).unwrap_or(i64::MAX), localizer);
        warnings.push(Warning::new(
            "data_disk_low",
            localizer.translate("WarnDataDiskLow", &[("free", &free)]),
        ));
    }

    // A retention count stored above its ceiling is honoured as it is:
    // lowering it removes what is beyond it, and nothing but the operator's
    // own save may do that. Named here so the operator is the one who lowers
    // it: the screen refuses to save it as it stands, and this says why.
    for (key, max) in crate::services::settings::retention_counts() {
        let stored: i64 = settings.get(key);
        if stored > max {
            warnings.push(Warning::new(
                "setting_above_maximum",
                localizer.translate(
                    "WarnSettingAboveMaximum",
                    &[("key", key), ("value", &stored.to_string()), ("max", &max.to_string())],
                ),
            ));
        }
    }

    // A passphrase the master key in place cannot open takes no backup at all,
    // rather than one in the clear, while the settings still say one is set.
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

    // An enabled instance with nothing mapped routes nowhere. Reported whether
    // or not it answers: being unreachable does not make the mapping appear,
    // and the two are separate things to fix.
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

    // An Arr rating for a country outside the certification regions: the
    // regions rank every source's rating by the country it belongs to, and
    // this one's then counts only after every rating inside them.
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

    // A Radarr whose rating country changed: the films it rated before keep
    // the previous country's ratings until each is refreshed, which it does on
    // its own within 180 days.
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

/// What is wrong with the metadata configuration, in the user's language.
///
/// Deliberately not "no TMDB key": with the Arr enabled, genre, language and
/// certification rules match perfectly well without one. Only keywords and
/// origin countries do not.
fn metadata_warnings(state: &AppState, localizer: &Localizer, settings: &Settings) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let order = AppState::metadata_order_from(settings);
    // A source counts as configured whether its key came from the interface
    // or the environment.
    let keys = state.provider_keys_from(settings);

    // One line per source that could answer and cannot: which key is missing
    // is the one thing the user can act on.
    for provider in &order {
        if provider.needs_key && !metadata::is_usable(provider, &keys) {
            warnings.push(Warning::restating(
                step::METADATA,
                "source_needs_key",
                localizer.translate("WarnProviderNeedsKey", &[("provider", provider.display_name)]),
            ));
        }
    }

    // A key set in the environment for a source the list leaves out. The start
    // lists TMDB for its key only while no list is stored, and any save of the
    // Settings screen stores one, so the key would otherwise be read and never
    // used, with nothing saying so.
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

/// What a probe of one Arr found, or `unchecked` and `disabled`, which no
/// probe asks.
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

/// Why a probe of an Arr failed, as `probe_results` keeps it: what the Arr
/// or the network said, which [`Failure::told`] turns into what to change in
/// the language of whoever reads it, or a sentence already told.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Failure {
    Upstream { status: u16, message: String },
    Told { sentence: String },
}

impl Failure {
    fn of(error: &crate::error::AppError) -> Self {
        match error {
            crate::error::AppError::ExternalApi { status, message, .. } => {
                Failure::Upstream { status: *status, message: message.clone() }
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
            Failure::Upstream { status, message } => {
                let error = crate::error::AppError::ExternalApi {
                    service: crate::services::connection::service_name(kind).to_string(),
                    status: *status,
                    message: message.clone(),
                    retry_after: None,
                };
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
            // Any key reads what a probe found, through `/status`: an internal
            // failure reads as one generic sentence, and goes to the log.
            if e.is_internal() {
                tracing::warn!(instance = %instance.name, "The probe failed: {e}");
            }
            Reached { status: "error", version: None, failure: Some(Failure::of(&e)) }
        }
    }
}

/// One instance as `/health` answers it: what the database counts, and what
/// the probe found, told in the reader's language.
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
    let status = match &detail {
        Some(detail) => format!("error: {detail}"),
        None => reached.status.to_string(),
    };

    Ok(InstanceHealth {
        id: instance.id.clone(),
        name: instance.name.clone(),
        instance_type: instance.instance_type.clone(),
        status,
        detail,
        version: reached.version,
        last_sync: instance.last_sync_at.clone(),
        last_sync_status: instance.last_sync_status.clone(),
        media_count,
        mapped_root_folders,
    })
}

/// All counters in one round trip instead of a dozen sequential `COUNT(*)`s.
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

/// No archive for twice the interval: the schedule stopped, or every attempt
/// since vanished from the Tasks screen. An installation younger than that
/// has not missed one yet.
async fn backup_overdue(state: &AppState, hours: i64) -> AppResult<bool> {
    let limit = chrono::Duration::hours(2 * hours);
    let since = match crate::services::backup::last_taken_at(state) {
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

/// The bytes left on the disk of the data folder, when fewer than the next
/// backup and the database's own growth need: twice the database, and room
/// to spare. Unknown on a filesystem that cannot say, which warns of nothing.
fn data_disk_low(config: &crate::config::Config) -> Option<u64> {
    let free = fs4::available_space(&config.data_dir).ok()?;
    let database = std::fs::metadata(&config.db_path).map(|meta| meta.len()).unwrap_or(0);
    disk_is_low(free, database).then_some(free)
}

/// Fewer bytes free than twice the database and 100 MiB.
pub(crate) fn disk_is_low(free: u64, database: u64) -> bool {
    free < database.saturating_mul(2).saturating_add(100 * 1024 * 1024)
}
