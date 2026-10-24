//! Metadata enrichment: the whole backlog drained in one pass, several requests in flight and a
//! back-off when a source rate-limits, so a large library is not hours from being classifiable.
//! Only *fetched* sources come through here: the library sync enriches `arr` itself.

use futures::stream::{self, StreamExt};
use sqlx::{AssertSqlSafe, SqlitePool};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult, Upstream};
use crate::integrations::is_quota_spent;
use crate::jobs::{Attribution, Detail, JobHandle, JobKind};
use crate::models::{Media, ProviderMetadata};
use crate::services::anime_ids;
use crate::services::metadata::{self, Addressing, Answer, FetchingSource};
use crate::services::rate_limit::RateLimiter;
use crate::services::routing;
use crate::state::AppState;

/// Outcome of an enrichment pass.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct EnrichmentReport {
    /// Titles a source that has to search for its own identifiers looked for.
    pub searched: usize,
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
    /// Today is closed for the source: its quota spent, or the address blocked.
    Deferred,
}

/// Enrich every media item whose metadata is missing or expired, source by
/// source, in the user's priority order.
pub async fn enrich_all_media(state: &AppState, by: &Attribution) -> AppResult<EnrichmentReport> {
    let _pass = pass_lock(state).await?;
    run_pass(state, by).await
}

/// Ask `source`, or every source, again for each answer it gave, the anime table's ids it refused
/// included, then run a pass reading them. Refused while a pass runs, before anything is marked:
/// that pass has counted its titles, and the refresh would wait for the next.
pub async fn refresh(
    state: &AppState,
    by: &Attribution,
    source: Option<&'static str>,
) -> AppResult<EnrichmentReport> {
    let _pass = pass_lock(state).await?;
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    sqlx::query("UPDATE metadata_cache SET stale = 1 WHERE ?1 IS NULL OR source = ?1")
        .bind(source)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM anime_ids_refused WHERE ?1 IS NULL OR source = ?1")
        .bind(source)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    run_pass(state, by).await
}

async fn pass_lock(state: &AppState) -> AppResult<crate::jobs::JobLock> {
    match state.jobs.try_lock("enrich") {
        Some(lock) => Ok(lock),
        None => Err(AppError::InProgress {
            reason: "enrichment_running",
            message: state.localizer().await.translate("ErrorEnrichmentInProgress", &[]),
        }),
    }
}

async fn run_pass(state: &AppState, by: &Attribution) -> AppResult<EnrichmentReport> {
    // From `metadata_providers`, not `metadata_order`: a source with no key fails every request.
    let sources = state.metadata_sources().await;
    if sources.is_empty() {
        debug!("No fetched metadata source is enabled, skipping enrichment");
        return Ok(EnrichmentReport::default());
    }

    let mut job =
        state.jobs.start(JobKind::Enrich, by, None, Detail::new("JobDetailEnriching")).await?;

    let mut report = EnrichmentReport::default();
    let mut outcome = Ok(());
    for source in &sources {
        match run_enrichment(state, source, &job).await {
            Ok(partial) => {
                report.searched += partial.searched;
                report.considered += partial.considered;
                report.enriched += partial.enriched;
                report.failed += partial.failed;
                report.skipped += partial.skipped;
                report.deferred += partial.deferred;
            }
            // An unreachable source must not cancel the sources ranked below it.
            Err(e) => {
                warn!(source = %source.id(), "Source '{}' failed: {e}", source.id());
                outcome = Err(e);
            }
        }
    }

    match &outcome {
        Ok(()) => {
            let counted = report.searched + report.considered;
            job.progress(counted, counted).await;
            job.report(&report);
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

/// The requests a pass takes from a daily quota at a time: a pass stopped mid-way, by a restart,
/// leaves at most this many counted that it never sent.
pub(crate) const QUOTA_SLICE: usize = 50;

async fn run_enrichment(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
) -> AppResult<EnrichmentReport> {
    let id = source.id();
    let stage = Stage::new(state.paces.of(source));
    let mut held = Held::default();
    let name = metadata::info(id).map_or(id, |info| info.display_name);
    let held_back = source.is_held_today(&state.pool).await;

    let mut report = EnrichmentReport::default();
    // Found identifiers are kept, and misses for about a month (`metadata::resolved_keys`), so
    // the search costs the first pass, not every run.
    if source.addressing() == Addressing::Search && !held_back {
        job.stage(Detail::new("JobDetailIdentifying").with("source", name)).await;
        report.searched = resolve_identifiers(state, source, job, &stage, &mut held).await?;
    }
    job.stage(Detail::new("JobDetailFetching").with("source", name)).await;

    let ttl_days: i64 = state.setting("metadata_cache_ttl_days").await;
    let targets = pending_targets(&state.pool, source, metadata::cache_days(id, ttl_days)).await?;
    let total = targets.len();
    report.considered = total;
    if targets.is_empty() {
        info!(source = %id, "The {id} cache is up to date");
    } else if held_back {
        info!(source = %id, "{id} blocks this address until the next UTC day: {total} wait");
        report.deferred = total;
    } else {
        info!(source = %id, "{total} item(s) need enrichment from {id}");
        fetch_targets(state, source, job, &stage, &mut held, targets, &mut report).await?;
    }

    finish(state, source, &stage, held, &mut report).await;
    info!(
        source = %id,
        "{} item(s) enriched, {} failed, {} skipped, {} deferred",
        report.enriched, report.failed, report.skipped, report.deferred
    );
    Ok(report)
}

/// The fetch stage: each target asked at the source's pace and stored as its answer arrives. A
/// daily quota is taken a slice of [`QUOTA_SLICE`] at a time, before the slice's requests leave:
/// taken by a request in flight, it would wait on the database while this loop writes to it.
async fn fetch_targets(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
    stage: &Stage,
    held: &mut Held,
    targets: Vec<(String, String)>,
    report: &mut EnrichmentReport,
) -> AppResult<()> {
    let id = source.id();
    let total = targets.len();
    let quota = source.daily_quota();
    let slice = if quota.is_some() { QUOTA_SLICE } else { total };
    let mut rate_limited = false;
    let mut index = 0;

    for slice in targets.chunks(slice) {
        let granted = match &quota {
            Some(_) if stage.is_closed() => 0,
            Some(quota) => quota.reserve(&state.pool, slice.len()).await?,
            None => slice.len(),
        };
        report.deferred += slice.len() - granted;
        index += slice.len() - granted;
        if granted == 0 {
            continue;
        }
        let sent = Arc::new(AtomicUsize::new(0));

        // Each future carries its own (external_id, media_type) out, since results arrive in
        // completion order: paired by position, a movie's answer would be filed under a series.
        let mut results = stream::iter(slice[..granted].to_vec())
            .map(|(external_id, media_type)| {
                let (source, stage, sent) = (source.clone(), stage.clone(), Arc::clone(&sent));
                async move {
                    // Checked as each future starts, `buffer_unordered` holding every item
                    // already, so the ones still queued send nothing.
                    if stage.breaker.is_open() {
                        return (external_id, media_type, Err(NotAsked::Skipped));
                    }
                    if stage.is_closed() {
                        return (external_id, media_type, Err(NotAsked::Deferred));
                    }

                    // Paced before the request, so a 429 is never earned.
                    stage.limiter.acquire().await;
                    sent.fetch_add(1, Ordering::Relaxed);
                    let outcome = source.fetched(&external_id, &media_type).await;
                    if outcome.as_ref().is_err_and(is_quota_spent) {
                        stage.close();
                        return (external_id, media_type, Err(NotAsked::Deferred));
                    }
                    source.paced_after(&stage.limiter, &outcome).await;
                    stage.breaker.record(&outcome);
                    (external_id, media_type, Ok(outcome))
                }
            })
            .buffer_unordered(source.concurrency(state.config.metadata_concurrency));

        // Stored as each answer arrives: a pass over a large library takes hours, and a restart
        // before its end would lose everything fetched, a day of a keyed source's quota included.
        while let Some((external_id, media_type, asked)) = results.next().await {
            index += 1;
            if stage.breaker.has_answered() {
                held.flush(state, source, report).await;
            }
            match asked {
                Err(NotAsked::Skipped) => report.skipped += 1,
                Err(NotAsked::Deferred) => report.deferred += 1,
                Ok(Ok(Answer::Said(data))) => {
                    let stored =
                        store_metadata(&state.pool, id, &external_id, &media_type, &data).await;
                    count(report, stored, id, &external_id, &media_type);
                }
                Ok(Ok(Answer::Refused(status))) if stage.breaker.has_answered() => {
                    let stored =
                        store_refusal(state, source, &external_id, &media_type, status).await;
                    count(report, stored, id, &external_id, &media_type);
                }
                Ok(Ok(Answer::Refused(status))) => {
                    held.fetches.push((external_id, media_type, status));
                }
                Ok(Err(AppError::ExternalApi { status: 429, .. })) => {
                    rate_limited = true;
                    report.failed += 1;
                }
                Ok(Err(e)) => {
                    warn!(source = %id, "Failed to enrich {id} {external_id} ({media_type}): {e}");
                    report.failed += 1;
                }
            }

            // Coarse steps: an UPDATE per item would cost more than the work it reports on.
            if index % 25 == 1 {
                job.progress(index, total).await;
            }
        }

        if let Some(quota) = &quota
            && !stage.is_closed()
            && let Err(e) =
                quota.give_back(&state.pool, granted - sent.load(Ordering::Relaxed)).await
        {
            warn!(source = %id, "Could not count the requests {id} sent today: {e}");
        }
    }

    job.progress(total, total).await;

    if rate_limited {
        warn!(
            source = %id,
            "{id} rate-limited part of this pass. The remainder is retried on the next run"
        );
    }
    if report.skipped > 0 {
        warn!(
            source = %id,
            "{id} looks unavailable: gave up after {} failures and skipped {} item(s), \
             which the next pass retries",
            Breaker::LIMIT,
            report.skipped
        );
    }
    if let Some(quota) = quota.as_ref().filter(|_| report.deferred > 0) {
        info!(
            source = %id,
            "{id} has spent the {} requests it may send today: {} item(s) wait for a later pass",
            quota.limit(),
            report.deferred
        );
    }
    Ok(())
}

/// Counts one stored answer in `report`.
fn count(
    report: &mut EnrichmentReport,
    stored: AppResult<()>,
    source: &str,
    external_id: &str,
    media_type: &str,
) {
    match stored {
        Ok(()) => report.enriched += 1,
        Err(e) => {
            warn!(source = %source, "Could not store {source} {external_id} ({media_type}): {e}");
            report.failed += 1;
        }
    }
}

/// Settles what a source's pass leaves: the refusals held written down, or dropped when the
/// source refused everything, and the day closed when the source refused for the rest of it.
async fn finish(
    state: &AppState,
    source: &FetchingSource,
    stage: &Stage,
    mut held: Held,
    report: &mut EnrichmentReport,
) {
    let id = source.id();
    if stage.breaker.refused_everything() && !held.is_empty() {
        warn!(
            source = %id,
            "{id} refused all {} request(s) it was sent and answered none, a fault of the source \
             rather than of each title: nothing is written down, and the next pass asks again",
            held.searches.len() + held.fetches.len()
        );
        report.failed += held.fetches.len();
    } else {
        held.flush(state, source, report).await;
    }
    if stage.is_closed()
        && let Err(e) = source.close_the_day(&state.pool).await
    {
        warn!(source = %id, "Could not record that {id} refuses until tomorrow: {e}");
    }
}

/// What one source's stages of a pass share: its pace, its breaker, and whether the source
/// closed the day by refusing for the rest of it.
#[derive(Clone)]
struct Stage {
    limiter: RateLimiter,
    breaker: Breaker,
    closed: Arc<AtomicBool>,
}

impl Stage {
    fn new(limiter: RateLimiter) -> Self {
        Self { limiter, breaker: Breaker::new(), closed: Arc::new(AtomicBool::new(false)) }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// Stops a pass from hammering a source that is plainly down. One per source and pass, shared
/// by the search and the fetch, so a source that trips it while searching is asked for no record.
#[derive(Clone)]
struct Breaker {
    failures: Arc<AtomicUsize>,
    /// Refusals of a title and answers that do not decode, counted while nothing was answered.
    unanswered: Arc<AtomicUsize>,
    answered: Arc<AtomicBool>,
}

impl Breaker {
    /// A source that has refused this many times in a row is down, not unlucky.
    const LIMIT: usize = 5;

    fn new() -> Self {
        Self {
            failures: Arc::new(AtomicUsize::new(0)),
            unanswered: Arc::new(AtomicUsize::new(0)),
            answered: Arc::new(AtomicBool::new(false)),
        }
    }

    fn is_open(&self) -> bool {
        self.failures.load(Ordering::Relaxed) >= Self::LIMIT || self.refused_everything()
    }

    fn has_answered(&self) -> bool {
        self.answered.load(Ordering::Relaxed)
    }

    /// Whether the source refused [`Breaker::LIMIT`] titles and answered none: a wrong base path
    /// or a retired query earns every request the same refusal, which says nothing of a title.
    fn refused_everything(&self) -> bool {
        !self.has_answered() && self.unanswered.load(Ordering::Relaxed) >= Self::LIMIT
    }

    /// Count only the failures that say something about the *source*, and only in a row, or
    /// scattered errors over hours would abandon a source that answers nearly every request. A
    /// refused title, or an answer that does not decode, counts until the source answers anything.
    fn record<T>(&self, outcome: &AppResult<Answer<T>>) {
        match outcome {
            Ok(Answer::Said(_)) => {
                self.answered.store(true, Ordering::Relaxed);
                self.failures.store(0, Ordering::Relaxed);
            }
            Ok(Answer::Refused(_))
            | Err(AppError::ExternalApi { kind: Upstream::Unreadable, .. }) => {
                self.failures.store(0, Ordering::Relaxed);
                self.unanswered.fetch_add(1, Ordering::Relaxed);
            }
            outcome if is_source_level_failure(outcome) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
            _ => self.failures.store(0, Ordering::Relaxed),
        }
    }
}

/// Whether a failure is the *source* being unavailable rather than one item unknown to it. A 404
/// or an answer that does not decode says nothing of the next item, while a 408, a 429, a 5xx, a
/// transport failure, a 401 or a 403 (a refused key, a spent quota) meets every item alike.
fn is_source_level_failure<T>(outcome: &AppResult<T>) -> bool {
    matches!(
        outcome,
        Err(AppError::ExternalApi { status: 401 | 403 | 408 | 429 | 500..=599, .. })
            | Err(AppError::ExternalApi { kind: Upstream::Transport(_), .. })
    )
}

/// Refusals a source gave before it answered anything this pass. Written down as the misses they
/// are once it answers, and at the end of the pass, unless it refused [`Breaker::LIMIT`] and
/// answered none.
#[derive(Default)]
struct Held {
    /// `(local_key, media_type)` of each search refused.
    searches: Vec<(String, String)>,
    /// `(external_id, media_type, status)` of each fetch refused.
    fetches: Vec<(String, String, u16)>,
}

impl Held {
    fn is_empty(&self) -> bool {
        self.searches.is_empty() && self.fetches.is_empty()
    }

    async fn flush(
        &mut self,
        state: &AppState,
        source: &FetchingSource,
        report: &mut EnrichmentReport,
    ) {
        for (key, media_type) in std::mem::take(&mut self.searches) {
            remember_search(state, source.id(), &key, &media_type, None).await;
        }
        for (external_id, media_type, status) in std::mem::take(&mut self.fetches) {
            let stored = store_refusal(state, source, &external_id, &media_type, status).await;
            count(report, stored, source.id(), &external_id, &media_type);
        }
    }
}

/// Writes down what a search answered for `key`, a miss included, so the next passes search
/// again only once it is old enough.
async fn remember_search(
    state: &AppState,
    source: &str,
    key: &str,
    media_type: &str,
    external_id: Option<&str>,
) {
    let remembered = metadata::remember_identifier(
        &state.pool,
        source,
        media_type,
        key,
        external_id,
        metadata::BY_SEARCH,
    )
    .await;
    if let Err(e) = remembered {
        warn!(source = %source, "Could not record what {key} is on {source}: {e}");
    }
}

/// A title the source refuses, stored as the answer it is: an empty one, and the anime table's id
/// set aside when the source does not hold the work.
async fn store_refusal(
    state: &AppState,
    source: &FetchingSource,
    external_id: &str,
    media_type: &str,
    status: u16,
) -> AppResult<()> {
    set_aside(state, source.id(), external_id, status, false).await;
    store_metadata(&state.pool, source.id(), external_id, media_type, &Default::default()).await
}

/// Whether a refusal says the source does not hold the work: a 404 or a 410. A 400 or a 422
/// speaks of the request, and sets nothing aside.
fn is_gone(status: u16) -> bool {
    matches!(status, 404 | 410)
}

/// An id `source` refused as a work it does not hold, set aside if the anime table named it, so
/// the title is searched instead. `named` is an id the table gave a title asked about alone.
async fn set_aside(state: &AppState, source: &str, external_id: &str, status: u16, named: bool) {
    if is_gone(status)
        && let Err(e) = anime_ids::refuse(&state.pool, source, external_id, named).await
    {
        warn!(source = %source, "Could not set aside {source} {external_id}: {e}");
    }
}

/// How a source is asked about one title, which a pass and a title asked about alone (a webhook,
/// a refresh of the title, `GET /route`) decide alike through [`address`].
#[derive(Debug, PartialEq, Eq)]
enum Address {
    /// By the id the library or an earlier resolution holds.
    Known(String),
    /// By an id read without a request, the anime table's or the one the source naming works by
    /// the same ids found, with how it was found.
    Read(String, &'static str),
    /// By a search on the title and its year.
    Search,
    /// Not at all.
    Skip,
}

/// How `source` is asked about a title: by `known`, its id on record, else, for a source that
/// searches, by the table's id not refused (`mapped`), the id its namespace's other source found
/// (`shared`), or a search, unless a miss is on record or the title is not `searchable`.
fn address(
    source: &FetchingSource,
    known: Option<String>,
    missed: bool,
    mapped: Option<String>,
    shared: Option<String>,
    searchable: bool,
) -> Address {
    match (known, source.addressing()) {
        (Some(external), _) => Address::Known(external),
        (None, Addressing::Search) => match (mapped, shared) {
            (Some(external), _) => Address::Read(external, metadata::BY_MAPPING),
            (None, Some(external)) => Address::Read(external, metadata::BY_SHARING),
            (None, None) if missed || !searchable => Address::Skip,
            (None, None) => Address::Search,
        },
        (None, _) => Address::Skip,
    }
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

/// Find this source's identifier for every item never asked about, and answer how many it looked
/// for. A miss is written down too, so the next passes search for it again only once it is old
/// enough.
async fn resolve_identifiers(
    state: &AppState,
    source: &FetchingSource,
    job: &JobHandle,
    stage: &Stage,
    held: &mut Held,
) -> AppResult<usize> {
    let id = source.id();
    let resolved = metadata::resolved_keys(&state.pool, id).await?;
    let found = metadata::found_identifiers(&state.pool, id).await?;
    let table = anime_ids::Table::load(&state.pool).await?;
    let shared = match metadata::sharing_ids_with(id) {
        Some(other) => metadata::found_identifiers(&state.pool, other).await?,
        None => Default::default(),
    };

    let rows: Vec<Candidate> = sqlx::query_as(
        "SELECT title, year, media_type, tmdb_id, tvdb_id, imdb_id, genres, series_type
           FROM media",
    )
    .fetch_all(&state.pool)
    .await?;
    let scope = state.setting::<String>("anime_search").await;

    // Deduplicated by local key: the same film in two Radarr instances is one search.
    let mut pending: Vec<(String, String, String, Option<i64>)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for row in rows {
        let key = metadata::local_key_of(
            row.tmdb_id,
            row.tvdb_id,
            row.imdb_id.as_deref(),
            &row.title,
            row.year,
        );
        let dedup = metadata::resolution_key(&row.media_type, &key);
        if seen.contains(&dedup) {
            continue;
        }
        // Read whatever the title's genres say: the table holds anime alone, and costs no request.
        let mapped = table
            .find(&row.media_type, row.tmdb_id, row.tvdb_id, row.imdb_id.as_deref())
            .and_then(|ids| ids.of(id))
            .filter(|external| !table.refused(id, external));
        let genres = crate::models::genres_from(row.genres.as_deref());
        let searchable = metadata::may_search(&scope, &genres, row.series_type.as_deref());
        let missed = resolved.contains(&dedup) && !found.contains_key(&dedup);
        let known = found.get(&dedup).cloned();
        match address(source, known, missed, mapped, shared.get(&dedup).cloned(), searchable) {
            // A found id is the fetch stage's to read.
            Address::Known(_) | Address::Skip => {}
            Address::Read(external, found_by) => {
                seen.insert(dedup);
                let remembered = metadata::remember_identifier(
                    &state.pool,
                    id,
                    &row.media_type,
                    &key,
                    Some(&external),
                    found_by,
                )
                .await;
                if let Err(e) = remembered {
                    warn!(source = %id, "Could not record what {key} is on {id}: {e}");
                }
            }
            Address::Search => {
                seen.insert(dedup);
                pending.push((key, row.media_type, row.title, row.year));
            }
        }
    }

    if pending.is_empty() {
        return Ok(0);
    }

    let total = pending.len();
    info!(source = %id, "{total} item(s) still to identify on {id}");
    job.progress(0, total).await;

    // Written down as each search answers, for the reason the fetch stage is.
    let mut results = stream::iter(pending)
        .map(|(key, media_type, title, year)| {
            let (source, stage) = (source.clone(), stage.clone());
            async move {
                // A failed search is not written down, so the storm would repeat every pass.
                if stage.breaker.is_open() {
                    return (key, media_type, Err(NotAsked::Skipped));
                }
                if stage.is_closed() {
                    return (key, media_type, Err(NotAsked::Deferred));
                }

                stage.limiter.acquire().await;
                let outcome = source.find(&title, year, &media_type).await;
                if outcome.as_ref().is_err_and(is_quota_spent) {
                    stage.close();
                    return (key, media_type, Err(NotAsked::Deferred));
                }
                source.paced_after(&stage.limiter, &outcome).await;
                stage.breaker.record(&outcome);
                (key, media_type, Ok(outcome))
            }
        })
        .buffer_unordered(source.concurrency(state.config.metadata_concurrency));

    let mut abandoned = 0usize;
    let mut index = 0;
    let mut unused = EnrichmentReport::default();
    while let Some((key, media_type, asked)) = results.next().await {
        index += 1;
        if stage.breaker.has_answered() {
            held.flush(state, source, &mut unused).await;
        }
        match asked {
            Err(NotAsked::Skipped) => abandoned += 1,
            Err(NotAsked::Deferred) => {}
            Ok(Ok(Answer::Said(external_id))) => {
                remember_search(state, id, &key, &media_type, external_id.as_deref()).await;
            }
            Ok(Ok(Answer::Refused(_))) if stage.breaker.has_answered() => {
                remember_search(state, id, &key, &media_type, None).await;
            }
            Ok(Ok(Answer::Refused(_))) => held.searches.push((key, media_type)),
            // Not written down: a failure says nothing of the title, and a miss on record would
            // keep the next passes from trying again.
            Ok(Err(e)) => warn!(source = %id, "Could not identify {key} on {id}: {e}"),
        }

        if index % 25 == 1 {
            job.progress(index, total).await;
        }
    }
    // The coarse steps stop short of the end, and a pass with nothing to fetch ends here.
    job.progress(total, total).await;

    if abandoned > 0 {
        warn!(
            source = %id,
            "{id} looks unavailable: gave up identifying {abandoned} item(s), retried next pass"
        );
    }

    Ok(total)
}

/// Distinct identifiers in this source's namespace whose answer is missing or due, never asked
/// first, then asked again, then the stalest, then by id: a pass a daily quota cuts short reaches
/// the rest next time. Due is older than `lifetime` days, read now so a lowered one applies at
/// once. Distinct, or a film in two Radarr instances is fetched twice.
async fn pending_targets(
    pool: &SqlitePool,
    source: &FetchingSource,
    lifetime: i64,
) -> AppResult<Vec<(String, String)>> {
    let due = format!("-{lifetime} days");
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

        // `column` and the media types are literals of `services::metadata`, never user input.
        Addressing::Column(column) => Ok(sqlx::query_as(AssertSqlSafe(format!(
            "SELECT DISTINCT CAST(m.{column} AS TEXT), m.media_type FROM media m
             LEFT JOIN metadata_cache c
                    ON c.source = ?
                   AND c.external_id = CAST(m.{column} AS TEXT)
                   AND c.media_type = m.media_type
             WHERE m.{column} IS NOT NULL AND CAST(m.{column} AS TEXT) != ''
               AND m.media_type IN ({answered})
               AND (c.external_id IS NULL OR c.stale = 1 OR c.cached_at < datetime('now', ?))
             ORDER BY c.external_id IS NOT NULL, c.stale = 0, c.cached_at,
                      m.{column}, m.media_type"
        )))
        .bind(source.id())
        .bind(&due)
        .fetch_all(pool)
        .await?),

        // A NULL `external_id` is a source that was asked and had nothing: it is not asked again.
        Addressing::Search => Ok(sqlx::query_as(
            "SELECT DISTINCT s.external_id, s.media_type FROM source_identifiers s
             LEFT JOIN metadata_cache c
                    ON c.source = s.source
                   AND c.external_id = s.external_id
                   AND c.media_type = s.media_type
             WHERE s.source = ? AND s.external_id IS NOT NULL
               AND (c.external_id IS NULL OR c.stale = 1 OR c.cached_at < datetime('now', ?))
             ORDER BY c.external_id IS NOT NULL, c.stale = 0, c.cached_at,
                      s.external_id, s.media_type",
        )
        .bind(source.id())
        .bind(&due)
        .fetch_all(pool)
        .await?),
    }
}

/// How long a title asked about alone waits for the sources, a webhook delivery's and a
/// `GET /route?enrich=true` alike: every one it can address, at its own pace, within this. What
/// is not reached is left to the next full pass.
pub const ASK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// The same for a refresh of one title, a task nobody waits on.
const TITLE_REFRESH_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

/// What each source able to answer says now about `media` where the cache holds none, each at
/// its own pace until `deadline`: the webhook stores it, the placement only reads it. A failing
/// source is skipped, and a search finding nothing or a refusal of the title alone is an answer.
pub async fn ask_now(state: &AppState, media: &Media, deadline: Instant) -> routing::Fresh {
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
    let scope = state.setting::<String>("anime_search").await;
    let searchable =
        metadata::may_search(&scope, &media.genre_list(), media.series_type.as_deref());
    let table = anime_ids::Table::for_title(
        &state.pool,
        &media.media_type,
        media.tmdb_id,
        media.tvdb_id,
        media.imdb_id.as_deref(),
    )
    .await
    .unwrap_or_default();
    let mapped_ids =
        table.find(&media.media_type, media.tmdb_id, media.tvdb_id, media.imdb_id.as_deref());
    for source in state.metadata_sources().await {
        if Instant::now() >= deadline {
            break;
        }
        let Some(provider) = metadata::info(source.id()) else { continue };
        if source.is_held_today(&state.pool).await {
            continue;
        }
        let known = metadata::external_id(provider, media, &identifiers);
        let key = |external: &str| {
            (source.id().to_string(), external.to_string(), media.media_type.clone())
        };
        let searched =
            (source.id().to_string(), media.media_type.clone(), metadata::local_key(media));
        // A search on record that found nothing is not run again here: the
        // enrichment pass searches again once the miss is old enough.
        let missed = identifiers.get(&searched).is_some_and(Option::is_none);
        let mapped = mapped_ids
            .and_then(|ids| ids.of(source.id()))
            .filter(|external| !table.refused(source.id(), external));
        let shared = metadata::sharing_ids_with(source.id()).and_then(|other| {
            let theirs = (other.to_string(), media.media_type.clone(), metadata::local_key(media));
            identifiers.get(&theirs).cloned().flatten()
        });
        // An answer asked for again counts as none.
        if let Some(external) =
            known.as_deref().filter(|external| cached.contains_key(&key(external)))
            && !metadata::is_stale(&state.pool, source.id(), external, &media.media_type).await
        {
            continue;
        }
        let pace = state.paces.of(&source);
        let external = match address(&source, known, missed, mapped, shared, searchable) {
            Address::Known(external) => external,
            Address::Read(external, found_by) => {
                fresh.found_by.insert(searched.clone(), found_by);
                fresh.identifiers.insert(searched.clone(), Some(external.clone()));
                external
            }
            Address::Skip => continue,
            Address::Search => {
                if !pace.acquire_by(deadline).await {
                    continue;
                }
                let finding = source.find(&media.title, media.year, &media.media_type);
                let Ok(resolved) = tokio::time::timeout_at(deadline, finding).await else {
                    continue;
                };
                if resolved.as_ref().is_err_and(is_quota_spent) {
                    close_the_day(state, &source).await;
                    continue;
                }
                source.paced_after(&pace, &resolved).await;
                match resolved {
                    Ok(Answer::Said(Some(external))) => {
                        fresh.identifiers.insert(searched.clone(), Some(external.clone()));
                        external
                    }
                    Ok(Answer::Said(None) | Answer::Refused(_)) => {
                        fresh.identifiers.insert(searched, None);
                        continue;
                    }
                    Err(_) => continue,
                }
            }
        };
        let quota = source.daily_quota();
        if let Some(quota) = &quota
            && quota.reserve(&state.pool, 1).await.unwrap_or(0) == 0
        {
            continue;
        }
        if !pace.acquire_by(deadline).await {
            if let Some(quota) = &quota
                && let Err(e) = quota.give_back(&state.pool, 1).await
            {
                let id = source.id();
                warn!(source = %id, "Could not give back a request of {id}: {e}");
            }
            continue;
        }
        let fetching = source.fetched(&external, &media.media_type);
        let Ok(fetched) = tokio::time::timeout_at(deadline, fetching).await else {
            continue;
        };
        if fetched.as_ref().is_err_and(is_quota_spent) {
            close_the_day(state, &source).await;
            continue;
        }
        source.paced_after(&pace, &fetched).await;
        match fetched {
            Ok(Answer::Said(answer)) => {
                fresh.metadata.insert(key(&external), answer);
            }
            Ok(Answer::Refused(status)) => {
                let named = fresh.found_by.get(&searched) == Some(&metadata::BY_MAPPING);
                // Set aside rather than kept as the title's, as a pass leaves it.
                if named && is_gone(status) {
                    fresh.identifiers.remove(&searched);
                    fresh.found_by.remove(&searched);
                }
                set_aside(state, source.id(), &external, status, named).await;
                fresh.metadata.insert(key(&external), Default::default());
            }
            Err(_) => {}
        }
    }
    fresh
}

async fn close_the_day(state: &AppState, source: &FetchingSource) {
    if let Err(e) = source.close_the_day(&state.pool).await {
        let id = source.id();
        warn!(source = %id, "Could not record that {id} refuses until tomorrow: {e}");
    }
}

/// Enrich one title now, before the webhook's rules run, so the right root folder is known while
/// the folder is still empty: every source the row can address, within [`ASK_BUDGET`], stored
/// as a full pass stores it.
pub async fn enrich_one(state: &AppState, media: &Media) -> AppResult<()> {
    let deadline = Instant::now() + ASK_BUDGET;
    store_fresh(state, &ask_now(state, media, deadline).await).await.map(|_| ())
}

/// What asking the sources again about one title found.
#[derive(Debug, serde::Serialize)]
pub struct TitleRefresh {
    /// The sources that gave an answer.
    pub answered: usize,
}

/// Ask the sources again about `media`, or `source` alone: its answers are marked stale and its
/// search match and refusals of the table's ids forgotten, so a wrong AniList or MyAnimeList match
/// is searched anew. An answer a source does not give stays stale for the next pass.
pub async fn refresh_title(
    state: &AppState,
    by: &Attribution,
    media: &Media,
    source: Option<&'static str>,
) -> AppResult<TitleRefresh> {
    let detail = |key| Detail::new(key).with("title", &media.title);
    let mut job =
        state.jobs.start(JobKind::Enrich, by, None, detail("JobDetailRefreshingTitle")).await?;
    let outcome = async {
        forget_title(state, media, source).await?;
        let deadline = Instant::now() + TITLE_REFRESH_BUDGET;
        store_fresh(state, &ask_now(state, media, deadline).await).await
    }
    .await;
    match outcome {
        Ok(answered) => {
            let refreshed = TitleRefresh { answered };
            job.report(&refreshed);
            job.succeed(detail("JobDetailRefreshedTitle").with("answered", answered)).await;
            Ok(refreshed)
        }
        Err(e) => {
            job.fail(&e).await;
            Err(e)
        }
    }
}

/// Mark `media`'s answers stale, `source`'s alone when named, and forget what each searched
/// source found for it and which of the table's ids for it each refused.
async fn forget_title(
    state: &AppState,
    media: &Media,
    source: Option<&'static str>,
) -> AppResult<()> {
    let title = std::slice::from_ref(media);
    let identifiers = {
        let mut connection = state.pool.acquire().await?;
        metadata::load_identifiers_of(&mut connection, title).await?
    };
    let mapped = anime_ids::Table::for_title(
        &state.pool,
        &media.media_type,
        media.tmdb_id,
        media.tvdb_id,
        media.imdb_id.as_deref(),
    )
    .await?
    .find(&media.media_type, media.tmdb_id, media.tvdb_id, media.imdb_id.as_deref());
    let asked = metadata::PROVIDERS
        .iter()
        .filter(|provider| provider.fetched && source.is_none_or(|id| id == provider.id));
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    for provider in asked {
        if let Some(external) = metadata::external_id(provider, media, &identifiers) {
            sqlx::query(
                "UPDATE metadata_cache SET stale = 1
                  WHERE source = ? AND external_id = ? AND media_type = ?",
            )
            .bind(provider.id)
            .bind(&external)
            .bind(&media.media_type)
            .execute(&mut *tx)
            .await?;
        }
        if provider.addressing == Addressing::Search {
            sqlx::query(
                "DELETE FROM source_identifiers
                  WHERE source = ? AND media_type = ? AND local_key = ?",
            )
            .bind(provider.id)
            .bind(&media.media_type)
            .bind(metadata::local_key(media))
            .execute(&mut *tx)
            .await?;
        }
        if let Some(external) = mapped.and_then(|ids| ids.of(provider.id)) {
            sqlx::query("DELETE FROM anime_ids_refused WHERE source = ? AND external_id = ?")
                .bind(provider.id)
                .bind(&external)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Store what [`ask_now`] found, as a pass stores it, and answer how many
/// answers that was.
async fn store_fresh(state: &AppState, fresh: &routing::Fresh) -> AppResult<usize> {
    for (key, external) in &fresh.identifiers {
        let (source, media_type, local_key) = key;
        let found_by = fresh.found_by.get(key).copied().unwrap_or(metadata::BY_SEARCH);
        metadata::remember_identifier(
            &state.pool,
            source,
            media_type,
            local_key,
            external.as_deref(),
            found_by,
        )
        .await?;
    }
    for ((source, external, media_type), data) in &fresh.metadata {
        store_metadata(&state.pool, source, external, media_type, data).await?;
    }
    Ok(fresh.metadata.len())
}

async fn store_metadata(
    pool: &SqlitePool,
    source: &str,
    external_id: &str,
    media_type: &str,
    data: &ProviderMetadata,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
         original_language, origin_countries, certification, certification_scale,
         certifications, status, overview, ratings, cached_at, stale)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), 0)
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
            ratings = excluded.ratings,
            cached_at = excluded.cached_at,
            stale = 0",
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
    .bind(serde_json::to_string(&data.ratings)?)
    .execute(pool)
    .await?;

    Ok(())
}

/// Everything the sources say about one item, merged as the simulation merges it over the
/// configured order, not the usable subset: a removed key stops new fetches but un-knows nothing,
/// or the media page and a pinned rule case would not see the item as the engine does.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Transport, Upstream};
    use crate::integrations::upstream;

    /// The source is down, or refuses every title alike: those trip the
    /// breaker. One title it answers with a record that does not decode, or
    /// does not have, says nothing about the next title.
    #[test]
    fn only_a_failure_of_the_source_trips_the_breaker() {
        let failed =
            |status, kind| -> AppResult<()> { Err(upstream("TMDB", status, kind, String::new())) };
        for down in [
            failed(0, Upstream::Transport(Transport::TimedOut)),
            failed(0, Upstream::Transport(Transport::Failed)),
            failed(503, Upstream::Http),
            failed(522, Upstream::Http),
            failed(501, Upstream::Http),
            failed(408, Upstream::Http),
            failed(401, Upstream::RefusedWith200),
            failed(429, Upstream::QuotaSpent),
        ] {
            assert!(is_source_level_failure(&down), "{down:?}");
        }
        for one_title in [
            failed(0, Upstream::Unreadable),
            failed(0, Upstream::TooLarge),
            failed(404, Upstream::Http),
            failed(200, Upstream::RefusedWith200),
        ] {
            assert!(!is_source_level_failure(&one_title), "{one_title:?}");
        }
    }
}
