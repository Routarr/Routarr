//! Metadata enrichment with a bounded-concurrency worker pool.
//!
//! The whole backlog is drained in one pass, several requests in flight, with a
//! back-off when a provider rate-limits. A per-tick cap with sequential calls
//! would leave a large library hours away from being classifiable.
//!
//! Only *fetched* sources come through here: `arr` is enriched by the library
//! sync itself, which is the point of it.

use chrono::Utc;
use futures::stream::{self, StreamExt};
use sqlx::{AssertSqlSafe, SqlitePool};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};
use crate::jobs::{JobHandle, JobKind};
use crate::models::ProviderMetadata;
use crate::services::metadata::{self, Addressing, FetchingSource};
use crate::services::rate_limit::RateLimiter;
use crate::state::AppState;

/// Outcome of an enrichment pass.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct EnrichmentReport {
    pub considered: usize,
    pub enriched: usize,
    pub failed: usize,
    /// Items never asked about because the source was declared down mid-pass.
    pub skipped: usize,
}

/// Enrich every media item whose metadata is missing or expired, source by
/// source, in the user's priority order.
pub async fn enrich_all_media(state: &AppState, trigger: &str) -> AppResult<EnrichmentReport> {
    let sources = state.metadata_sources().await;
    if sources.is_empty() {
        debug!("No fetched metadata source is enabled, skipping enrichment");
        return Ok(EnrichmentReport::default());
    }

    let Some(_lock) = state.jobs.try_lock("enrich") else {
        return Err(AppError::Conflict("An enrichment pass is already running".into()));
    };

    let job = state.jobs.start(JobKind::Enrich, trigger, None, "Enriching metadata").await?;

    let mut report = EnrichmentReport::default();
    let mut outcome = Ok(());
    for source in &sources {
        match run_enrichment(state, source, &job).await {
            Ok(partial) => {
                report.considered += partial.considered;
                report.enriched += partial.enriched;
                report.failed += partial.failed;
                report.skipped += partial.skipped;
            }
            // One unreachable source must not cancel the ones below it: that is
            // exactly the case the ordered list exists to survive.
            Err(e) => {
                warn!("Source '{}' failed: {e}", source.id());
                outcome = Err(e);
            }
        }
    }

    match &outcome {
        Ok(()) => {
            job.succeed(&format!("{} enriched, {} failed", report.enriched, report.failed)).await
        }
        Err(e) => job.fail(&e.to_string()).await,
    }

    outcome.map(|()| report)
}

async fn run_enrichment(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
) -> AppResult<EnrichmentReport> {
    // A source that knows none of our identifiers has to find its own first.
    // The answers are permanent — including the ones that found nothing — so
    // this is a first-pass cost, not a per-run one.
    let limiter = source.limiter();

    if source.addressing() == Addressing::Search {
        resolve_identifiers(state, source, job, &limiter).await?;
    }

    let targets = pending_targets(&state.pool, source).await?;
    if targets.is_empty() {
        info!("The {} cache is up to date", source.id());
        return Ok(EnrichmentReport::default());
    }

    info!("{} item(s) need enrichment from {}", targets.len(), source.id());
    let ttl_days: i64 = state.setting("metadata_cache_ttl_days", 7).await;
    let total = targets.len();

    // `buffer_unordered` keeps N requests in flight without spawning a task per
    // item; the providers tolerate a handful of parallel calls and this is the
    // main driver of how fast a cold library becomes classifiable.
    //
    // Each future carries its own (external_id, media_type) back out: results
    // arrive in completion order, not input order, so pairing them with the
    // input list positionally would file a movie's metadata under a series and
    // lose both.
    let breaker = Breaker::new();

    let results: Vec<(String, String, Option<AppResult<ProviderMetadata>>)> = stream::iter(targets)
        .map(|(external_id, media_type)| {
            let source = source.clone();
            let breaker = breaker.clone();
            let limiter = limiter.clone();
            async move {
                // `buffer_unordered` has already been handed every item, so the
                // breaker is checked here, as each future starts: the ones still
                // queued turn into no-ops instead of requests.
                if breaker.is_open() {
                    return (external_id, media_type, None);
                }

                // Paced before the request, not after a refusal: the point is
                // for the 429 never to be earned.
                limiter.acquire().await;
                let outcome = source.fetch(&external_id, &media_type).await;
                honour_retry_after(&limiter, &outcome).await;
                breaker.record(&outcome);
                (external_id, media_type, Some(outcome))
            }
        })
        .buffer_unordered(source.concurrency(state.config.metadata_concurrency))
        .collect()
        .await;

    let mut report = EnrichmentReport { considered: total, ..Default::default() };
    let mut rate_limited = false;

    for (index, (external_id, media_type, result)) in results.into_iter().enumerate() {
        let Some(result) = result else {
            report.skipped += 1;
            continue;
        };

        match result {
            Ok(data) => {
                store_metadata(
                    &state.pool,
                    source.id(),
                    &external_id,
                    &media_type,
                    &data,
                    ttl_days,
                )
                .await?;
                report.enriched += 1;
            }
            Err(AppError::ExternalApi { status: 429, .. }) => {
                rate_limited = true;
                report.failed += 1;
            }
            Err(e) => {
                warn!("Failed to enrich {} {external_id} ({media_type}): {e}", source.id());
                report.failed += 1;
            }
        }

        // Progress is only interesting at a coarse grain; one UPDATE per item
        // would cost more than the work it reports on.
        if index % 25 == 0 {
            job.progress(index + 1, total).await;
        }
    }

    job.progress(total, total).await;

    if rate_limited {
        warn!(
            "{} rate-limited part of this pass. The remainder is retried on the next run",
            source.id()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    if report.skipped > 0 {
        warn!(
            "{} looks unavailable: gave up after {} failures and skipped {} item(s), \
             which the next pass retries",
            source.id(),
            Breaker::LIMIT,
            report.skipped
        );
    }

    info!(
        "{} item(s) enriched, {} failed, {} skipped",
        report.enriched, report.failed, report.skipped
    );
    Ok(report)
}

/// Stops a pass from hammering a source that is plainly down.
///
/// Shared by both stages that issue requests — resolution and fetching — because
/// a search storm against an unavailable source costs exactly as much as a fetch
/// storm, and for a searching source it is the one that happens first.
#[derive(Clone)]
struct Breaker {
    failures: Arc<AtomicUsize>,
}

impl Breaker {
    /// A source that has refused this many times in one pass is down, not
    /// unlucky.
    const LIMIT: usize = 5;

    fn new() -> Self {
        Self { failures: Arc::new(AtomicUsize::new(0)) }
    }

    fn is_open(&self) -> bool {
        self.failures.load(Ordering::Relaxed) >= Self::LIMIT
    }

    /// Record an outcome, counting only the failures that say something about
    /// the *source* rather than about one item.
    fn record<T>(&self, outcome: &AppResult<T>) {
        if is_source_level_failure(outcome) {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Hold the limiter back when a source states how long it wants to be left
/// alone.
///
/// Pacing is a guess about someone else's limit; `Retry-After` is that someone
/// telling us. When it arrives, the rest of the pass slows to match instead of
/// spending its remaining breaker budget discovering the same thing four more
/// times.
async fn honour_retry_after<T>(limiter: &RateLimiter, outcome: &AppResult<T>) {
    if let Err(AppError::ExternalApi { retry_after: Some(seconds), service, .. }) = outcome {
        warn!(
            "{service} asked for {seconds}s before the next request, pacing the rest of the pass"
        );
        limiter.penalise(Duration::from_secs(*seconds)).await;
    }
}

/// Whether a failure is the *source* being unavailable rather than one item
/// being unknown to it.
///
/// A 404 means "this source does not have that item" and says nothing about the
/// next one. A 429, a 5xx or a transport failure means asking again right now is
/// pointless — those are the ones that trip the breaker.
fn is_source_level_failure<T>(outcome: &AppResult<T>) -> bool {
    matches!(
        outcome,
        Err(AppError::ExternalApi { status: 429 | 500 | 502..=504, .. })
            | Err(AppError::ExternalApi { status: 0, .. })
    )
}

/// The outcome of one resolution attempt: its key, its media type, and either
/// what the source answered or `None` for an attempt the breaker cut off.
type Resolution = (String, String, Option<AppResult<Option<String>>>);

/// One media row, reduced to what identifying it needs.
type Candidate = (String, Option<i64>, String, Option<i64>, Option<i64>, Option<String>);

/// Find this source's identifier for every item it has never been asked about.
///
/// Both outcomes are written down. Remembering that AniList has nothing for
/// *The Matrix* is what stops the next pass — and every pass after it — from
/// searching for it again.
async fn resolve_identifiers(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
    limiter: &RateLimiter,
) -> AppResult<()> {
    let known = metadata::resolved_keys(&state.pool, source.id()).await?;

    let rows: Vec<Candidate> =
        sqlx::query_as("SELECT title, year, media_type, tmdb_id, tvdb_id, imdb_id FROM media")
            .fetch_all(&state.pool)
            .await?;

    // Deduplicated by local key: the same film in two Radarr instances is one
    // search, not two.
    let mut pending: Vec<(String, String, String, Option<i64>)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (title, year, media_type, tmdb_id, tvdb_id, imdb_id) in rows {
        let key = metadata::local_key_of(tmdb_id, tvdb_id, imdb_id.as_deref(), &title, year);
        let dedup = metadata::resolution_key(&media_type, &key);
        if known.contains(&dedup) || !seen.insert(dedup) {
            continue;
        }
        pending.push((key, media_type, title, year));
    }

    if pending.is_empty() {
        return Ok(());
    }

    let total = pending.len();
    info!("{total} item(s) still to identify on {}", source.id());
    job.progress(0, total).await;

    let breaker = Breaker::new();

    let results: Vec<Resolution> = stream::iter(pending)
        .map(|(key, media_type, title, year)| {
            let source = source.clone();
            let breaker = breaker.clone();
            let limiter = limiter.clone();
            async move {
                // The stage where an unavailable source hurts most: one search
                // per *unresolved item*, and nothing is written down, so the
                // whole storm repeats on the next pass.
                if breaker.is_open() {
                    return (key, media_type, None);
                }

                limiter.acquire().await;
                let outcome = source.resolve(&title, year, &media_type).await;
                honour_retry_after(&limiter, &outcome).await;
                breaker.record(&outcome);
                (key, media_type, Some(outcome))
            }
        })
        .buffer_unordered(source.concurrency(state.config.metadata_concurrency))
        .collect()
        .await;

    let mut abandoned = 0usize;
    for (index, (key, media_type, outcome)) in results.into_iter().enumerate() {
        let Some(outcome) = outcome else {
            abandoned += 1;
            continue;
        };

        match outcome {
            Ok(external_id) => {
                metadata::remember_identifier(
                    &state.pool,
                    source.id(),
                    &media_type,
                    &key,
                    external_id.as_deref(),
                )
                .await?;
            }
            // A failed search is *not* written down: it means the network
            // failed, not that the source has nothing, and the difference
            // decides whether the next pass ever tries again.
            Err(e) => warn!("Could not identify {key} on {}: {e}", source.id()),
        }

        if index % 25 == 0 {
            job.progress(index + 1, total).await;
        }
    }

    if abandoned > 0 {
        warn!(
            "{} looks unavailable: gave up identifying {abandoned} item(s), retried next pass",
            source.id()
        );
    }

    Ok(())
}

/// Distinct identifiers, in this source's own namespace, whose cache entry is
/// missing or stale.
///
/// Deduplicating here matters: the same film present in two Radarr instances
/// would otherwise be fetched twice.
async fn pending_targets(
    pool: &SqlitePool,
    source: &FetchingSource,
) -> AppResult<Vec<(String, String)>> {
    match source.addressing() {
        // Not fetched at all.
        Addressing::Local => Ok(Vec::new()),

        // `column` is a literal from `services::metadata`, never user input.
        Addressing::Column(column) => Ok(sqlx::query_as(AssertSqlSafe(format!(
            "SELECT DISTINCT CAST(m.{column} AS TEXT), m.media_type FROM media m
             LEFT JOIN metadata_cache c
                    ON c.source = ?
                   AND c.external_id = CAST(m.{column} AS TEXT)
                   AND c.media_type = m.media_type
             WHERE m.{column} IS NOT NULL AND CAST(m.{column} AS TEXT) != ''
               AND (c.external_id IS NULL OR c.expires_at < datetime('now'))"
        )))
        .bind(source.id())
        .fetch_all(pool)
        .await?),

        // The identifiers were resolved just before this. A row whose
        // `external_id` is NULL means the source was asked and had nothing, so
        // it is not asked again.
        Addressing::Search => Ok(sqlx::query_as(
            "SELECT DISTINCT s.external_id, s.media_type FROM source_identifiers s
             LEFT JOIN metadata_cache c
                    ON c.source = s.source
                   AND c.external_id = s.external_id
                   AND c.media_type = s.media_type
             WHERE s.source = ? AND s.external_id IS NOT NULL
               AND (c.external_id IS NULL OR c.expires_at < datetime('now'))",
        )
        .bind(source.id())
        .fetch_all(pool)
        .await?),
    }
}

/// Enrich one specific item, used by the webhook path.
pub async fn enrich_one(state: &AppState, tmdb_id: i64, media_type: &str) -> AppResult<()> {
    let ttl_days: i64 = state.setting("metadata_cache_ttl_days", 7).await;

    for source in state.metadata_sources().await {
        // The webhook only carries a TMDb id, so a source addressed by anything
        // else — another id, or a search — is left to the next full pass rather
        // than guessed at.
        if source.addressing() != Addressing::Column("tmdb_id") {
            continue;
        }
        let external_id = tmdb_id.to_string();
        let data = source.fetch(&external_id, media_type).await?;
        store_metadata(&state.pool, source.id(), &external_id, media_type, &data, ttl_days).await?;
    }

    Ok(())
}

async fn store_metadata(
    pool: &SqlitePool,
    source: &str,
    external_id: &str,
    media_type: &str,
    data: &ProviderMetadata,
    ttl_days: i64,
) -> AppResult<()> {
    let expires_at = (Utc::now() + chrono::Duration::days(ttl_days.max(1)))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, certification, status, overview, poster_path,
         cached_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
         ON CONFLICT(source, external_id, media_type) DO UPDATE SET
            genres = excluded.genres,
            keywords = excluded.keywords,
            original_language = excluded.original_language,
            origin_countries = excluded.origin_countries,
            certification = excluded.certification,
            status = excluded.status,
            overview = excluded.overview,
            poster_path = excluded.poster_path,
            cached_at = excluded.cached_at,
            expires_at = excluded.expires_at",
    )
    .bind(source)
    .bind(external_id)
    .bind(media_type)
    .bind(serde_json::to_string(&data.genres)?)
    .bind(serde_json::to_string(&data.keywords)?)
    .bind(&data.original_language)
    .bind(serde_json::to_string(&data.origin_countries)?)
    .bind(&data.certification)
    .bind(&data.status)
    .bind(&data.overview)
    .bind(&data.poster_path)
    .bind(&expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Everything known about one item, every enabled source collapsed in priority
/// order — what a single media page needs, without loading the whole cache.
pub async fn resolve_for_media(
    state: &AppState,
    media: &crate::models::Media,
) -> AppResult<Option<crate::models::MediaMetadata>> {
    let mut parts: Vec<(&str, ProviderMetadata)> = Vec::new();
    let identifiers = metadata::load_identifiers(&state.pool).await?;

    // The configured order, *not* the usable subset: a key that has been
    // removed stops new fetches, it does not un-know what is already cached.
    // Reading and fetching must not disagree, or the explanation screen would
    // contradict the simulation that produced the decision it explains.
    for provider in state.metadata_order().await {
        if provider.id == metadata::ARR {
            parts.push((provider.id, metadata::from_media(media)));
            continue;
        }

        let Some(external_id) = metadata::external_id(provider, media, &identifiers) else {
            continue;
        };

        let row: Option<metadata::CacheRow> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} FROM metadata_cache WHERE source = ? AND external_id = ? AND media_type = ?",
            metadata::CACHE_COLUMNS
        )))
        .bind(provider.id)
        .bind(&external_id)
        .bind(&media.media_type)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(row) = row {
            parts.push((provider.id, row.into_answer()));
        }
    }

    Ok(crate::models::MediaMetadata::merge(parts))
}
