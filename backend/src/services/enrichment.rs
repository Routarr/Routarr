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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobHandle, JobKind};
use crate::models::{Media, ProviderMetadata};
use crate::services::metadata::{self, Addressing, FetchingSource};
use crate::services::rate_limit::RateLimiter;
use crate::services::routing;
use crate::state::AppState;

/// Outcome of an enrichment pass.
#[derive(Debug, Default, Clone)]
pub struct EnrichmentReport {
    pub considered: usize,
    pub enriched: usize,
    pub failed: usize,
    /// Items never asked about because the source was declared down mid-pass.
    pub skipped: usize,
    /// Items left to a later day: the source's daily quota was spent.
    pub deferred: usize,
}

/// Why an item of a pass went unanswered.
enum NotAsked {
    /// The breaker had opened.
    Skipped,
    /// Today's quota is spent, by the count or by the source's refusal.
    Deferred,
}

/// Enrich every media item whose metadata is missing or expired, source by
/// source, in the user's priority order.
pub async fn enrich_all_media(state: &AppState, by: &Attribution) -> AppResult<EnrichmentReport> {
    // Built on `metadata_providers`, the sources able to answer today: a source
    // with no key would fail every request of the pass. Evaluation reads the
    // whole `metadata_order` instead, so what a source answered before its key
    // was removed keeps counting.
    let sources = state.metadata_sources().await;
    if sources.is_empty() {
        debug!("No fetched metadata source is enabled, skipping enrichment");
        return Ok(EnrichmentReport::default());
    }

    let Some(_lock) = state.jobs.try_lock("enrich") else {
        return Err(AppError::Conflict("An enrichment pass is already running".into()));
    };

    let job =
        state.jobs.start(JobKind::Enrich, by, None, Detail::new("JobDetailEnriching")).await?;

    let mut report = EnrichmentReport::default();
    let mut outcome = Ok(());
    for source in &sources {
        match run_enrichment(state, source, &job).await {
            Ok(partial) => {
                report.considered += partial.considered;
                report.enriched += partial.enriched;
                report.failed += partial.failed;
                report.skipped += partial.skipped;
                report.deferred += partial.deferred;
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
            job.succeed(
                Detail::new("JobDetailEnriched")
                    .with("enriched", report.enriched)
                    .with("failed", report.failed),
            )
            .await
        }
        Err(e) => job.fail(e).await,
    }

    outcome.map(|()| report)
}

async fn run_enrichment(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
) -> AppResult<EnrichmentReport> {
    // A source that knows none of our identifiers has to find its own first.
    // A found identifier is kept, and a miss for a month or so
    // (`metadata::resolved_keys`), so this is a first-pass cost, not a
    // per-run one.
    let limiter = state.paces.of(source);

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
    // item. The providers tolerate a handful of parallel calls, and this is the
    // main driver of how fast a cold library becomes classifiable.
    //
    // Each future carries its own (external_id, media_type) back out: results
    // arrive in completion order, not input order, so pairing them with the
    // input list positionally would file a movie's metadata under a series and
    // lose both.
    let breaker = Breaker::new();
    // A source with a daily quota has the pass's share reserved at once, and
    // what goes unsent given back after: a count taken inside each request in
    // flight would wait on the database while this loop writes to it.
    let quota = source.daily_quota();
    let granted = match &quota {
        Some(quota) => quota.reserve(&state.pool, total).await?,
        None => total,
    };
    let targets: Vec<_> = targets.into_iter().take(granted).collect();
    // Set once the source refuses for its quota, which closes the day early.
    let closed = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicUsize::new(0));

    // Each answer is stored as it arrives, not once the pass ends: a pass over
    // a large library takes hours, and a restart or one failed write before
    // its end would otherwise throw away everything fetched, a day of a keyed
    // source's quota included.
    let mut results = stream::iter(targets)
        .map(|(external_id, media_type)| {
            let source = source.clone();
            let breaker = breaker.clone();
            let limiter = limiter.clone();
            let (closed, sent) = (Arc::clone(&closed), Arc::clone(&sent));
            async move {
                // `buffer_unordered` has already been handed every item, so the
                // breaker is checked here, as each future starts: the ones still
                // queued turn into no-ops instead of requests.
                if breaker.is_open() {
                    return (external_id, media_type, Err(NotAsked::Skipped));
                }
                if closed.load(Ordering::Relaxed) {
                    return (external_id, media_type, Err(NotAsked::Deferred));
                }

                // Paced before the request, not after a refusal: the point is
                // for the 429 never to be earned.
                limiter.acquire().await;
                sent.fetch_add(1, Ordering::Relaxed);
                let outcome = source.answer(&external_id, &media_type).await;
                if outcome.as_ref().is_err_and(crate::integrations::is_quota_spent) {
                    closed.store(true, Ordering::Relaxed);
                    return (external_id, media_type, Err(NotAsked::Deferred));
                }
                source.paced_after(&limiter, &outcome).await;
                breaker.record(&outcome);
                (external_id, media_type, Ok(outcome))
            }
        })
        .buffer_unordered(source.concurrency(state.config.metadata_concurrency));

    let mut report =
        EnrichmentReport { considered: total, deferred: total - granted, ..Default::default() };
    let mut rate_limited = false;
    let mut index = 0;

    while let Some((external_id, media_type, asked)) = results.next().await {
        index += 1;
        let result = match asked {
            Ok(result) => result,
            Err(NotAsked::Skipped) => {
                report.skipped += 1;
                continue;
            }
            Err(NotAsked::Deferred) => {
                report.deferred += 1;
                continue;
            }
        };
        match result {
            Ok(data) => {
                let stored = store_metadata(
                    &state.pool,
                    source.id(),
                    &external_id,
                    &media_type,
                    &data,
                    ttl_days,
                )
                .await;
                match stored {
                    Ok(()) => report.enriched += 1,
                    Err(e) => {
                        warn!("Could not store {} {external_id} ({media_type}): {e}", source.id());
                        report.failed += 1;
                    }
                }
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

        // Progress is only interesting at a coarse grain: one UPDATE per item
        // would cost more than the work it reports on.
        if index % 25 == 1 {
            job.progress(index, total).await;
        }
    }

    job.progress(total, total).await;

    if rate_limited {
        warn!(
            "{} rate-limited part of this pass. The remainder is retried on the next run",
            source.id()
        );
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

    if let Some(quota) = &quota {
        let given_back = if closed.load(Ordering::Relaxed) {
            quota.exhaust(&state.pool).await
        } else {
            quota.give_back(&state.pool, granted - sent.load(Ordering::Relaxed)).await
        };
        if let Err(e) = given_back {
            warn!("Could not count the requests {} sent today: {e}", source.id());
        }
    }
    if let Some(quota) = quota.as_ref().filter(|_| report.deferred > 0) {
        info!(
            "{} has spent the {} requests it may send today: {} item(s) wait for a later pass",
            source.id(),
            quota.limit(),
            report.deferred
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
/// Shared by both stages that issue requests (resolution and fetching), because
/// a search storm against an unavailable source costs exactly as much as a fetch
/// storm, and for a searching source it is the one that happens first.
#[derive(Clone)]
struct Breaker {
    failures: Arc<AtomicUsize>,
}

impl Breaker {
    /// A source that has refused this many times in a row is down, not
    /// unlucky.
    const LIMIT: usize = 5;

    fn new() -> Self {
        Self { failures: Arc::new(AtomicUsize::new(0)) }
    }

    fn is_open(&self) -> bool {
        self.failures.load(Ordering::Relaxed) >= Self::LIMIT
    }

    /// Record an outcome, counting only the failures that say something about
    /// the *source* rather than about one item, and only in a row: one answer
    /// starts the count again, or scattered errors over hours would abandon a
    /// source that answers nearly every request.
    fn record<T>(&self, outcome: &AppResult<T>) {
        if is_source_level_failure(outcome) {
            self.failures.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failures.store(0, Ordering::Relaxed);
        }
    }
}

/// Whether a failure is the *source* being unavailable rather than one item
/// being unknown to it.
///
/// A 404 means "this source does not have that item" and says nothing about the
/// next one. A 429, a 5xx or a transport failure means asking again right now is
/// pointless, and a 401 or a 403 (a refused key, a spent quota) is answered to
/// every item alike: those are the ones that trip the breaker.
fn is_source_level_failure<T>(outcome: &AppResult<T>) -> bool {
    matches!(
        outcome,
        Err(AppError::ExternalApi { status: 401 | 403 | 429 | 500 | 502..=504, .. })
            | Err(AppError::ExternalApi { status: 0, .. })
    )
}

/// One media row, reduced to what identifying it and telling whether it may
/// be searched need.
#[derive(sqlx::FromRow)]
struct Candidate {
    title: String,
    year: Option<i64>,
    media_type: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    genres: Option<String>,
    series_type: Option<String>,
}

/// Find this source's identifier for every item it has never been asked about.
///
/// Both outcomes are written down. Remembering that AniList has nothing for
/// *The Matrix* is what stops the next passes from searching for it again,
/// until the miss is old enough to be worth asking about once more.
async fn resolve_identifiers(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
    limiter: &RateLimiter,
) -> AppResult<()> {
    let known = metadata::resolved_keys(&state.pool, source.id()).await?;

    let rows: Vec<Candidate> = sqlx::query_as(
        "SELECT title, year, media_type, tmdb_id, tvdb_id, imdb_id, genres, series_type
           FROM media",
    )
    .fetch_all(&state.pool)
    .await?;
    let scope = state.setting("anime_search", metadata::ANIME_SEARCH[0].to_string()).await;

    // Deduplicated by local key: the same film in two Radarr instances is one
    // search, not two.
    let mut pending: Vec<(String, String, String, Option<i64>)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for row in rows {
        let genres = crate::models::genres_from(row.genres.as_deref());
        if !metadata::may_search(&scope, &genres, row.series_type.as_deref()) {
            continue;
        }
        let key = metadata::local_key_of(
            row.tmdb_id,
            row.tvdb_id,
            row.imdb_id.as_deref(),
            &row.title,
            row.year,
        );
        let dedup = metadata::resolution_key(&row.media_type, &key);
        if known.contains(&dedup) || !seen.insert(dedup) {
            continue;
        }
        pending.push((key, row.media_type, row.title, row.year));
    }

    if pending.is_empty() {
        return Ok(());
    }

    let total = pending.len();
    info!("{total} item(s) still to identify on {}", source.id());
    job.progress(0, total).await;

    let breaker = Breaker::new();

    // Written down as each search answers, for the reason the fetch stage is.
    let mut results = stream::iter(pending)
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
                let outcome = source.find(&title, year, &media_type).await;
                source.paced_after(&limiter, &outcome).await;
                breaker.record(&outcome);
                (key, media_type, Some(outcome))
            }
        })
        .buffer_unordered(source.concurrency(state.config.metadata_concurrency));

    let mut abandoned = 0usize;
    let mut index = 0;
    while let Some((key, media_type, outcome)) = results.next().await {
        index += 1;
        let Some(outcome) = outcome else {
            abandoned += 1;
            continue;
        };

        match outcome {
            Ok(external_id) => {
                let remembered = metadata::remember_identifier(
                    &state.pool,
                    source.id(),
                    &media_type,
                    &key,
                    external_id.as_deref(),
                )
                .await;
                if let Err(e) = remembered {
                    warn!("Could not record what {key} is on {}: {e}", source.id());
                }
            }
            // A failed search is *not* written down: the source or the
            // network failed, which says nothing of the title, and the
            // difference decides whether the next pass ever tries again.
            Err(e) => warn!("Could not identify {key} on {}: {e}", source.id()),
        }

        if index % 25 == 1 {
            job.progress(index, total).await;
        }
    }
    // The coarse steps above stop short of the end, and a pass left with
    // nothing to fetch ends here: without this its task finishes at "1/47".
    job.progress(total, total).await;

    if abandoned > 0 {
        warn!(
            "{} looks unavailable: gave up identifying {abandoned} item(s), retried next pass",
            source.id()
        );
    }

    Ok(())
}

/// Distinct identifiers, in this source's own namespace, whose cache entry is
/// missing or stale: the ones never asked first, then the stalest. A pass
/// that stops short, at a daily quota, would otherwise refresh the same
/// titles each day and never reach the rest.
///
/// Deduplicating here matters: the same film present in two Radarr instances
/// would otherwise be fetched twice.
async fn pending_targets(
    pool: &SqlitePool,
    source: &FetchingSource,
) -> AppResult<Vec<(String, String)>> {
    let answered = metadata::info(source.id())
        .map(|provider| provider.media_types)
        .unwrap_or_default()
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ");
    match source.addressing() {
        // Not fetched at all.
        Addressing::Local => Ok(Vec::new()),

        // `column` and the media types are literals from `services::metadata`,
        // never user input.
        Addressing::Column(column) => Ok(sqlx::query_as(AssertSqlSafe(format!(
            "SELECT DISTINCT CAST(m.{column} AS TEXT), m.media_type FROM media m
             LEFT JOIN metadata_cache c
                    ON c.source = ?
                   AND c.external_id = CAST(m.{column} AS TEXT)
                   AND c.media_type = m.media_type
             WHERE m.{column} IS NOT NULL AND CAST(m.{column} AS TEXT) != ''
               AND m.media_type IN ({answered})
               AND (c.external_id IS NULL OR c.expires_at < datetime('now'))
             ORDER BY c.external_id IS NOT NULL, c.expires_at"
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
               AND (c.external_id IS NULL OR c.expires_at < datetime('now'))
             ORDER BY c.external_id IS NOT NULL, c.expires_at",
        )
        .bind(source.id())
        .fetch_all(pool)
        .await?),
    }
}

/// How long a webhook delivery spends asking the sources about its title:
/// every one it can address, at its own pace, within this. What is not
/// reached is left to the next full pass.
pub const WEBHOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// What each source able to answer says now about `media`, where the cache
/// holds no answer: the webhook stores it, the placement only reads it.
///
/// Each source is asked by the id it is addressed with, from the row or from
/// a recorded search, or found by a search of its own, at its own pace, until
/// `deadline`. A search that finds nothing is an answer, and so is a refusal
/// about the title alone (`FetchingSource::answer`). A source that fails is
/// left out and the next is asked.
pub async fn ask_now(
    state: &AppState,
    media: &Media,
    deadline: Option<tokio::time::Instant>,
) -> routing::Fresh {
    let mut fresh = routing::Fresh::default();
    let providers = state.metadata_order().await;
    // Given back before any source is asked: held through their answers, it
    // would starve the pool.
    let read = async {
        let mut connection = state.pool.acquire().await?;
        let title = std::slice::from_ref(media);
        let identifiers = metadata::load_identifiers_of(&mut connection, title).await?;
        let cached =
            metadata::load_cache_of(&mut connection, title, &providers, &identifiers).await?;
        AppResult::Ok((identifiers, cached))
    };
    let Ok((identifiers, cached)) = read.await else {
        return fresh;
    };
    let scope = state.setting("anime_search", metadata::ANIME_SEARCH[0].to_string()).await;
    let searchable =
        metadata::may_search(&scope, &media.genre_list(), media.series_type.as_deref());
    for source in state.metadata_sources().await {
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            break;
        }
        let Some(provider) = metadata::info(source.id()) else { continue };
        let known = metadata::external_id(provider, media, &identifiers);
        let key = |external: &str| {
            (source.id().to_string(), external.to_string(), media.media_type.clone())
        };
        let searched =
            (source.id().to_string(), media.media_type.clone(), metadata::local_key(media));
        // A search on record that found nothing is not run again here: the
        // enrichment pass searches again once the miss is old enough.
        let missed = identifiers.get(&searched).is_some_and(Option::is_none);
        if known.as_deref().is_some_and(|external| cached.contains_key(&key(external))) {
            continue;
        }
        let pace = state.paces.of(&source);
        let external = match (known, source.addressing()) {
            (Some(external), _) => external,
            (None, Addressing::Search) if missed || !searchable => continue,
            (None, Addressing::Search) => {
                pace.acquire().await;
                let resolved = source.find(&media.title, media.year, &media.media_type).await;
                source.paced_after(&pace, &resolved).await;
                match resolved {
                    Ok(Some(external)) => {
                        fresh.identifiers.insert(searched, Some(external.clone()));
                        external
                    }
                    Ok(None) => {
                        fresh.identifiers.insert(searched, None);
                        continue;
                    }
                    Err(_) => continue,
                }
            }
            (None, _) => continue,
        };
        let quota = source.daily_quota();
        if let Some(quota) = &quota
            && quota.reserve(&state.pool, 1).await.unwrap_or(0) == 0
        {
            continue;
        }
        pace.acquire().await;
        let fetched = source.answer(&external, &media.media_type).await;
        if let (Some(quota), Err(error)) = (&quota, &fetched)
            && crate::integrations::is_quota_spent(error)
        {
            if let Err(e) = quota.exhaust(&state.pool).await {
                warn!("Could not record that {} spent its quota: {e}", source.id());
            }
            continue;
        }
        source.paced_after(&pace, &fetched).await;
        if let Ok(answer) = fetched {
            fresh.metadata.insert(key(&external), answer);
        }
    }
    fresh
}

/// Enrich one title now, as the webhook does before its rules run, so the
/// right root folder is known while the folder is still empty: every source
/// the row can address, within [`WEBHOOK_BUDGET`]. What a source answered is
/// stored as a full pass would store it.
pub async fn enrich_one(state: &AppState, media: &Media) -> AppResult<()> {
    let ttl_days: i64 = state.setting("metadata_cache_ttl_days", 7).await;
    let deadline = tokio::time::Instant::now() + WEBHOOK_BUDGET;
    let fresh = ask_now(state, media, Some(deadline)).await;
    for ((source, media_type, local_key), external) in &fresh.identifiers {
        metadata::remember_identifier(
            &state.pool,
            source,
            media_type,
            local_key,
            external.as_deref(),
        )
        .await?;
    }
    for ((source, external, media_type), data) in &fresh.metadata {
        store_metadata(&state.pool, source, external, media_type, data, ttl_days).await?;
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
    let expires_at = crate::services::routing::format_timestamp(
        Utc::now() + chrono::Duration::days(metadata::cache_days(source, ttl_days)),
    );

    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, certification, certification_scale,
         certifications, status, overview, cached_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), ?)
         ON CONFLICT(source, external_id, media_type) DO UPDATE SET
            genres = excluded.genres,
            keywords = excluded.keywords,
            original_language = excluded.original_language,
            origin_countries = excluded.origin_countries,
            certification = excluded.certification,
            certification_scale = excluded.certification_scale,
            certifications = excluded.certifications,
            status = excluded.status,
            overview = excluded.overview,
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
    .bind(&data.certification_scale)
    .bind(serde_json::to_string(&data.certifications)?)
    .bind(&data.status)
    .bind(&data.overview)
    .bind(&expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Everything known about one item, every enabled source collapsed in priority
/// order: what a single media page needs, without loading the whole cache.
///
/// Merged by the simulation's own merge over the configured order, *not* the
/// usable subset: a key that has been removed stops new fetches, it does not
/// un-know what is already cached. Read any other way, the media page and a
/// pinned rule case would describe the item otherwise than the engine sees it.
pub async fn resolve_for_media(
    state: &AppState,
    media: &crate::models::Media,
) -> AppResult<Option<crate::models::MediaMetadata>> {
    let providers = state.metadata_order().await;
    let (identifiers, cache) = {
        let mut connection = state.pool.acquire().await?;
        let title = std::slice::from_ref(media);
        let identifiers = metadata::load_identifiers_of(&mut connection, title).await?;
        let cache =
            metadata::load_cache_of(&mut connection, title, &providers, &identifiers).await?;
        (identifiers, cache)
    };
    let regions = AppState::certification_regions_from(&state.settings().await);
    let country: Option<String> =
        sqlx::query_scalar("SELECT certification_country FROM instances WHERE id = ?")
            .bind(&media.instance_id)
            .fetch_optional(&state.pool)
            .await?
            .flatten();
    Ok(crate::services::routing::resolve_metadata(
        media,
        &providers,
        &cache,
        &identifiers,
        &regions,
        country.as_deref(),
    ))
}
