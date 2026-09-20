//! Health and diagnostics.

use super::Json;
use axum::extract::{Query, State};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use sqlx::AssertSqlSafe;

use crate::error::AppResult;
use crate::localization::Localizer;
use crate::models::Instance;
use crate::services::metadata;
use crate::state::{AppState, Settings};

/// Liveness probe: no database work, no outbound calls, no authentication.
pub async fn ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Everything the app shell needs, with no outbound calls.
///
/// The top bar reads this rather than `/health`, which probes every Arr
/// instance and would block the whole UI on an unreachable server.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub version: String,
    pub dry_run: bool,
    pub running_jobs: i64,
    pub pending_decisions: i64,
    pub failed_decisions: i64,
    /// Configuration problems detectable without touching the network.
    pub warnings: Vec<String>,
}

/// Cheap status for the persistent chrome of the UI.
pub async fn status(State(state): State<AppState>) -> AppResult<Json<StatusResponse>> {
    let settings = state.settings().await;
    let localizer = Localizer::new(&AppState::language_from(&settings));
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM jobs WHERE status = 'running'),
            (SELECT COUNT(*) FROM decisions WHERE status = 'pending' AND superseded = 0),
            (SELECT COUNT(*) FROM decisions WHERE status = 'failed')",
    )
    .fetch_one(&state.pool)
    .await?;

    // The same list the diagnostics page shows, minus what needs a probe: the
    // badge counts warnings the user will actually find when they click it.
    let warnings = offline_warnings(&state, &localizer, &settings).await?;

    Ok(Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        dry_run: settings.bool("global_dry_run", true),
        running_jobs: row.0,
        pending_decisions: row.1,
        failed_decisions: row.2,
        warnings,
    }))
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub database: String,
    pub instances: Vec<InstanceHealth>,
    pub metadata: MetadataHealth,
    pub stats: AppStats,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InstanceHealth {
    pub id: String,
    pub name: String,
    pub instance_type: String,
    pub status: String,
    pub version: Option<String>,
    pub last_sync: Option<String>,
    pub last_sync_status: Option<String>,
    pub media_count: i64,
    pub mapped_root_folders: i64,
}

#[derive(Debug, Serialize)]
pub struct MetadataHealth {
    /// Every source in the user's order, whether or not it can answer.
    pub providers: Vec<MetadataProviderHealth>,
    pub cached_items: i64,
    /// Items no source describes at all.
    pub media_missing_metadata: i64,
}

#[derive(Debug, Serialize)]
pub struct MetadataProviderHealth {
    pub id: String,
    pub display_name: String,
    pub needs_key: bool,
    /// A source that needs a key and has one, or that needs none.
    pub configured: bool,
    /// Probed only for a fetched, configured source; `None` for the Arr, which
    /// is reached through the instance probes above.
    pub connected: Option<bool>,
}

#[derive(Debug, Serialize)]
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
    pub failed_decisions: i64,
    pub unmapped_categories: i64,
    pub running_jobs: i64,
}

/// Whether to reach out to the Arrs and the metadata sources.
///
/// The dashboard asks for `probe=false` and gets what the database can answer
/// immediately; the probe costs a full connect timeout per unreachable Arr,
/// which is exactly when somebody is looking at the dashboard to find out why.
#[derive(Debug, Deserialize, Default)]
pub struct HealthQuery {
    probe: Option<bool>,
}

/// Full diagnostics. Instances are probed concurrently and each probe is bounded
/// by the shared HTTP timeout, so one unreachable Arr does not cost the sum of
/// every connection attempt.
pub async fn health_check(
    State(state): State<AppState>,
    Query(query): Query<HealthQuery>,
) -> AppResult<Json<HealthResponse>> {
    let probe = query.probe.unwrap_or(true);
    let pool = &state.pool;
    let settings = state.settings().await;
    let localizer = Localizer::new(&AppState::language_from(&settings));

    let database = match sqlx::query("SELECT 1").execute(pool).await {
        Ok(_) => "connected",
        Err(_) => "error",
    };

    let instances = state.instances(false).await?;

    let (instance_health, connectivity) = if probe {
        futures::join!(probe_instances(&state, &instances), probe_sources(&state))
    } else {
        (describe_instances(&state, &instances).await, HashMap::new())
    };

    let stats = gather_stats(&state).await?;

    let cached_items: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM metadata_cache").fetch_one(pool).await?;
    // "Nothing at all is known about this item": no genres from its Arr and no
    // fetched answer either. An item the `arr` source alone describes is not
    // missing metadata any more, which is the whole point of that source.
    let known = crate::api::media::metadata_predicate(&state.metadata_order().await);
    let media_missing_metadata: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM media m WHERE NOT ({known})"
    )))
    .fetch_one(pool)
    .await?;

    let providers = provider_health(&state, &connectivity, &settings);

    // A probe writes down what it found before anything is reported, so the
    // two findings it alone can make survive into the answer `/status` gives.
    // That endpoint is polled and must never probe: an unreachable host costs a
    // full connect timeout. Without this the dashboard reported a source that
    // had stopped answering while the navigation beside it, unable to know,
    // counted zero.
    if probe {
        record_probe(&state, &providers, &instance_health).await?;
    }

    // One list, produced in one place. What a probe learned is in the table by
    // now, so `offline_warnings` is the single source for all of them and not
    // merely for most.
    let warnings = offline_warnings(&state, &localizer, &settings).await?;

    let status = if database == "connected" && warnings.is_empty() { "ok" } else { "degraded" };

    Ok(Json(HealthResponse {
        status: status.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        database: database.to_string(),
        instances: instance_health,
        metadata: MetadataHealth { providers, cached_items, media_missing_metadata },
        stats,
        warnings,
    }))
}

/// What the last probe found, for the endpoints that may not probe themselves.
///
/// A subject that answered is not reported, and neither is one nothing has
/// looked at yet: the table holds verdicts, not a roster. An instance deleted
/// since the probe leaves a row nothing can name, which is dropped rather than
/// reported as an unnamed failure.
async fn last_probe_warnings(state: &AppState, localizer: &Localizer) -> AppResult<Vec<String>> {
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
                warnings.push(
                    localizer
                        .translate("WarnProviderUnreachable", &[("provider", info.display_name)]),
                );
            }
        } else if let Some(id) = subject.strip_prefix("instance:") {
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM instances WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&state.pool)
                    .await?;
            if let Some(name) = name {
                warnings.push(localizer.translate(
                    "WarnInstanceUnreachable",
                    &[("name", &name), ("status", detail.as_deref().unwrap_or(""))],
                ));
            }
        }
    }
    Ok(warnings)
}

/// Write down what the probe found, for the endpoints that may not probe.
///
/// Replaces rather than accumulates: the question is what the last look saw,
/// and a source that answers again has to stop being reported. A verdict is
/// only as fresh as the last probe, and the dashboard is the default route, so
/// in practice it is rewritten on every visit — but nothing here expires it,
/// which is why the diagnostics page states its own findings from a live probe
/// rather than from this table.
async fn record_probe(
    state: &AppState,
    providers: &[MetadataProviderHealth],
    instances: &[InstanceHealth],
) -> AppResult<()> {
    let rows: Vec<(String, bool, Option<String>)> = providers
        .iter()
        // `None` is the Arr, reached through the instance probes instead, and a
        // source that could not be built at all: neither was looked at, so
        // neither has a verdict to record.
        .filter_map(|p| p.connected.map(|ok| (format!("source:{}", p.id), ok, None)))
        .chain(instances.iter().map(|i| {
            let ok = !i.status.starts_with("error");
            (format!("instance:{}", i.id), ok, (!ok).then(|| i.status.clone()))
        }))
        .collect();

    let mut tx = state.pool.begin().await?;
    // A verdict outlives its subject otherwise. A source switched off, or one
    // whose key was cleared, is no longer probed — `connected` is `None` and
    // nothing above writes a row for it — so its last failure would be reported
    // for ever, with no screen able to clear it.
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

/// Reachability of every fetched source that is enabled and configured.
///
/// Probed concurrently and bounded by the shared HTTP timeout, like the Arr
/// instances: five sources probed one after another would make this page as
/// slow as the slowest sum, which is what the instance probes were fixed for.
async fn probe_sources(state: &AppState) -> HashMap<String, bool> {
    let sources = state.metadata_sources().await;
    let probes = sources.iter().map(|source| async move {
        (source.id().to_string(), source.test_connection().await.unwrap_or(false))
    });

    join_all(probes).await.into_iter().collect()
}

/// The configured order, annotated with what each source can do right now.
fn provider_health(
    state: &AppState,
    connectivity: &HashMap<String, bool>,
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
            connected: connectivity.get(provider.id).copied(),
        })
        .collect()
}

/// Every warning that can be reached without touching the network.
///
/// Shared by the top bar and the diagnostics page: two hand-built lists drift in
/// both directions, and the badge then reads "1 warning" over a page listing
/// three.
///
/// Everything here is a fact about the database or the configuration. The two
/// findings that need a probe are added by `health_check`, the only caller
/// allowed to wait on the network.
async fn offline_warnings(
    state: &AppState,
    localizer: &Localizer,
    settings: &Settings,
) -> AppResult<Vec<String>> {
    let mut warnings = Vec::new();

    // `external` is a decision, not an omission, so it is stated as its own
    // line: telling an operator who put Authelia in front that their API is
    // unauthenticated is how a diagnostic gets ignored.
    match state.config.auth_mode {
        crate::config::AuthMode::None => {
            warnings.push(localizer.translate("WarnApiUnauthenticated", &[]));
        }
        crate::config::AuthMode::External => {
            warnings.push(localizer.translate("WarnApiExternalAuth", &[]));
        }
        crate::config::AuthMode::ApiKey
        | crate::config::AuthMode::Forms
        | crate::config::AuthMode::Oidc => {}
    }

    warnings.extend(metadata_warnings(state, localizer, settings));
    warnings.extend(last_probe_warnings(state, localizer).await?);

    // The same predicate the library column and the diagnostics count splice:
    // three spellings of one question is how a badge ends up contradicting the
    // number above it.
    let known = crate::api::media::metadata_predicate(&state.metadata_order().await);
    let row: (i64, i64, i64) = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT
            (SELECT COUNT(*) FROM categories c WHERE NOT EXISTS
                (SELECT 1 FROM root_folders rf WHERE rf.category = c.name)),
            (SELECT COUNT(*) FROM instances WHERE enabled = 1),
            (SELECT COUNT(*) FROM media m WHERE NOT ({known}))"
    )))
    .fetch_one(&state.pool)
    .await?;

    if row.0 > 0 {
        warnings
            .push(localizer.translate("WarnUnmappedCategories", &[("count", &row.0.to_string())]));
    }
    if row.1 == 0 {
        warnings.push(localizer.translate("WarnNoEnabledInstance", &[]));
    }
    if row.2 > 0 {
        warnings.push(localizer.translate("WarnMissingMetadata", &[("count", &row.2.to_string())]));
    }

    // An unattended pass that panicked. The loop catches it and carries on —
    // stopping would silently end every sync, backup and purge — but carrying
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
        warnings.push(
            localizer.translate("WarnSchedulerPanicked", &[("count", &panicked.to_string())]),
        );
    }

    // A retention count stored above its ceiling is honoured as it is:
    // lowering it removes what is beyond it, and nothing but the operator's
    // own save may do that. Named here so the operator is the one who lowers
    // it — the screen refuses to save it as it stands, and this says why.
    for (key, max) in crate::api::settings::retention_counts() {
        let stored: i64 = settings.get(key, 0i64);
        if stored > max {
            warnings.push(localizer.translate(
                "WarnSettingAboveMaximum",
                &[("key", key), ("value", &stored.to_string()), ("max", &max.to_string())],
            ));
        }
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
        warnings.push(localizer.translate("WarnInstanceNoMapping", &[("name", &name)]));
    }

    Ok(warnings)
}

/// What is wrong with the metadata configuration, in the user's language.
///
/// Deliberately not "no TMDb key": with the Arr enabled, genre, language and
/// certification rules match perfectly well without one — only keywords and
/// origin countries do not.
fn metadata_warnings(state: &AppState, localizer: &Localizer, settings: &Settings) -> Vec<String> {
    let mut warnings = Vec::new();
    let order = AppState::metadata_order_from(settings);
    // A source counts as configured whether its key came from the interface
    // or the environment.
    let keys = state.provider_keys_from(settings);

    // One line per source that could answer and cannot: which key is missing
    // is the one thing the user can act on.
    for provider in order {
        if provider.needs_key && !metadata::is_usable(provider, &keys) {
            warnings.push(
                localizer.translate("WarnProviderNeedsKey", &[("provider", provider.display_name)]),
            );
        }
    }

    warnings
}

/// The same rows without the network call.
///
/// `status` is `unchecked` rather than a guess. Reporting the last sync's
/// outcome here would read as a live connection state and be wrong the moment
/// an Arr goes down between two syncs; saying nothing is the honest answer, and
/// the interface asks for the real one in the background.
async fn describe_instances(state: &AppState, instances: &[Instance]) -> Vec<InstanceHealth> {
    join_all(instances.iter().map(|instance| describe_instance(state, instance))).await
}

async fn describe_instance(state: &AppState, instance: &Instance) -> InstanceHealth {
    let mut health = counts_for(state, instance).await;
    health.status = if instance.enabled { "unchecked".into() } else { "disabled".into() };
    health
}

async fn probe_instances(state: &AppState, instances: &[Instance]) -> Vec<InstanceHealth> {
    join_all(instances.iter().map(|instance| probe_instance(state, instance))).await
}

/// The counts a database can answer for one instance. Both paths want these;
/// only the probe adds a network call on top.
async fn counts_for(state: &AppState, instance: &Instance) -> InstanceHealth {
    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE instance_id = ?")
        .bind(&instance.id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let mapped_root_folders: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_folders WHERE instance_id = ? AND category IS NOT NULL AND category != ''",
    )
    .bind(&instance.id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    InstanceHealth {
        id: instance.id.clone(),
        name: instance.name.clone(),
        instance_type: instance.instance_type.clone(),
        status: String::new(),
        version: None,
        last_sync: instance.last_sync_at.clone(),
        last_sync_status: instance.last_sync_status.clone(),
        media_count,
        mapped_root_folders,
    }
}

async fn probe_instance(state: &AppState, instance: &Instance) -> InstanceHealth {
    let mut health = counts_for(state, instance).await;

    let (status, version) = if !instance.enabled {
        ("disabled".to_string(), None)
    } else {
        match state.adapter(instance) {
            Ok(adapter) => match adapter.test_connection().await {
                Ok(s) => ("connected".to_string(), Some(s.version)),
                Err(e) => (format!("error: {e}"), None),
            },
            Err(e) => (format!("error: {e}"), None),
        }
    };

    health.status = status;
    health.version = version;
    health
}

/// All counters in one round trip instead of a dozen sequential `COUNT(*)`s.
async fn gather_stats(state: &AppState) -> AppResult<AppStats> {
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
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
            (SELECT COUNT(*) FROM decisions WHERE status = 'failed'),
            (SELECT COUNT(*) FROM categories c WHERE NOT EXISTS
                (SELECT 1 FROM root_folders rf WHERE rf.category = c.name)),
            (SELECT COUNT(*) FROM jobs WHERE status = 'running')",
    )
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
