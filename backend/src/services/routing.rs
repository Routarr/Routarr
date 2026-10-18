//! Simulation: turn the current library state into explainable routing decisions.
//! A run loads in a fixed number of queries and writes in one transaction, since a
//! query per item fails `tests/scale.rs`.

use chrono::Utc;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use tracing::info;
use uuid::Uuid;

use crate::error::AppResult;
use crate::localization::Localizer;
use crate::models::*;
use crate::services::metadata::{self, ProviderInfo};
use crate::services::rule_engine::{self, EvalContext, OVERRIDE_RULE_ID};

/// How often a simulation followed by a job says how far it has gone.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Knobs for a simulation run.
#[derive(Debug, Clone)]
pub struct SimulationOptions {
    /// What caused this run, one of the `TRIGGER_*` constants, stored on every
    /// decision it writes.
    pub trigger: String,
    /// The name the authentication mode vouched for, if any. `None` for the
    /// scheduler and the webhook. See `Identity::actor`.
    pub subject: Option<String>,
    /// The application key that asked, when one did (`Attribution::key`).
    pub subject_key: Option<String>,
    pub instance_ids: Vec<String>,
    /// Evaluate only these media rows, so a webhook import runs no full
    /// simulation per episode.
    pub media_ids: Option<Vec<String>>,
    pub media_type: Option<String>,
    /// Persist the decisions so they can be applied and audited.
    pub persist: bool,
    /// Also persist decisions for media already in the right place, off by
    /// default since a large library would store thousands such per run.
    pub persist_unchanged: bool,
    /// Evaluate this rule set instead of the stored one (impact preview).
    pub rules_override: Option<Vec<Rule>>,
    /// Cap the number of decisions returned in the response payload.
    pub max_returned: Option<usize>,
    /// Language the stored explanations are written in.
    pub language: String,
    /// The job a caller follows, told how many titles have been evaluated.
    pub progress: Option<crate::jobs::Progress>,
}

impl Default for SimulationOptions {
    /// A run nobody attributed is a manual one: the scheduler and the webhook
    /// both name themselves, and nothing else runs a simulation unprompted.
    fn default() -> Self {
        Self {
            trigger: crate::jobs::TRIGGER_MANUAL.to_string(),
            subject: None,
            subject_key: None,
            instance_ids: Vec::new(),
            media_ids: None,
            media_type: None,
            persist: false,
            persist_unchanged: false,
            rules_override: None,
            max_returned: None,
            language: String::new(),
            progress: None,
        }
    }
}

/// Everything needed to evaluate a library, loaded once per run.
#[derive(Default)]
struct RoutingContext {
    rules: Vec<Rule>,
    overrides: HashMap<String, String>,
    /// (instance_id, category) -> root folder path
    root_folders: HashMap<(String, String), String>,
    /// (instance_id, `paths::key` of a root folder) -> the category it leads to
    folder_categories: HashMap<(String, String), String>,
    /// (instance_id, normalised path) -> free bytes the Arr last reported, or
    /// `None`. Normalised, or a trailing slash makes every move cross filesystems.
    free_space: HashMap<(String, String), Option<i64>>,
    instance_names: HashMap<String, String>,
    /// The country each instance's Arr rates for, where it is known.
    instance_countries: HashMap<String, String>,
    /// The certification regions, most preferred first.
    regions: Vec<String>,
    /// Enabled metadata sources, highest priority first.
    providers: Vec<&'static ProviderInfo>,
    /// (source, external id, media type) -> that source's answer.
    metadata: HashMap<(String, String, String), ProviderMetadata>,
    /// What the searching sources resolved each item to, loaded once, never per item.
    identifiers: metadata::Identifiers,
    default_category: String,
}

/// Everything one pass over the library reads, loaded once and held with the
/// permit that bounds concurrent loads. Several rule sets evaluate over one load.
pub struct LoadedLibrary {
    /// `None` for named titles, which load their own context and no library.
    _pass: Option<tokio::sync::OwnedSemaphorePermit>,
    /// What the load took, counted into every evaluation over it: a run's
    /// figure is load plus evaluation, whichever of the two ran it.
    loaded_in: std::time::Duration,
    /// Where this load stands among every other: see [`load_order`].
    order: i64,
    ctx: RoutingContext,
    media: Vec<Media>,
}

/// A number larger than any load took before, so that two runs storing one
/// title tell which of them read the library last. Taken from the clock, in
/// microseconds, so the order holds across a restart.
fn load_order() -> i64 {
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = Utc::now().timestamp_micros();
    let next = |last: i64| now.max(last + 1);
    match LAST.try_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(next(last))) {
        Ok(last) | Err(last) => next(last),
    }
}

/// Load the context and the media a pass with these options evaluates.
pub async fn load_library(
    pool: &SqlitePool,
    passes: &LibraryPasses,
    options: &SimulationOptions,
) -> AppResult<Arc<LoadedLibrary>> {
    let started = std::time::Instant::now();
    let order = load_order();

    // A filter one statement cannot bind is refused as a bad request.
    let media_filter_len = options.media_ids.as_ref().map_or(0, Vec::len);
    for (name, len) in
        [("instance_ids", options.instance_ids.len()), ("media_ids", media_filter_len)]
    {
        if len > BIND_CHUNK {
            return Err(crate::error::AppError::BadRequest(format!(
                "{name} names more than {BIND_CHUNK} ids"
            )));
        }
    }

    // Named titles load only their own context, with no permit to queue behind previews.
    if let Some(ids) = options.media_ids.as_deref() {
        let mut snapshot = pool.begin().await?;
        let media = load_media(
            &mut *snapshot,
            &options.instance_ids,
            Some(ids),
            options.media_type.as_deref(),
        )
        .await?;
        let ctx = if media.is_empty() {
            RoutingContext::default()
        } else {
            load_context(&mut snapshot, Scope::Items(&media)).await?
        };
        snapshot.commit().await?;
        let loaded_in = started.elapsed();
        return Ok(Arc::new(LoadedLibrary { _pass: None, loaded_in, order, ctx, media }));
    }

    let pass = passes.take().await;
    let mut snapshot = pool.begin().await?;
    let ctx = load_context(&mut snapshot, Scope::Library).await?;
    let media =
        load_media(&mut *snapshot, &options.instance_ids, None, options.media_type.as_deref())
            .await?;
    snapshot.commit().await?;

    let loaded_in = started.elapsed();
    Ok(Arc::new(LoadedLibrary { _pass: Some(pass), loaded_in, order, ctx, media }))
}

/// Run a full simulation and, when asked, persist the resulting decisions.
pub async fn run_simulation(
    pool: &SqlitePool,
    passes: &LibraryPasses,
    options: SimulationOptions,
) -> AppResult<SimulationResult> {
    let library = load_library(pool, passes, &options).await?;
    simulate_loaded(pool, &library, options).await
}

/// Evaluate a loaded library under these options, and persist when asked. Only the persist
/// reaches the database, so a second evaluation over the same load costs no query.
pub async fn simulate_loaded(
    pool: &SqlitePool,
    library: &Arc<LoadedLibrary>,
    mut options: SimulationOptions,
) -> AppResult<SimulationResult> {
    let started = std::time::Instant::now();
    let simulation_id = Uuid::new_v4().to_string();
    let now = Utc::now();

    // Every move and skip is worded, and as many unchanged titles as the run keeps.
    let unchanged = if options.persist_unchanged {
        usize::MAX
    } else {
        options.max_returned.unwrap_or(usize::MAX)
    };
    let wording = Wording {
        localizer: Localizer::new(&options.language),
        trigger: options.trigger.clone(),
        subject: options.subject.clone(),
        subject_key: options.subject_key.clone(),
        simulation_id: simulation_id.clone(),
        unchanged,
    };
    let rules = options.rules_override.take();
    let loaded = Arc::clone(library);
    let Pass { verdicts, mut decisions, counters, capacity } =
        off_the_workers(options.progress.as_ref(), library.media.len(), move |evaluated| {
            let rules = rules.as_deref().unwrap_or(&loaded.ctx.rules);
            evaluate_pass(&loaded, rules, now, Some(&wording), evaluated)
        })
        .await?;

    if options.persist {
        let stored: Vec<&Decision> = decisions
            .iter()
            .filter(|d| options.persist_unchanged || d.action != DecisionAction::None)
            .collect();
        store_run(pool, library, &options.trigger, &simulation_id, now, &verdicts, &stored).await?;
    }

    let elapsed_ms = (library.loaded_in + started.elapsed()).as_millis() as u64;
    info!(
        simulation_id = %simulation_id,
        total = verdicts.len(),
        moves = counters.moves_required,
        correct = counters.already_correct,
        unmapped = counters.skipped_unmapped,
        unmatched = counters.no_category_match,
        overrides = counters.overrides_applied,
        elapsed_ms,
        "Simulation complete"
    );

    if let Some(max) = options.max_returned {
        // Keep the actionable ones when the payload has to be trimmed.
        decisions.sort_by_key(|d| match d.action {
            DecisionAction::Move => 0,
            DecisionAction::Skip => 1,
            DecisionAction::None => 2,
        });
        decisions.truncate(max);
    }

    Ok(SimulationResult {
        capacity,
        simulation_id,
        total_media: verdicts.len(),
        returned: decisions.len(),
        decisions,
        moves_required: counters.moves_required,
        already_correct: counters.already_correct,
        no_category_match: counters.no_category_match,
        overrides_applied: counters.overrides_applied,
        skipped_unmapped: counters.skipped_unmapped,
        excluded_by_rule: counters.excluded_by_rule,
        elapsed_ms,
    })
}

/// Run a pass on a blocking thread, reporting to `progress` meanwhile: seconds
/// of computing on an async worker would stall the polling and the webhooks.
async fn off_the_workers<T: Send + 'static>(
    progress: Option<&crate::jobs::Progress>,
    total: usize,
    pass: impl FnOnce(&AtomicUsize) -> T + Send + 'static,
) -> AppResult<T> {
    let evaluated = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&evaluated);
    let mut running = tokio::task::spawn_blocking(move || pass(&counted));
    let Some(progress) = progress else {
        return running.await.map_err(pass_failed);
    };
    progress.report(0, total).await;
    loop {
        tokio::select! {
            done = &mut running => {
                let done = done.map_err(pass_failed)?;
                progress.report(total, total).await;
                return Ok(done);
            }
            _ = tokio::time::sleep(PROGRESS_EVERY) => {
                progress.report(evaluated.load(Ordering::Relaxed), total).await;
            }
        }
    }
}

fn pass_failed(e: tokio::task::JoinError) -> crate::error::AppError {
    crate::error::AppError::Internal(format!("A pass over the library stopped: {e}"))
}

/// How a pass words the decisions it keeps.
struct Wording {
    localizer: Localizer,
    trigger: String,
    subject: Option<String>,
    subject_key: Option<String>,
    simulation_id: String,
    /// How many titles left where they are get a decision.
    unchanged: usize,
}

/// What one title's evaluation decided, without the words: kept for every
/// title a pass reads, where decisions are kept for those worth showing.
struct Verdict {
    category: String,
    /// The rule that sent the title there, `OVERRIDE_RULE_ID` for an exception.
    matched_rule_id: Option<String>,
}

/// One rule set over one library.
struct Pass {
    /// One per title, in the library's order.
    verdicts: Vec<Verdict>,
    decisions: Vec<Decision>,
    counters: Counters,
    capacity: Vec<CapacityForecast>,
}

/// Evaluate every title of `library` under `rules`, and word the decisions
/// `wording` keeps. Pure computing: no query, no await, so it runs off the
/// async workers.
fn evaluate_pass(
    library: &LoadedLibrary,
    rules: &[Rule],
    now: chrono::DateTime<Utc>,
    wording: Option<&Wording>,
    evaluated: &AtomicUsize,
) -> Pass {
    let ctx = &library.ctx;
    let mut verdicts = Vec::with_capacity(library.media.len());
    let mut decisions = Vec::new();
    let mut counters = Counters::default();
    let mut left_alone = 0;

    /// What one destination receives from this plan.
    #[derive(Default)]
    struct Incoming {
        bytes: i64,
        same_filesystem_bytes: i64,
        items: usize,
    }
    let mut incoming: HashMap<(String, String), Incoming> = HashMap::new();

    for (index, media) in library.media.iter().enumerate() {
        evaluated.store(index, Ordering::Relaxed);
        let route = route(ctx, media, rules, now);

        match &route.evaluation.winner {
            Some(winner) if winner.rule_id == OVERRIDE_RULE_ID => counters.overrides_applied += 1,
            Some(_) => {}
            None => counters.no_category_match += 1,
        }
        counters.excluded_by_rule += route.evaluation.excluded.len();

        match (route.action, &route.target) {
            (DecisionAction::Move, Some(target)) => {
                counters.moves_required += 1;

                // Equal free space means one filesystem, where a move consumes nothing.
                let free_of = |path: &str| -> Option<i64> {
                    ctx.free_space
                        .get(&(media.instance_id.clone(), crate::paths::key(path)))
                        .copied()
                        .flatten()
                };
                let entry =
                    incoming.entry((media.instance_id.clone(), target.clone())).or_default();
                entry.items += 1;
                let size = media.size_on_disk.unwrap_or(0);
                let destination = free_of(target);
                let current = media.current_root_folder.as_deref().unwrap_or("");
                if destination.is_some() && destination == free_of(current) {
                    entry.same_filesystem_bytes += size;
                } else {
                    entry.bytes += size;
                }
            }
            (DecisionAction::None, _) => counters.already_correct += 1,
            _ => counters.skipped_unmapped += 1,
        }

        verdicts.push(Verdict {
            category: route.category.clone(),
            matched_rule_id: route.evaluation.winner.as_ref().map(|w| w.rule_id.clone()),
        });
        let Some(wording) = wording else { continue };
        if route.action == DecisionAction::None {
            if left_alone == wording.unchanged {
                continue;
            }
            left_alone += 1;
        }
        decisions.push(decide(ctx, media, route, wording, now));
    }
    evaluated.store(library.media.len(), Ordering::Relaxed);

    // A folder that cannot take what it is sent sorts first, to be seen before an apply.
    let mut capacity: Vec<CapacityForecast> = incoming
        .into_iter()
        .filter_map(|((instance_id, path), got)| {
            let free = ctx
                .free_space
                .get(&(instance_id.clone(), crate::paths::key(&path)))
                .copied()
                .flatten()?;
            Some(CapacityForecast {
                instance_name: ctx.instance_names.get(&instance_id).cloned(),
                instance_id,
                path,
                incoming_bytes: got.bytes,
                same_filesystem_bytes: got.same_filesystem_bytes,
                free_bytes: free,
                items: got.items,
                fits: got.bytes <= free,
            })
        })
        .collect();
    // Folders of one instance with equal free space are one volume, which must fit their sum.
    let mut volumes: HashMap<(String, i64), i64> = HashMap::new();
    for row in &capacity {
        *volumes.entry((row.instance_id.clone(), row.free_bytes)).or_default() +=
            row.incoming_bytes;
    }
    for row in &mut capacity {
        row.fits = volumes[&(row.instance_id.clone(), row.free_bytes)] <= row.free_bytes;
    }
    capacity
        .sort_by(|a, b| a.fits.cmp(&b.fits).then_with(|| b.incoming_bytes.cmp(&a.incoming_bytes)));

    Pass { verdicts, decisions, counters, capacity }
}

/// One title's route as the decision a run stores and returns, in words.
fn decide(
    ctx: &RoutingContext,
    media: &Media,
    route: Route,
    wording: &Wording,
    now: chrono::DateTime<Utc>,
) -> Decision {
    let localizer = &wording.localizer;
    let Route { evaluation, category, target, action, .. } = route;
    let is_override = evaluation.winner.as_ref().is_some_and(|w| w.rule_id == OVERRIDE_RULE_ID);
    let (matched_rule_id, matched_rule_name, told, confidence) = match &evaluation.winner {
        Some(m) if is_override => {
            (Some(m.rule_id.clone()), None, vec![Reason::ManualOverride], m.confidence)
        }
        Some(m) => {
            let mut told: Vec<Reason> =
                m.evaluations.iter().cloned().map(Reason::Condition).collect();
            told.extend(m.excluded_by.clone().map(Reason::Excluded));
            (Some(m.rule_id.clone()), Some(m.rule_name.clone()), told, m.confidence)
        }
        None => (
            None,
            None,
            vec![Reason::NoRuleMatched { category: ctx.default_category.clone() }],
            0.0,
        ),
    };
    let told_alternatives: Vec<AlternativeTold> = evaluation
        .alternatives
        .iter()
        .chain(&evaluation.excluded)
        .map(|m| AlternativeTold {
            rule_name: m.rule_name.clone(),
            category: m.category.clone(),
            evaluations: m.evaluations.clone(),
            excluded_by: m.excluded_by.clone(),
            confidence: m.confidence,
        })
        .collect();
    let reason_lines: Vec<ReasonLine> = told.iter().map(|reason| reason.line(localizer)).collect();
    let alternatives = told_alternatives.iter().map(|told| told.said(localizer)).collect();
    let stored_reasons = serde_json::to_string(&told).unwrap_or_default();
    let stored_alternatives = serde_json::to_string(&told_alternatives).unwrap_or_default();
    let matched_rule_name = match is_override {
        true => Some(localizer.translate("ManualOverrideRuleName", &[])),
        false => matched_rule_name,
    };

    Decision {
        actor: Some(wording.trigger.clone()),
        subject: wording.subject.clone(),
        subject_key: wording.subject_key.clone(),
        revertible: false,
        id: Uuid::new_v4().to_string(),
        media_id: media.id.clone(),
        media_title: media.title.clone(),
        media_type: media.media_type.clone(),
        instance_id: media.instance_id.clone(),
        instance_name: ctx.instance_names.get(&media.instance_id).cloned(),
        current_root_folder: media.current_root_folder.clone(),
        current_category: media.current_root_folder.as_deref().and_then(|folder| {
            let key = (media.instance_id.clone(), crate::paths::key(folder));
            ctx.folder_categories.get(&key).cloned()
        }),
        target_root_folder: target,
        target_category: category,
        matched_rule_id,
        matched_rule_name,
        is_override,
        reasons: reason_lines.iter().map(|line| line.text.clone()).collect(),
        reason_lines,
        alternatives,
        stored_reasons,
        stored_alternatives,
        action,
        status: DecisionStatus::Pending,
        confidence,
        superseded: false,
        simulation_id: Some(wording.simulation_id.clone()),
        error_message: None,
        decided_at: format_timestamp(now),
        applied_at: None,
        reverted_at: None,
    }
}

/// Where one item goes under a rule set, and why.
pub struct Route {
    /// What the rules read: every source's answer, merged in the configured order.
    pub metadata: Option<MediaMetadata>,
    pub evaluation: rule_engine::Evaluation,
    /// The winner's category, or the default one when no rule matched.
    pub category: String,
    /// The folder mapped to that category on the item's instance, if any.
    pub target: Option<String>,
    /// `move`, `none` when the item is already there, or `skip` when the
    /// category has no folder on the item's instance.
    pub action: DecisionAction,
}

/// Decide one item: its metadata, the rules, its override and the mappings.
/// The simulation, the apply, the rule report and the explanation all call it,
/// so they agree on where an item goes.
fn route(ctx: &RoutingContext, media: &Media, rules: &[Rule], now: chrono::DateTime<Utc>) -> Route {
    let metadata = resolve_metadata(
        media,
        &ctx.providers,
        &ctx.metadata,
        &ctx.identifiers,
        &ctx.regions,
        ctx.instance_countries.get(&media.instance_id).map(String::as_str),
    );
    let evaluation = rule_engine::evaluate_rules(
        EvalContext { media, metadata: metadata.as_ref(), now },
        rules,
        ctx.overrides.get(&media.id).map(String::as_str),
    );
    let category = evaluation
        .winner
        .as_ref()
        .map_or_else(|| ctx.default_category.clone(), |winner| winner.category.clone());
    let target = ctx.root_folders.get(&(media.instance_id.clone(), category.clone())).cloned();
    let action = match &target {
        None => DecisionAction::Skip,
        Some(target) => {
            let current = media.current_root_folder.as_deref().unwrap_or("");
            if crate::paths::key(current) == crate::paths::key(target) {
                DecisionAction::None
            } else {
                DecisionAction::Move
            }
        }
    };
    Route { metadata, evaluation, category, target, action }
}

/// One item decided as the simulation decides it, with the rules it read.
pub struct ItemRoute {
    /// Every stored rule, in priority order: the explanation traces each one.
    pub rules: Vec<Rule>,
    /// The category a person pinned the item to, if one did.
    pub override_category: Option<String>,
    pub route: Route,
}

/// Decide one item live, through `route`, from what the database holds now. It
/// loads only this item's context, so it takes no library-pass permit.
pub async fn route_one(
    pool: &SqlitePool,
    media: &Media,
    now: chrono::DateTime<Utc>,
) -> AppResult<ItemRoute> {
    route_one_with(pool, media, now, Fresh::default()).await
}

/// What sources asked about one title said just now, never stored: their
/// answers, and the id a source found by title resolved it to, without which
/// the evaluation cannot tell that source's answer belongs to the title.
#[derive(Default, Clone)]
pub struct Fresh {
    pub metadata: HashMap<(String, String, String), ProviderMetadata>,
    pub identifiers: metadata::Identifiers,
}

/// `route_one`, with answers a source gave just now where the cache has none.
/// Stores nothing, so a title the library does not hold fills no cache.
pub async fn route_one_with(
    pool: &SqlitePool,
    media: &Media,
    now: chrono::DateTime<Utc>,
    fresh: Fresh,
) -> AppResult<ItemRoute> {
    let mut snapshot = pool.begin().await?;
    let mut ctx = load_context(&mut snapshot, Scope::Items(std::slice::from_ref(media))).await?;
    snapshot.commit().await?;
    for (key, answer) in fresh.metadata {
        ctx.metadata.entry(key).or_insert(answer);
    }
    for (key, external) in fresh.identifiers {
        ctx.identifiers.entry(key).or_insert(external);
    }
    let route = route(&ctx, media, &ctx.rules, now);
    let override_category = ctx.overrides.get(&media.id).cloned();
    Ok(ItemRoute { rules: ctx.rules, override_category, route })
}

/// Where the rules and mappings as they stand now send the items an apply is
/// about to move, since a stored proposal may be stale. `None` for an item sent
/// nowhere, and no entry for an id with no media row.
pub async fn revalidated_targets(
    pool: &SqlitePool,
    media_ids: &[String],
) -> AppResult<HashMap<String, Option<String>>> {
    let mut snapshot = pool.begin().await?;
    let mut media = Vec::with_capacity(media_ids.len());
    for chunk in media_ids.chunks(BIND_CHUNK) {
        media.extend(load_media(&mut *snapshot, &[], Some(chunk), None).await?);
    }
    if media.is_empty() {
        return Ok(HashMap::new());
    }
    let ctx = load_context(&mut snapshot, Scope::Items(&media)).await?;
    snapshot.commit().await?;
    let now = Utc::now();
    Ok(media
        .into_iter()
        .map(|item| {
            let Route { target, .. } = route(&ctx, &item, &ctx.rules, now);
            (item.id, target)
        })
        .collect())
}

/// What a pass decided, counted.
#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    pub moves_required: usize,
    pub already_correct: usize,
    pub no_category_match: usize,
    pub overrides_applied: usize,
    pub skipped_unmapped: usize,
    pub excluded_by_rule: usize,
}

/// One line of a decision's justification, as a run stores it: said in the
/// language of whoever reads the decision, not in the run's.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "line", rename_all = "snake_case")]
pub enum Reason {
    Condition(rule_engine::ConditionOutcome),
    Excluded(rule_engine::ConditionOutcome),
    ManualOverride,
    NoRuleMatched { category: String },
}

impl Reason {
    fn line(&self, localizer: &Localizer) -> ReasonLine {
        let (text, matched) = match self {
            Reason::Condition(outcome) => (localizer.describe(outcome), outcome.matched),
            Reason::Excluded(veto) => (localizer.describe_exclusion(veto), false),
            Reason::ManualOverride => (localizer.translate("ReasonManualOverride", &[]), true),
            Reason::NoRuleMatched { category } => {
                (localizer.translate("ReasonNoRuleMatched", &[("category", category)]), false)
            }
        };
        ReasonLine { text, matched: Some(matched) }
    }
}

/// A rule that matched and lost, as a run stores it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlternativeTold {
    rule_name: String,
    category: String,
    evaluations: Vec<rule_engine::ConditionOutcome>,
    excluded_by: Option<rule_engine::ConditionOutcome>,
    confidence: f32,
}

impl AlternativeTold {
    fn said(&self, localizer: &Localizer) -> AlternativeDecision {
        AlternativeDecision {
            rule_name: self.rule_name.clone(),
            category: self.category.clone(),
            reason: localizer.describe_all(&self.evaluations, None).join(" · "),
            excluded_by: self
                .excluded_by
                .as_ref()
                .map(|veto| localizer.localize_outcome(veto.clone()).expected),
            confidence: self.confidence,
        }
    }
}

/// What a decision's two columns hold: values a run stores, or sentences,
/// which a row may hold and which are read as they are.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredReason {
    Told(Reason),
    Said(String),
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredAlternative {
    Told(AlternativeTold),
    Said(AlternativeDecision),
}

/// A stored decision's reasons, in `localizer`'s language.
pub fn stored_reasons(stored: &str, localizer: &Localizer) -> Vec<ReasonLine> {
    let read: Vec<StoredReason> = serde_json::from_str(stored).unwrap_or_default();
    read.into_iter()
        .map(|reason| match reason {
            StoredReason::Told(reason) => reason.line(localizer),
            StoredReason::Said(text) => ReasonLine { text, matched: None },
        })
        .collect()
}

/// A stored decision's alternatives, in `localizer`'s language.
pub fn stored_alternatives(stored: &str, localizer: &Localizer) -> Vec<AlternativeDecision> {
    let read: Vec<StoredAlternative> = serde_json::from_str(stored).unwrap_or_default();
    read.into_iter()
        .map(|alternative| match alternative {
            StoredAlternative::Told(told) => told.said(localizer),
            StoredAlternative::Said(said) => said,
        })
        .collect()
}

/// Two rule sets over one library, as the rule preview shows them.
pub struct Comparison {
    pub total_media: usize,
    pub before: Counters,
    pub after: Counters,
    /// The first titles the second set sends to another category than the
    /// first, each with the category it leaves, worded as the second decides.
    pub changed: Vec<(String, Decision)>,
    pub changed_total: usize,
}

/// Evaluate `before` and `after` over `library`, and word only the first
/// `sample` titles whose category differs.
pub async fn compare(
    library: &Arc<LoadedLibrary>,
    before: Vec<Rule>,
    after: Vec<Rule>,
    sample: usize,
    language: &str,
) -> AppResult<Comparison> {
    let now = Utc::now();
    let wording = Wording {
        localizer: Localizer::new(language),
        trigger: String::new(),
        subject: None,
        subject_key: None,
        simulation_id: String::new(),
        unchanged: 0,
    };
    let loaded = Arc::clone(library);
    off_the_workers(None, 0, move |evaluated| {
        let first = evaluate_pass(&loaded, &before, now, None, evaluated);
        let second = evaluate_pass(&loaded, &after, now, None, evaluated);
        let differing: Vec<usize> = (0..loaded.media.len())
            .filter(|&index| first.verdicts[index].category != second.verdicts[index].category)
            .collect();
        let changed = differing
            .iter()
            .take(sample)
            .map(|&index| {
                let media = &loaded.media[index];
                let route = route(&loaded.ctx, media, &after, now);
                (
                    first.verdicts[index].category.clone(),
                    decide(&loaded.ctx, media, route, &wording, now),
                )
            })
            .collect();
        Comparison {
            total_media: loaded.media.len(),
            before: first.counters,
            after: second.counters,
            changed,
            changed_total: differing.len(),
        }
    })
    .await
}

/// What the rules of `watched` would decide among one rule set.
#[derive(Debug, Default)]
pub struct Reach {
    pub titles: usize,
    pub to_move: usize,
    pub in_place: usize,
    /// Decided, but their category has no folder on their instance.
    pub unmapped: usize,
    /// The first titles they decide, those to move first, worded.
    pub sample: Vec<Decision>,
    /// Each rule outside `watched` winning titles a rule of `watched` matched
    /// too, with how many, most first.
    pub taken: Vec<(String, usize)>,
}

/// Evaluate `rules` over `library` once, and tell what its rules of `watched`
/// decide and what keeps them from deciding more.
pub async fn reach(
    library: &Arc<LoadedLibrary>,
    rules: Vec<Rule>,
    watched: Vec<String>,
    sample: usize,
    language: &str,
) -> AppResult<Reach> {
    let now = Utc::now();
    let wording = Wording {
        localizer: Localizer::new(language),
        trigger: String::new(),
        subject: None,
        subject_key: None,
        simulation_id: String::new(),
        unchanged: 0,
    };
    let loaded = Arc::clone(library);
    off_the_workers(None, 0, move |_| {
        let mut reach = Reach::default();
        let mut taken: HashMap<String, usize> = HashMap::new();
        // By what becomes of them, so the sample shows moves first.
        let mut kept: [Vec<(&Media, Route)>; 3] = Default::default();
        for media in &loaded.media {
            let route = route(&loaded.ctx, media, &rules, now);
            let Some(winner) = &route.evaluation.winner else { continue };
            if watched.contains(&winner.rule_id) {
                reach.titles += 1;
                let bucket = match route.action {
                    DecisionAction::Move => {
                        reach.to_move += 1;
                        0
                    }
                    DecisionAction::Skip => {
                        reach.unmapped += 1;
                        1
                    }
                    DecisionAction::None => {
                        reach.in_place += 1;
                        2
                    }
                };
                if kept[bucket].len() < sample {
                    kept[bucket].push((media, route));
                }
            } else if route
                .evaluation
                .alternatives
                .iter()
                .any(|lost| watched.contains(&lost.rule_id))
            {
                *taken.entry(winner.rule_id.clone()).or_default() += 1;
            }
        }
        reach.sample = kept
            .into_iter()
            .flatten()
            .take(sample)
            .map(|(media, route)| decide(&loaded.ctx, media, route, &wording, now))
            .collect();
        reach.taken = taken.into_iter().collect();
        reach.taken.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        reach
    })
    .await
}

/// What the engine decided about one item, in rule ids, since two rules may
/// share a name.
pub struct LibraryOutcome {
    pub winner: Option<String>,
    /// Matched and lost on priority.
    pub alternatives: Vec<String>,
    /// Matched and vetoed by one of the rule's own exclusions.
    pub excluded: Vec<String>,
}

/// Evaluate every item against the current rules and report which rule did
/// what, to be counted. Persists nothing and words nothing.
pub async fn evaluate_library(
    pool: &SqlitePool,
    passes: &LibraryPasses,
) -> AppResult<Vec<LibraryOutcome>> {
    let now = Utc::now();
    let library = load_library(pool, passes, &SimulationOptions::default()).await?;
    off_the_workers(None, 0, move |_| {
        let ctx = &library.ctx;
        library
            .media
            .iter()
            .map(|media| {
                // Through `route`, so a pinned item counts against no rule it never consulted.
                let evaluation = route(ctx, media, &ctx.rules, now).evaluation;
                LibraryOutcome {
                    winner: evaluation.winner.map(|m| m.rule_id),
                    alternatives: evaluation.alternatives.into_iter().map(|m| m.rule_id).collect(),
                    excluded: evaluation.excluded.into_iter().map(|m| m.rule_id).collect(),
                }
            })
            .collect()
    })
    .await
}

pub fn format_timestamp(ts: chrono::DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// The inverse of `format_timestamp`, also reading RFC 3339 and a bare date as
/// an Arr writes them. `None` when unreadable, so a caller never falls back to
/// today's clock unawares.
pub fn parse_timestamp(raw: &str) -> Option<chrono::DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(stamp) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(stamp.with_timezone(&Utc));
    }
    for shape in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, shape) {
            return Some(naive.and_utc());
        }
    }
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc())
}

/// Which items a context is loaded for.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// Every item: a simulation, a rule report.
    Library,
    /// The items named: the explanation panel, the webhook's one title and
    /// an apply's slice, none of which loads the whole metadata cache.
    Items(&'a [Media]),
}

/// Load rules, overrides, mappings and metadata in one statement per table
/// whatever the scope, the items' ids bound in chunks. A single item's context
/// is narrowed to its instance.
async fn load_context(
    snapshot: &mut sqlx::SqliteConnection,
    scope: Scope<'_>,
) -> AppResult<RoutingContext> {
    let rules = load_rules(&mut *snapshot).await?;
    crate::race::checked("routing::load", rules.first().map_or("", |rule| rule.id.as_str())).await;
    let instance_id = match scope {
        Scope::Items([media]) => Some(media.instance_id.as_str()),
        _ => None,
    };

    let overrides: HashMap<String, String> = match scope {
        Scope::Library => {
            sqlx::query_as::<_, (String, String)>("SELECT media_id, target_category FROM overrides")
                .fetch_all(&mut *snapshot)
                .await?
                .into_iter()
                .collect()
        }
        Scope::Items(media) => {
            let mut overrides = HashMap::new();
            for chunk in media.chunks(BIND_CHUNK) {
                let sql = format!(
                    "SELECT media_id, target_category FROM overrides WHERE media_id IN ({})",
                    crate::db::placeholders(chunk.len())
                );
                let mut query = sqlx::query_as::<_, (String, String)>(AssertSqlSafe(sql.as_str()));
                for item in chunk {
                    query = query.bind(&item.id);
                }
                overrides.extend(query.fetch_all(&mut *snapshot).await?);
            }
            overrides
        }
    };

    let mapped: Vec<(String, String, String)> = sqlx::query_as(
        // Reachable or not, or a folder on a sleeping NAS reads as unmapped.
        "SELECT instance_id, category, path FROM root_folders
         WHERE category IS NOT NULL AND category != ''
           AND (? IS NULL OR instance_id = ?)",
    )
    .bind(instance_id)
    .bind(instance_id)
    .fetch_all(&mut *snapshot)
    .await?;
    let folder_categories: HashMap<(String, String), String> = mapped
        .iter()
        .map(|(instance_id, category, path)| {
            ((instance_id.clone(), crate::paths::key(path)), category.clone())
        })
        .collect();
    let root_folders: HashMap<(String, String), String> = mapped
        .into_iter()
        .map(|(instance_id, category, path)| ((instance_id, category), path))
        .collect();

    let free_space: HashMap<(String, String), Option<i64>> =
        sqlx::query_as::<_, (String, String, Option<i64>)>(
            "SELECT instance_id, path, free_space FROM root_folders
              WHERE ? IS NULL OR instance_id = ?",
        )
        .bind(instance_id)
        .bind(instance_id)
        .fetch_all(&mut *snapshot)
        .await?
        .into_iter()
        .map(|(instance_id, path, free)| ((instance_id, crate::paths::key(&path)), free))
        .collect();

    let instances: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, certification_country FROM instances WHERE ? IS NULL OR id = ?",
    )
    .bind(instance_id)
    .bind(instance_id)
    .fetch_all(&mut *snapshot)
    .await?;
    let instance_countries: HashMap<String, String> = instances
        .iter()
        .filter_map(|(id, _, country)| Some((id.clone(), country.clone()?)))
        .collect();
    let instance_names: HashMap<String, String> =
        instances.into_iter().map(|(id, name, _)| (id, name)).collect();
    let regions_setting: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'certification_regions'")
            .fetch_optional(&mut *snapshot)
            .await?;
    let regions = crate::state::AppState::certification_regions_of(regions_setting.as_deref());

    // The configured order whatever the keys, so a cached answer outlives its key.
    let providers_setting: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'metadata_providers'")
            .fetch_optional(&mut *snapshot)
            .await?;
    let providers = metadata::configured_order(providers_setting.as_deref());

    let (identifiers, metadata) = match scope {
        Scope::Library => (
            metadata::load_identifiers(&mut *snapshot).await?,
            metadata::load_cache(&mut *snapshot).await?,
        ),
        Scope::Items(media) => {
            let identifiers = metadata::load_identifiers_of(&mut *snapshot, media).await?;
            let cache =
                metadata::load_cache_of(&mut *snapshot, media, &providers, &identifiers).await?;
            (identifiers, cache)
        }
    };

    let default_category = crate::state::AppState::default_category(&mut *snapshot).await?;

    Ok(RoutingContext {
        rules,
        overrides,
        root_folders,
        folder_categories,
        free_space,
        instance_names,
        instance_countries,
        regions,
        providers,
        metadata,
        identifiers,
        default_category,
    })
}

/// Collapse every enabled source's answer for one item, the first source with a
/// value keeping each field. The Arr's English original language comes last, and
/// a fetched source counts only when the cache holds it under the item's id.
pub(crate) fn resolve_metadata(
    media: &Media,
    providers: &[&'static ProviderInfo],
    cache: &HashMap<(String, String, String), ProviderMetadata>,
    identifiers: &metadata::Identifiers,
    regions: &[String],
    arr_country: Option<&str>,
) -> Option<MediaMetadata> {
    let mut parts: Vec<(&str, ProviderMetadata)> = Vec::with_capacity(providers.len() + 1);
    let mut arrs_english = None;

    for provider in providers {
        if provider.id == metadata::ARR {
            let mut from_arr = metadata::from_media(media);
            from_arr.certification_scale =
                from_arr.certification.as_ref().and(arr_country.map(str::to_string));
            // An Arr says English for any language it does not list, so its English goes last.
            if from_arr.original_language.as_deref() == Some("en") {
                arrs_english = from_arr.original_language.take();
            }
            parts.push((provider.id, from_arr));
            continue;
        }

        let Some(external_id) = metadata::external_id(provider, media, identifiers) else {
            continue;
        };
        let key = (provider.id.to_string(), external_id, media.media_type.clone());
        if let Some(cached) = cache.get(&key) {
            parts.push((provider.id, cached.clone()));
        }
    }
    if arrs_english.is_some() {
        parts.push((
            metadata::ARR,
            ProviderMetadata { original_language: arrs_english, ..Default::default() },
        ));
    }

    keep_the_regions_rating(&mut parts, regions);
    MediaMetadata::merge(parts)
}

/// Leave one rating among `parts`: the first certification region's, whichever
/// source gave it, else the first source's. A blank rating claims nothing, or
/// the Arr's empty one would erase every source's.
fn keep_the_regions_rating(parts: &mut [(&str, ProviderMetadata)], regions: &[String]) {
    for (_, part) in parts.iter_mut().filter(|(_, part)| part.certification.is_none()) {
        (part.certification, part.certification_scale) =
            metadata::rating_for(&part.certifications, regions).unzip();
    }
    let rank = |part: &ProviderMetadata| {
        part.certification_scale
            .as_deref()
            .and_then(|scale| regions.iter().position(|region| region.eq_ignore_ascii_case(scale)))
            .unwrap_or(regions.len())
    };
    let kept = parts
        .iter()
        .enumerate()
        .filter(|(_, (_, part))| {
            part.certification.as_deref().is_some_and(|c| !c.trim().is_empty())
        })
        .min_by_key(|(order, (_, part))| (rank(part), *order))
        .map(|(order, _)| order);
    for (order, (_, part)) in parts.iter_mut().enumerate() {
        if Some(order) != kept {
            part.certification = None;
            part.certification_scale = None;
        }
    }
}

/// A row of `rules`, read by column name: read by position, a column added
/// between two of the same type would land in the wrong field.
#[derive(sqlx::FromRow)]
pub struct RuleRecord {
    id: String,
    name: String,
    description: Option<String>,
    priority: i64,
    enabled: bool,
    media_type: String,
    conditions: String,
    target_category: String,
    instance_ids: Option<String>,
    created_at: String,
    updated_at: String,
    match_mode: String,
    exclusions: String,
    template: Option<String>,
}

/// The gap a reorder leaves between two rules, and a new rule after the last.
pub const PRIORITY_STEP: i64 = 10;

pub const RULE_COLUMNS: &str = "id, name, description, priority, enabled, media_type, conditions,
     target_category, instance_ids, created_at, updated_at, match_mode, exclusions, template";

/// Every column `Media` expects, in one place: `FromRow` maps by name, so a
/// query missing one fails at run time, on that path alone.
pub const MEDIA_COLUMNS: &str = "id, instance_id, arr_id, media_type, title, sort_title, year,
     tmdb_id, tvdb_id, imdb_id, current_path, current_root_folder, monitored, has_files,
     status, added_at, series_type, size_on_disk, season_count, tags, genres,
     original_language, certification, quality_profile, runtime, studio, network, collection,
     collection_tmdb_id, ratings, last_synced_at";

/// The library rows of a title another service names, on one instance or all.
pub async fn media_by_external_id(
    pool: &SqlitePool,
    media_type: &str,
    id: &ExternalId,
    instance_id: Option<&str>,
) -> AppResult<Vec<Media>> {
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {MEDIA_COLUMNS} FROM media
          WHERE media_type = ? AND {} = ? AND (? IS NULL OR instance_id = ?)
          ORDER BY instance_id, id",
        id.column()
    )))
    .bind(media_type)
    .bind(id.value())
    .bind(instance_id)
    .bind(instance_id)
    .fetch_all(pool)
    .await?)
}

/// Load every rule, in the order the engine tries them (`rule_engine::in_order`),
/// tolerating rows whose JSON payload got corrupted.
pub async fn load_rules<'e, E>(executor: E) -> AppResult<Vec<Rule>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows: Vec<RuleRecord> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM rules ORDER BY priority, name, id"
    )))
    .fetch_all(executor)
    .await?;

    Ok(rows.into_iter().map(rule_from_row).collect())
}

pub fn rule_from_row(r: RuleRecord) -> Rule {
    // An unreadable part makes the rule match nothing, never widens what it reaches.
    let unreadable = |part: &str, e: serde_json::Error| {
        tracing::warn!(rule_id = %r.id, rule = %r.name, "The rule matches nothing: its {part} cannot be read: {e}");
    };
    let mut conditions: Vec<Condition> = serde_json::from_str(&r.conditions).unwrap_or_else(|e| {
        unreadable("conditions", e);
        Vec::new()
    });
    let exclusions: Vec<Condition> = if r.exclusions.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&r.exclusions).unwrap_or_else(|e| {
            unreadable("exclusions", e);
            conditions.clear();
            Vec::new()
        })
    };
    let instance_ids: Option<Vec<String>> =
        match r.instance_ids.as_deref().filter(|s| !s.trim().is_empty()) {
            None => None,
            Some(raw) => serde_json::from_str(raw).unwrap_or_else(|e| {
                unreadable("instance list", e);
                conditions.clear();
                None
            }),
        };
    let match_mode: MatchMode = r.match_mode.parse().unwrap_or_default();
    let media_type = r.media_type.parse().unwrap_or_else(|e: String| {
        tracing::warn!(rule_id = %r.id, rule = %r.name, "The rule matches nothing: {e}");
        conditions.clear();
        RuleMediaType::Both
    });

    Rule {
        id: r.id,
        name: r.name,
        description: r.description,
        priority: r.priority,
        enabled: r.enabled,
        media_type,
        conditions,
        exclusions,
        match_mode,
        target_category: r.target_category,
        instance_ids,
        template: r.template,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// Load media, filtering in SQL. An empty `instance_filter` is every instance, as
/// `Rule::covers_instance` reads it, and an empty `media_ids` is no item at all.
async fn load_media<'e, E>(
    executor: E,
    instance_filter: &[String],
    media_ids: Option<&[String]>,
    media_type: Option<&str>,
) -> AppResult<Vec<Media>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    // A switched-off instance is neither synced nor routed.
    let mut sql = String::from(&format!(
        "SELECT {MEDIA_COLUMNS} FROM media
          WHERE instance_id IN (SELECT id FROM instances WHERE enabled = 1)"
    ));

    if media_type.is_some() {
        sql.push_str(" AND media_type = ?");
    }
    if !instance_filter.is_empty() {
        sql.push_str(" AND instance_id IN (");
        sql.push_str(&crate::db::placeholders(instance_filter.len()));
        sql.push(')');
    }
    // Nothing, or a webhook for a deleted item would apply the whole instance.
    let media_filter: &[String] = match media_ids {
        None => &[],
        Some([]) => {
            sql.push_str(" AND 1 = 0");
            &[]
        }
        Some(ids) => {
            sql.push_str(" AND id IN (");
            sql.push_str(&crate::db::placeholders(ids.len()));
            sql.push(')');
            ids
        }
    };
    sql.push_str(" ORDER BY title ASC");

    let mut query = sqlx::query_as::<_, Media>(AssertSqlSafe(sql.as_str()));
    if let Some(mt) = media_type {
        query = query.bind(mt.to_string());
    }
    for id in instance_filter.iter().chain(media_filter) {
        query = query.bind(id.clone());
    }

    Ok(query.fetch_all(executor).await?)
}

/// How many whole-library passes one state runs at once, each holding the cache
/// and every media row. A third waits, since a refusal would be a 409 on a page
/// that only wanted to render.
pub const MAX_CONCURRENT_LIBRARY_PASSES: usize = 2;

/// The permits the whole-library passes of one state share. Held by the state
/// rather than the process, so two states never wait on each other.
#[derive(Debug, Clone)]
pub struct LibraryPasses(Arc<tokio::sync::Semaphore>);

impl Default for LibraryPasses {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_LIBRARY_PASSES)))
    }
}

impl LibraryPasses {
    /// The permit one whole-library pass holds while it runs, public so a test
    /// can hold every permit.
    pub async fn take(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(&self.0)
            .acquire_owned()
            .await
            .expect("the library-pass semaphore is never closed")
    }
}

/// How many ids one statement binds, far under SQLite's cap on bound parameters.
pub const BIND_CHUNK: usize = 400;

/// Retire every pending proposal for these media, in place, once a run, a sync,
/// a delete or an override has made them stale.
pub async fn supersede_pending(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    media_ids: &[&str],
) -> AppResult<()> {
    retire_pending(tx, "media_id", media_ids).await
}

/// Retire these pending proposals, and only these: a simulation may have written
/// the item's next proposal since the apply read this one.
pub async fn supersede_decisions(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    decision_ids: &[&str],
) -> AppResult<()> {
    retire_pending(tx, "id", decision_ids).await
}

/// Retire a deleted instance's pending proposals, which would move titles that are gone.
/// What was applied stays.
pub async fn supersede_instance_decisions(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    instance_id: &str,
) -> AppResult<()> {
    retire_pending(tx, "instance_id", &[instance_id]).await
}

/// The one statement that writes `superseded`, whichever list it is given,
/// so a change to its chunking or its filter reaches every caller.
async fn retire_pending(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    column: &'static str,
    ids: &[&str],
) -> AppResult<()> {
    for chunk in ids.chunks(BIND_CHUNK) {
        let placeholders = crate::db::placeholders(chunk.len());
        let sql = format!(
            "UPDATE decisions SET superseded = 1
             WHERE status = 'pending' AND superseded = 0 AND {column} IN ({placeholders})"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql.as_str()));
        for id in chunk {
            q = q.bind(*id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Persist a run: what it decided for every title, then its proposals in place
/// of the pending ones. A title is skipped when its row went after the read, or
/// when a run that read the library later stored it first.
async fn store_run(
    pool: &SqlitePool,
    library: &LoadedLibrary,
    trigger: &str,
    simulation_id: &str,
    now: chrono::DateTime<Utc>,
    verdicts: &[Verdict],
    decisions: &[&Decision],
) -> AppResult<()> {
    if verdicts.is_empty() {
        return Ok(());
    }
    // One statement for every title, bound as a JSON array, whatever the library's size.
    let rows: Vec<serde_json::Value> = library
        .media
        .iter()
        .zip(verdicts)
        .map(|(media, verdict)| {
            serde_json::json!([media.id, verdict.category, verdict.matched_rule_id])
        })
        .collect();
    let rows = serde_json::to_string(&rows)?;

    crate::race::checked("routing::store", trigger).await;
    let mut tx = crate::db::write_transaction(pool).await?;
    sqlx::query(
        "INSERT INTO media_routing (media_id, category, matched_rule_id, load_order,
                                    simulation_id, evaluated_at)
         SELECT j.value ->> 0, j.value ->> 1, j.value ->> 2, ?, ?, ?
           FROM json_each(?) j
          WHERE EXISTS (SELECT 1 FROM media m WHERE m.id = j.value ->> 0)
         ON CONFLICT(media_id) DO UPDATE SET
            category = excluded.category,
            matched_rule_id = excluded.matched_rule_id,
            load_order = excluded.load_order,
            simulation_id = excluded.simulation_id,
            evaluated_at = excluded.evaluated_at
          WHERE excluded.load_order > media_routing.load_order",
    )
    .bind(library.order)
    .bind(simulation_id)
    .bind(format_timestamp(now))
    .bind(&rows)
    .execute(&mut *tx)
    .await?;

    // Only the titles this run decided: its proposals for the others are already wrong.
    let ours: HashSet<String> =
        sqlx::query_scalar("SELECT media_id FROM media_routing WHERE load_order = ?")
            .bind(library.order)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let media_ids: Vec<&str> = ours.iter().map(String::as_str).collect();
    supersede_pending(&mut tx, &media_ids).await?;

    for decision in decisions.iter().filter(|d| ours.contains(&d.media_id)) {
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id, instance_name,
             current_root_folder, target_root_folder, target_category, matched_rule_id, matched_rule_name,
             is_override, reasons, alternatives, action, status, decided_at, confidence,
             simulation_id, actor, subject, subject_key)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&decision.id)
        .bind(&decision.media_id)
        .bind(&decision.media_title)
        .bind(&decision.media_type)
        .bind(&decision.instance_id)
        .bind(&decision.instance_name)
        .bind(&decision.current_root_folder)
        .bind(&decision.target_root_folder)
        .bind(&decision.target_category)
        .bind(&decision.matched_rule_id)
        // An exception's name is the reader's word for it (`api::decisions`).
        .bind(decision.matched_rule_name.as_ref().filter(|_| !decision.is_override))
        .bind(decision.is_override)
        .bind(&decision.stored_reasons)
        .bind(&decision.stored_alternatives)
        .bind(decision.action)
        .bind(decision.status)
        .bind(&decision.decided_at)
        .bind(decision.confidence)
        .bind(&decision.simulation_id)
        .bind(&decision.actor)
        .bind(&decision.subject)
        .bind(&decision.subject_key)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
