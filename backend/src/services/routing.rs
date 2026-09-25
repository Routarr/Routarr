//! Simulation: turn the current library state into explainable routing decisions.
//!
//! The whole run is built around preloading. The first implementation issued a
//! handful of queries *per media item* (metadata, instance name, default
//! category, then one INSERT); on a 5 000-item library that is >20 000 round
//! trips. Everything is now fetched in a fixed number of queries and the
//! decisions are written in a single transaction.

use chrono::Utc;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};
use tracing::info;
use uuid::Uuid;

use crate::error::AppResult;
use crate::localization::Localizer;
use crate::models::*;
use crate::services::metadata::{self, ProviderInfo};
use crate::services::rule_engine::{
    self, EvalContext, OVERRIDE_RULE_ID, RuleMatch, normalize_path,
};

/// Knobs for a simulation run.
#[derive(Debug, Clone)]
pub struct SimulationOptions {
    /// What caused this run, stored on every decision it writes.
    ///
    /// One of the `TRIGGER_*` constants. It is the history screen's answer to
    /// "did the nightly sweep propose this, or did I"; `subject` is its answer
    /// to which "I".
    pub trigger: String,
    /// The name the authentication mode vouched for, when it vouched for one.
    ///
    /// `None` for the scheduler and the webhook, which nobody signed in to run,
    /// and for the modes that name nobody. See `Identity::actor`.
    pub subject: Option<String>,
    pub instance_ids: Vec<String>,
    /// Evaluate only these media rows. Used by the webhook path, where
    /// re-evaluating the whole library on every import event would mean one full
    /// simulation per episode of a season.
    pub media_ids: Option<Vec<String>>,
    pub media_type: Option<String>,
    /// Persist the decisions so they can be applied and audited.
    pub persist: bool,
    /// Also persist decisions for media already in the right place. Off by
    /// default: on a large library that is thousands of rows saying "nothing
    /// to do" on every run.
    pub persist_unchanged: bool,
    /// Evaluate this rule set instead of the stored one (impact preview).
    pub rules_override: Option<Vec<Rule>>,
    /// Cap the number of decisions returned in the response payload.
    pub max_returned: Option<usize>,
    /// Language the stored explanations are written in.
    pub language: String,
}

impl Default for SimulationOptions {
    /// A run nobody attributed is a manual one: the scheduler and the webhook
    /// both name themselves, and nothing else runs a simulation unprompted.
    fn default() -> Self {
        Self {
            trigger: crate::jobs::TRIGGER_MANUAL.to_string(),
            subject: None,
            instance_ids: Vec::new(),
            media_ids: None,
            media_type: None,
            persist: false,
            persist_unchanged: false,
            rules_override: None,
            max_returned: None,
            language: String::new(),
        }
    }
}

/// Everything needed to evaluate a library, loaded once per run.
struct RoutingContext {
    rules: Vec<Rule>,
    overrides: HashMap<String, String>,
    /// (instance_id, category) -> root folder path
    root_folders: HashMap<(String, String), String>,
    /// (instance_id, **normalised** path) -> free bytes the Arr last reported.
    ///
    /// Loaded with the mapping above rather than queried per decision, like
    /// everything else here. Normalised because the source path arrives from
    /// the Arr's payload and the destination from this table, and a trailing
    /// slash on one of them would make every item look like a cross-filesystem
    /// move. `None` where the Arr reported no figure.
    free_space: HashMap<(String, String), Option<i64>>,
    instance_names: HashMap<String, String>,
    /// Enabled metadata sources, highest priority first.
    providers: Vec<&'static ProviderInfo>,
    /// (source, external id, media type) -> that source's answer.
    metadata: HashMap<(String, String, String), ProviderMetadata>,
    /// What the searching sources resolved each item to, loaded once like
    /// everything else here: a lookup per item and per source is the shape this
    /// module exists to avoid.
    identifiers: metadata::Identifiers,
    default_category: String,
}

/// Everything one pass over the library reads, loaded once and held with the
/// permit that bounds how many such loads run at once.
///
/// A preview evaluates two rule sets over the same library, and a health
/// report evaluates the current one: loaded apart from the evaluating, each
/// of them costs one load however many rule sets it asks about.
pub struct LoadedLibrary {
    _pass: tokio::sync::SemaphorePermit<'static>,
    /// What the load took, counted into every evaluation over it: a run's
    /// figure is load plus evaluation, whichever of the two ran it.
    loaded_in: std::time::Duration,
    ctx: RoutingContext,
    media: Vec<Media>,
}

/// Load the context and the media a pass with these options evaluates.
pub async fn load_library(
    pool: &SqlitePool,
    options: &SimulationOptions,
) -> AppResult<LoadedLibrary> {
    let started = std::time::Instant::now();

    // A filter longer than one statement binds is a request no library can
    // satisfy, and refused as one rather than failed inside the query.
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

    let pass = library_pass().await;
    let ctx = load_context(pool).await?;
    let media = load_media(
        pool,
        &options.instance_ids,
        options.media_ids.as_deref(),
        options.media_type.as_deref(),
    )
    .await?;

    Ok(LoadedLibrary { _pass: pass, loaded_in: started.elapsed(), ctx, media })
}

/// Run a full simulation and, when asked, persist the resulting decisions.
pub async fn run_simulation(
    pool: &SqlitePool,
    options: SimulationOptions,
) -> AppResult<SimulationResult> {
    let library = load_library(pool, &options).await?;
    simulate_loaded(pool, &library, options).await
}

/// Evaluate a loaded library under these options, and persist when asked.
///
/// Reaches the database only to persist: everything the evaluation reads is
/// in `library`, so a second evaluation over the same load costs no query.
pub async fn simulate_loaded(
    pool: &SqlitePool,
    library: &LoadedLibrary,
    options: SimulationOptions,
) -> AppResult<SimulationResult> {
    let started = std::time::Instant::now();
    let simulation_id = Uuid::new_v4().to_string();
    let now = Utc::now();

    let localizer = Localizer::new(&options.language);
    let ctx = &library.ctx;
    let media_list = &library.media;
    let rules: &[Rule] = options.rules_override.as_deref().unwrap_or(&ctx.rules);

    let mut decisions = Vec::with_capacity(media_list.len());
    let mut summary = Counters::default();

    /// What one destination receives from this plan.
    #[derive(Default)]
    struct Incoming {
        bytes: i64,
        same_filesystem_bytes: i64,
        items: usize,
    }
    let mut incoming: HashMap<(String, String), Incoming> = HashMap::new();

    for media in media_list {
        let Route { evaluation, category: target_category, target: target_root_folder } =
            route(ctx, media, rules, now);

        let is_override = evaluation.winner.as_ref().is_some_and(|w| w.rule_id == OVERRIDE_RULE_ID);
        if is_override {
            summary.overrides_applied += 1;
        }

        let (matched_rule_id, matched_rule_name, reasons, confidence) =
            match &evaluation.winner {
                Some(m) if is_override => (
                    Some(m.rule_id.clone()),
                    Some(localizer.translate("ManualOverrideRuleName", &[])),
                    vec![localizer.translate("ReasonManualOverride", &[])],
                    m.confidence,
                ),
                Some(m) => (
                    Some(m.rule_id.clone()),
                    Some(m.rule_name.clone()),
                    localizer.describe_all(&m.evaluations, m.excluded_by.as_ref()),
                    m.confidence,
                ),
                None => {
                    summary.no_category_match += 1;
                    (
                        None,
                        None,
                        vec![localizer.translate(
                            "ReasonNoRuleMatched",
                            &[("category", &ctx.default_category)],
                        )],
                        0.0,
                    )
                }
            };

        let action = match &target_root_folder {
            Some(target) => {
                let current = media.current_root_folder.as_deref().unwrap_or("");
                if normalize_path(current) == normalize_path(target) {
                    summary.already_correct += 1;
                    "none"
                } else {
                    summary.moves_required += 1;

                    // Weigh the plan as it is built. A move between two folders
                    // that report the *same* free space is a rename on one
                    // filesystem and consumes nothing; only what crosses one
                    // has to fit. Identical byte-level figures are strong
                    // evidence of the same volume, and the alternative — asking
                    // the Arr, which does not tell us — is no alternative.
                    let free_of = |path: &str| -> Option<i64> {
                        ctx.free_space
                            .get(&(media.instance_id.clone(), normalize_path(path)))
                            .copied()
                            .flatten()
                    };
                    let entry =
                        incoming.entry((media.instance_id.clone(), target.clone())).or_default();
                    entry.items += 1;
                    let size = media.size_on_disk.unwrap_or(0);
                    let destination = free_of(target);
                    if destination.is_some() && destination == free_of(current) {
                        entry.same_filesystem_bytes += size;
                    } else {
                        entry.bytes += size;
                    }

                    "move"
                }
            }
            None => {
                summary.skipped_unmapped += 1;
                "skip"
            }
        };

        let mut alternatives: Vec<AlternativeDecision> =
            evaluation.alternatives.iter().map(|m| to_alternative(m, &localizer)).collect();
        alternatives.extend(evaluation.excluded.iter().map(|m| to_alternative(m, &localizer)));
        summary.excluded_by_rule += evaluation.excluded.len();

        decisions.push(Decision {
            actor: Some(options.trigger.clone()),
            subject: options.subject.clone(),
            id: Uuid::new_v4().to_string(),
            media_id: media.id.clone(),
            media_title: media.title.clone(),
            media_type: media.media_type.clone(),
            instance_id: media.instance_id.clone(),
            instance_name: ctx.instance_names.get(&media.instance_id).cloned(),
            current_root_folder: media.current_root_folder.clone(),
            target_root_folder,
            target_category,
            matched_rule_id,
            matched_rule_name,
            is_override,
            reasons,
            alternatives,
            action: action.to_string(),
            status: "pending".to_string(),
            confidence,
            superseded: false,
            simulation_id: Some(simulation_id.clone()),
            error_message: None,
            decided_at: format_timestamp(now),
            applied_at: None,
            reverted_at: None,
        });
    }

    if options.persist {
        let to_store: Vec<&Decision> =
            decisions.iter().filter(|d| options.persist_unchanged || d.action != "none").collect();
        // Supersede over every media the run *evaluated*, not only those whose
        // decision is stored: an item that became "nothing to do" drops out of
        // the batch, and its previous proposal would otherwise stay pending —
        // and remain applicable long after the rules stopped justifying it.
        let evaluated: Vec<&str> = media_list.iter().map(|m| m.id.as_str()).collect();
        store_decisions(pool, &evaluated, &to_store).await?;
    }

    info!(
        simulation_id = %simulation_id,
        total = media_list.len(),
        moves = summary.moves_required,
        correct = summary.already_correct,
        unmapped = summary.skipped_unmapped,
        unmatched = summary.no_category_match,
        overrides = summary.overrides_applied,
        elapsed_ms = (library.loaded_in + started.elapsed()).as_millis() as u64,
        "Simulation complete"
    );

    let total_media = media_list.len();
    if let Some(max) = options.max_returned {
        // Keep the actionable ones when the payload has to be trimmed.
        decisions.sort_by_key(|d| match d.action.as_str() {
            "move" => 0,
            "skip" => 1,
            _ => 2,
        });
        decisions.truncate(max);
    }

    // Sorted so a folder that cannot take what it is being sent comes first:
    // this list exists to be read at a glance before anything is applied.
    let mut capacity: Vec<CapacityForecast> = incoming
        .into_iter()
        .filter_map(|((instance_id, path), got)| {
            let free = ctx
                .free_space
                .get(&(instance_id.clone(), normalize_path(&path)))
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
    capacity
        .sort_by(|a, b| a.fits.cmp(&b.fits).then_with(|| b.incoming_bytes.cmp(&a.incoming_bytes)));

    Ok(SimulationResult {
        capacity,
        simulation_id,
        total_media,
        returned: decisions.len(),
        decisions,
        moves_required: summary.moves_required,
        already_correct: summary.already_correct,
        no_category_match: summary.no_category_match,
        overrides_applied: summary.overrides_applied,
        skipped_unmapped: summary.skipped_unmapped,
        excluded_by_rule: summary.excluded_by_rule,
        elapsed_ms: (library.loaded_in + started.elapsed()).as_millis() as u64,
    })
}

/// Where one item goes under a rule set, and why.
struct Route {
    evaluation: rule_engine::Evaluation,
    /// The winner's category, or the default one when no rule matched.
    category: String,
    /// The folder mapped to that category on the item's instance, if any.
    target: Option<String>,
}

/// Decide one item: its metadata, the rules, its override and the mappings.
///
/// Shared by the simulation and by the revalidation at apply time, which have
/// to agree on where an item goes: a second spelling of it would retire a
/// proposal the simulation still makes, or apply one it no longer makes.
fn route(ctx: &RoutingContext, media: &Media, rules: &[Rule], now: chrono::DateTime<Utc>) -> Route {
    let metadata = resolve_metadata(media, &ctx.providers, &ctx.metadata, &ctx.identifiers);
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
    Route { evaluation, category, target }
}

/// Where the rules and mappings as they stand now send each of these items.
///
/// `None` for an item they send nowhere, its category having no folder on its
/// instance, and no entry for an id with no media row. This is what an apply
/// checks a proposal against: a proposal records what the rules said when the
/// simulation ran, and nothing retires it when a rule, a mapping or the
/// metadata changes afterwards.
///
/// Holds a library-pass permit, since it loads the whole metadata cache, which
/// is what that bound exists for.
pub async fn current_targets(
    pool: &SqlitePool,
    media_ids: &[String],
) -> AppResult<HashMap<String, Option<String>>> {
    let mut targets = HashMap::with_capacity(media_ids.len());
    if media_ids.is_empty() {
        return Ok(targets);
    }

    let _pass = library_pass().await;
    let ctx = load_context(pool).await?;
    let now = Utc::now();
    for chunk in media_ids.chunks(BIND_CHUNK) {
        for media in load_media(pool, &[], Some(chunk), None).await? {
            let Route { target, .. } = route(&ctx, &media, &ctx.rules, now);
            targets.insert(media.id, target);
        }
    }
    Ok(targets)
}

#[derive(Default)]
struct Counters {
    moves_required: usize,
    already_correct: usize,
    no_category_match: usize,
    overrides_applied: usize,
    skipped_unmapped: usize,
    excluded_by_rule: usize,
}

fn to_alternative(m: &RuleMatch, localizer: &Localizer) -> AlternativeDecision {
    AlternativeDecision {
        rule_name: m.rule_name.clone(),
        category: m.category.clone(),
        reason: localizer.describe_all(&m.evaluations, None).join(" · "),
        excluded_by: m
            .excluded_by
            .as_ref()
            .map(|veto| localizer.localize_outcome(veto.clone()).expected),
        confidence: m.confidence,
    }
}

/// What the engine decided about one item, in rule ids alone.
///
/// The decision rows carry rule *names*, which are not unique — the engine
/// breaks ties on name precisely because two rules may share one — so anything
/// counting per rule has to work from ids.
pub struct LibraryOutcome {
    pub winner: Option<String>,
    /// Matched and lost on priority.
    pub alternatives: Vec<String>,
    /// Matched and vetoed by one of the rule's own exclusions.
    pub excluded: Vec<String>,
}

/// Evaluate every item against the current rules and report which rule did
/// what. Persists nothing, renders no prose, and takes no localizer: it exists
/// to be counted, not read.
pub async fn evaluate_library(pool: &SqlitePool) -> AppResult<Vec<LibraryOutcome>> {
    let now = Utc::now();
    let library = load_library(pool, &SimulationOptions::default()).await?;
    let ctx = &library.ctx;

    Ok(library
        .media
        .iter()
        .map(|media| {
            let metadata = resolve_metadata(media, &ctx.providers, &ctx.metadata, &ctx.identifiers);
            // Overrides are passed through, because a pinned item genuinely is
            // decided by a human and counting it against a rule would say the
            // rule lost when it was never consulted.
            let evaluation = rule_engine::evaluate_rules(
                EvalContext { media, metadata: metadata.as_ref(), now },
                &ctx.rules,
                ctx.overrides.get(&media.id).map(String::as_str),
            );
            LibraryOutcome {
                winner: evaluation.winner.map(|m| m.rule_id),
                alternatives: evaluation.alternatives.into_iter().map(|m| m.rule_id).collect(),
                excluded: evaluation.excluded.into_iter().map(|m| m.rule_id).collect(),
            }
        })
        .collect())
}

pub fn format_timestamp(ts: chrono::DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// The inverse, so the shape of a stored timestamp is stated once.
///
/// `None` rather than a fallback to the current instant: a caller reading a
/// pinned instant that turned out unreadable must say so, not silently answer
/// a different question with today's clock.
pub fn parse_timestamp(raw: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Load rules, overrides, mappings and metadata in a fixed number of queries.
async fn load_context(pool: &SqlitePool) -> AppResult<RoutingContext> {
    let rules = load_rules(pool).await?;

    let overrides: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT media_id, target_category FROM overrides")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let root_folders: HashMap<(String, String), String> =
        sqlx::query_as::<_, (String, String, String)>(
            // Every mapped folder, reachable or not. An Arr reports a folder
            // on a sleeping NAS as inaccessible, and reading that as *absent*
            // unmapped its category: everything bound for it became "skip",
            // indistinguishable on screen from a category nobody mapped, and
            // the rerun then superseded the plan built while the disk was
            // awake. Unknown is not gone — whether the destination can be
            // written to is asked at apply time, where it can be answered.
            "SELECT instance_id, category, path FROM root_folders
             WHERE category IS NOT NULL AND category != ''",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(instance_id, category, path)| ((instance_id, category), path))
        .collect();

    let free_space: HashMap<(String, String), Option<i64>> =
        sqlx::query_as::<_, (String, String, Option<i64>)>(
            "SELECT instance_id, path, free_space FROM root_folders",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(instance_id, path, free)| ((instance_id, normalize_path(&path)), free))
        .collect();

    let instance_names: HashMap<String, String> =
        sqlx::query_as::<_, (String, String)>("SELECT id, name FROM instances")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    // An absent row means "never configured", which is the default order; a row
    // set to the empty string means the user turned every source off, which is
    // a legitimate choice for a library routed on paths and titles alone.
    let providers_setting: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'metadata_providers'")
            .fetch_optional(pool)
            .await?;
    let providers = match providers_setting {
        Some(raw) => metadata::parse_order(&raw),
        None => metadata::parse_order(&metadata::DEFAULT_ORDER.join(",")),
    };

    let metadata = metadata::load_cache(pool).await?;
    let identifiers = metadata::load_identifiers(pool).await?;

    let default_category = crate::state::AppState::default_category(pool).await;

    Ok(RoutingContext {
        rules,
        overrides,
        root_folders,
        free_space,
        instance_names,
        providers,
        metadata,
        identifiers,
        default_category,
    })
}

/// Collapse every enabled source's answer for one item.
///
/// The order is the user's; per field the first source that has a value keeps
/// it. `arr` is answered from the row itself — it is the only source that never
/// costs a request — and a fetched source only contributes when the item
/// carries an identifier in that source's namespace *and* the cache holds it.
fn resolve_metadata(
    media: &Media,
    providers: &[&'static ProviderInfo],
    cache: &HashMap<(String, String, String), ProviderMetadata>,
    identifiers: &metadata::Identifiers,
) -> Option<MediaMetadata> {
    let mut parts: Vec<(&str, ProviderMetadata)> = Vec::with_capacity(providers.len());

    for provider in providers {
        if provider.id == metadata::ARR {
            parts.push((provider.id, metadata::from_media(media)));
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

    MediaMetadata::merge(parts)
}

type RuleRow = (
    String,
    String,
    Option<String>,
    i64,
    bool,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
);

pub const RULE_COLUMNS: &str = "id, name, description, priority, enabled, media_type, conditions,
     target_category, instance_ids, created_at, updated_at, match_mode, exclusions";

/// Every column `Media` expects, in one place.
///
/// `FromRow` maps by name, so a query that forgets a column fails at *runtime*,
/// not at compile time — and only on the code path that runs it. Spelled out at
/// each call site, adding a column is one such failure per site missed.
pub const MEDIA_COLUMNS: &str = "id, instance_id, arr_id, media_type, title, sort_title, year,
     tmdb_id, tvdb_id, imdb_id, current_path, current_root_folder, monitored, has_files,
     status, added_at, series_type, size_on_disk, season_count, tags, genres,
     original_language, certification, last_synced_at";

/// Load every rule, tolerating rows whose JSON payload got corrupted.
pub async fn load_rules(pool: &SqlitePool) -> AppResult<Vec<Rule>> {
    let rows: Vec<RuleRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM rules ORDER BY priority ASC"
    )))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(rule_from_row).collect())
}

pub fn rule_from_row(r: RuleRow) -> Rule {
    let conditions: Vec<Condition> = serde_json::from_str(&r.6).unwrap_or_else(|e| {
        tracing::warn!(rule_id = %r.0, "Ignoring unreadable rule conditions: {e}");
        Vec::new()
    });
    let exclusions: Vec<Condition> = serde_json::from_str(&r.12).unwrap_or_default();
    let instance_ids: Option<Vec<String>> =
        r.8.as_deref().filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(s).ok());
    let match_mode: MatchMode = r.11.parse().unwrap_or_default();

    Rule {
        id: r.0,
        name: r.1,
        description: r.2,
        priority: r.3,
        enabled: r.4,
        media_type: r.5,
        conditions,
        exclusions,
        match_mode,
        target_category: r.7,
        instance_ids,
        created_at: r.9,
        updated_at: r.10,
    }
}

/// Load media, filtering in SQL rather than in memory.
/// `instance_ids` is a plain list because an empty one is every instance, the
/// way `Rule::covers_instance` reads the same shape — there is no second
/// spelling of "all" for a caller to get wrong. `media_ids` is not: an empty
/// list there is these zero items, which a webhook whose item has just been
/// deleted needs to say.
async fn load_media(
    pool: &SqlitePool,
    instance_filter: &[String],
    media_ids: Option<&[String]>,
    media_type: Option<&str>,
) -> AppResult<Vec<Media>> {
    let mut sql = String::from(&format!("SELECT {MEDIA_COLUMNS} FROM media WHERE 1=1"));

    if media_type.is_some() {
        sql.push_str(" AND media_type = ?");
    }
    if !instance_filter.is_empty() {
        sql.push_str(" AND instance_id IN (");
        sql.push_str(&crate::db::placeholders(instance_filter.len()));
        sql.push(')');
    }
    // Read as no filter, an empty media list had a webhook whose item had
    // just been deleted evaluate — and persist, and automatically apply — the
    // whole instance.
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

    Ok(query.fetch_all(pool).await?)
}

/// How many whole-library passes may run at once, across every caller.
///
/// A pass holds the rules, the overrides, the mappings, the metadata cache and
/// every media row for as long as it runs, and no lock bounds that: a preview
/// is not refused for a sweep, a health report is refused for nothing. Two at
/// once is one sweep and one reader; the third waits. A wait is what a read
/// can afford, where a refusal is a 409 on a page that only wanted to render.
pub const MAX_CONCURRENT_LIBRARY_PASSES: usize = 2;

static LIBRARY_PASSES: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_LIBRARY_PASSES));

/// The permit one whole-library pass holds while it runs.
///
/// Public so a test can hold every permit and prove that the next pass waits.
pub async fn library_pass() -> tokio::sync::SemaphorePermit<'static> {
    LIBRARY_PASSES.acquire().await.expect("the library-pass semaphore is never closed")
}

/// How many ids one statement binds. SQLite caps bound parameters at 32 766;
/// this stays far under it so a library-sized list is a few statements, not
/// an error on the one path that runs it.
pub const BIND_CHUNK: usize = 400;

/// Retire every pending proposal for these media, in place.
///
/// A run supersedes what it re-evaluated, and a row that leaves the library,
/// a full sync or a delete event, takes its proposals with it.
pub async fn supersede_pending(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    media_ids: &[&str],
) -> AppResult<()> {
    retire_pending(tx, "media_id", media_ids).await
}

/// Retire these pending proposals, and only these.
///
/// An apply retires the proposals it found stale by their own ids: a
/// simulation may have written the item's next proposal since the apply read
/// this one, and that one stands.
pub async fn supersede_decisions(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    decision_ids: &[&str],
) -> AppResult<()> {
    retire_pending(tx, "id", decision_ids).await
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

/// Persist a run: obsolete the previous proposals, then write the new ones.
///
/// `evaluated` is every media the run looked at; `decisions` is the subset worth
/// storing. The two differ on purpose — see the call site.
async fn store_decisions(
    pool: &SqlitePool,
    evaluated: &[&str],
    decisions: &[&Decision],
) -> AppResult<()> {
    if evaluated.is_empty() && decisions.is_empty() {
        return Ok(());
    }

    let media_ids: Vec<&str> =
        evaluated.iter().copied().collect::<HashSet<_>>().into_iter().collect();
    let mut tx = pool.begin().await?;
    supersede_pending(&mut tx, &media_ids).await?;

    for decision in decisions {
        sqlx::query(
            "INSERT INTO decisions (id, media_id, media_title, media_type, instance_id, instance_name,
             current_root_folder, target_root_folder, target_category, matched_rule_id, matched_rule_name,
             is_override, reasons, alternatives, action, status, decided_at, confidence,
             simulation_id, actor, subject)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&decision.matched_rule_name)
        .bind(decision.is_override)
        .bind(serde_json::to_string(&decision.reasons)?)
        .bind(serde_json::to_string(&decision.alternatives)?)
        .bind(&decision.action)
        .bind(&decision.status)
        .bind(&decision.decided_at)
        .bind(decision.confidence)
        .bind(&decision.simulation_id)
        .bind(&decision.actor)
        .bind(&decision.subject)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
