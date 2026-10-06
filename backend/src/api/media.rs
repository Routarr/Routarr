//! Media explorer and per-item explainability.

use super::{Json, Query};
use axum::extract::State;

use super::Path;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::api::{Page, paginate};
use crate::error::{AppError, AppResult};
use crate::integrations::language;
use crate::localization::Localizer;
use crate::models::*;
use crate::services::metadata::{self, ProviderInfo};
use crate::services::routing::MEDIA_COLUMNS;
use crate::services::rule_engine::{self, EvalContext};
use crate::services::{enrichment, routing};
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct MediaListItem {
    pub id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub arr_id: i64,
    pub media_type: String,
    pub title: String,
    pub year: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub current_root_folder: Option<String>,
    pub monitored: bool,
    pub has_files: bool,
    pub status: Option<String>,
    pub last_synced_at: Option<String>,
    /// The category the last stored run sends the title to, whether it moves
    /// or already sits there. Null until a run has evaluated it.
    pub computed_category: Option<String>,
    pub override_category: Option<String>,
    pub has_metadata: bool,
}

/// "Something a rule could read is known about this item", in SQL: what
/// `MediaMetadata::merge` answers for the engine, and the two must agree.
///
/// Built from the source list rather than hard-coded: with every source off the
/// answer is `0`, not "whatever happens to be left in the cache". The library
/// column, the diagnostics count and the warning beside it all splice it, and
/// must agree: a badge saying "metadata missing" over a count saying otherwise
/// is how a diagnostic stops being read.
///
/// Only the *enabled* sources count, exactly as `routing::load_context` reads
/// them. Each is joined on its own identifier, as `facet_rows` joins them: a
/// TMDb id and a TheTVDB id share no namespace, and a source found by search
/// is reached through what `source_identifiers` resolved the title's key to.
/// A title known to no id is keyed by its folded title, which SQL cannot
/// spell, so its search answers are left out, as in the facets. And a cached
/// row is not a cached *answer*: a synopsis is readable by no condition, so
/// the five fields `MetadataField` names are what is looked for.
///
/// The source ids are `&'static str` from the catalogue, never anything a
/// caller sent, which is what makes splicing them safe.
pub(crate) fn metadata_predicate(
    providers: &[&'static crate::services::metadata::ProviderInfo],
) -> String {
    let answers = holds_any("c", &MetadataField::ALL);
    let clauses: Vec<String> = providers
        .iter()
        .map(|provider| {
            let source = provider.id;
            match provider.addressing {
                metadata::Addressing::Local => holds_any("m", provider.fields),
                metadata::Addressing::Column(column) => {
                    let held = match column {
                        "imdb_id" => "m.imdb_id",
                        "tmdb_id" => "CAST(m.tmdb_id AS TEXT)",
                        _ => "CAST(m.tvdb_id AS TEXT)",
                    };
                    format!(
                        "EXISTS (SELECT 1 FROM metadata_cache c
                                  WHERE c.source = '{source}' AND c.external_id = {held}
                                    AND c.media_type = m.media_type AND {answers})"
                    )
                }
                metadata::Addressing::Search => format!(
                    "EXISTS (SELECT 1 FROM source_identifiers si
                               JOIN metadata_cache c ON c.source = si.source
                                AND c.external_id = si.external_id
                                AND c.media_type = si.media_type
                              WHERE si.source = '{source}' AND si.media_type = m.media_type
                                AND si.local_key = {LOCAL_KEY} AND {answers})"
                ),
            }
        })
        .collect();
    if clauses.is_empty() { "0".to_string() } else { clauses.join(" OR ") }
}

/// A title's key among the searched sources' resolutions, in SQL:
/// `metadata::local_key_of`, but for a title known to no id.
const LOCAL_KEY: &str = "CASE WHEN m.tmdb_id IS NOT NULL THEN 'tmdb:' || m.tmdb_id
                              WHEN m.tvdb_id IS NOT NULL THEN 'tvdb:' || m.tvdb_id
                              WHEN COALESCE(TRIM(m.imdb_id), '') != ''
                                   THEN 'imdb:' || TRIM(m.imdb_id) END";

/// "This row holds a value for one of these fields", in SQL. A column holds a
/// JSON list or a plain value, and the merge counts neither when it is blank,
/// so `''`, `'[]'` and NULL are all empty.
fn holds_any(alias: &str, fields: &[MetadataField]) -> String {
    let any: Vec<String> = fields
        .iter()
        .map(|field| format!("COALESCE(TRIM({alias}.{}), '') NOT IN ('', '[]')", field.as_str()))
        .collect();
    format!("({})", any.join(" OR "))
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<MediaQuery>,
) -> AppResult<Json<Page<MediaListItem>>> {
    let (page, per_page, offset) = paginate(query.page, query.per_page);

    let mut filters = String::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(v) = &query.instance_id {
        filters.push_str(" AND m.instance_id = ?");
        binds.push(v.clone());
    }
    if let Some(v) = &query.media_type {
        filters.push_str(" AND m.media_type = ?");
        binds.push(v.clone());
    }
    for (column, id) in [("m.tmdb_id", query.tmdb_id), ("m.tvdb_id", query.tvdb_id)] {
        if let Some(id) = id {
            filters.push_str(&format!(" AND {column} = CAST(? AS INTEGER)"));
            binds.push(id.to_string());
        }
    }
    if let Some(v) = &query.imdb_id {
        filters.push_str(" AND m.imdb_id = ?");
        binds.push(v.trim().to_string());
    }
    if let Some(v) = &query.search {
        filters.push_str(" AND m.title LIKE ? ESCAPE '\\'");
        binds.push(format!("%{}%", crate::db::escape_like(v)));
    }
    // Both read what the last run decided, the outcome `computed_category`
    // below shows: the decisions hold only what a run proposed.
    if let Some(v) = &query.category {
        filters
            .push_str(" AND m.id IN (SELECT r.media_id FROM media_routing r WHERE r.category = ?)");
        binds.push(v.clone());
    }
    if query.unmatched.unwrap_or(false) {
        filters.push_str(
            " AND NOT EXISTS (SELECT 1 FROM media_routing r
                               WHERE r.media_id = m.id AND r.matched_rule_id IS NOT NULL)",
        );
    }

    // "Something is known about this item", counted over the *enabled* sources
    // only. Reading it off any cached row would contradict `has_metadata` in the
    // engine, which honours the order, and a list saying "metadata" next to a
    // rule saying there is none is the kind of disagreement nobody debugs twice.
    let has_metadata = metadata_predicate(&state.metadata_order().await);

    // The correlated sub-selects keep this to two queries instead of the
    // per-row lookups the explorer would otherwise need.
    let list_sql = format!(
        "SELECT m.id, m.instance_id, i.name AS instance_name, m.arr_id, m.media_type, m.title,
                m.year, m.tmdb_id, m.tvdb_id, m.imdb_id, m.current_root_folder, m.monitored, m.has_files, m.status,
                m.last_synced_at,
                (SELECT r.category FROM media_routing r WHERE r.media_id = m.id)
                  AS computed_category,
                (SELECT o.target_category FROM overrides o WHERE o.media_id = m.id) AS override_category,
                ({has_metadata}) AS has_metadata
         FROM media m
         JOIN instances i ON m.instance_id = i.id
         WHERE 1=1{filters}
         ORDER BY m.title ASC, m.id ASC LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM media m WHERE 1=1{filters}");

    let mut list_query = sqlx::query_as::<_, MediaListItem>(AssertSqlSafe(list_sql.as_str()));
    let mut count_query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql.as_str()));
    for bind in &binds {
        list_query = list_query.bind(bind);
        count_query = count_query.bind(bind);
    }

    let rows = list_query.bind(per_page).bind(offset).fetch_all(&state.pool).await?;
    let total = count_query.fetch_one(&state.pool).await?;

    Ok(Json(Page::new(rows, page, per_page, total)))
}

/// A title named by the id another service gives it, on every instance that
/// holds it or on one.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExternalTitle {
    /// `movie` or `series`.
    #[serde(rename = "type")]
    pub media_type: String,
    /// The title's TMDb id. Name the title by exactly one of `tmdb`, `tvdb`
    /// and `imdb`.
    pub tmdb: Option<i64>,
    /// The title's TheTVDB id.
    pub tvdb: Option<i64>,
    /// The title's IMDb id, as `tt0133093`.
    pub imdb: Option<String>,
    /// Only the copy on this instance.
    pub instance: Option<String>,
}

impl ExternalTitle {
    /// The title's type and the one id naming it, or why they do not name one.
    pub fn named(&self) -> AppResult<(&str, ExternalId)> {
        if !matches!(self.media_type.as_str(), "movie" | "series") {
            return Err(AppError::BadRequest("`type` is `movie` or `series`.".into()));
        }
        let id =
            ExternalId::one_of(self.tmdb, self.tvdb, self.imdb.as_deref()).ok_or_else(|| {
                AppError::BadRequest(
                    "Name the title by exactly one of `tmdb`, `tvdb` and `imdb`.".into(),
                )
            })?;
        // TheTVDB knows series alone, and no movie carries its id.
        if self.media_type == "movie" && matches!(id, ExternalId::Tvdb(_)) {
            return Err(AppError::BadRequest(
                "A movie is named by its `tmdb` or `imdb` id.".into(),
            ));
        }
        Ok((&self.media_type, id))
    }
}

/// What `GET /route` takes beside the title.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PlacementOptions {
    /// The tag labels a title the Arr does not hold would be added with,
    /// separated by commas, for the rules that read tags.
    pub tags: Option<String>,
    /// The series type a series Sonarr does not hold would be added with:
    /// `standard`, `daily` or `anime`. Defaults to what Sonarr's lookup says.
    pub series_type: Option<String>,
    /// Whether a title the Arr does not hold would be added monitored.
    /// Defaults to true, as Radarr and Sonarr add one.
    pub monitored: Option<bool>,
    /// Ask every source that can answer and has no cached answer now, storing
    /// nothing. Slower, and paced by each source's limits. Defaults to false.
    pub enrich: Option<bool>,
}

/// Where a title another service names would go, on each Arr that holds it or
/// knows it.
pub async fn place(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Query(title): Query<ExternalTitle>,
    Query(options): Query<PlacementOptions>,
) -> AppResult<Json<crate::services::placement::Placement>> {
    let enrich = options.enrich.unwrap_or(false);
    // Asking the sources now spends the owner's quotas, as the `operate`
    // probes of `/health` do. What is cached is the read scope's.
    if enrich && !identity.holds(crate::services::applications::Scope::Operate) {
        return Err(AppError::Forbidden(
            "enrich=true asks the metadata sources now and needs the operate scope. Leave it out \
             to place from what is cached."
                .into(),
        ));
    }
    let (media_type, id) = title.named()?;
    let tags: Vec<String> = options
        .tags
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect();
    let series_type = match options.series_type.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(kind @ ("standard" | "daily" | "anime")) => Some(kind.to_string()),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "series_type '{other}' is not one of standard, daily and anime."
            )));
        }
    };
    let added =
        crate::services::placement::AddedWith { tags, series_type, monitored: options.monitored };
    let placement = crate::services::placement::place(
        &state,
        media_type,
        &id,
        title.instance.as_deref(),
        &added,
        enrich,
    )
    .await?;
    Ok(Json(placement))
}

/// One title, what its sources say about it, and the exception pinning it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MediaDetail {
    pub media: MediaView,
    pub instance_name: Option<String>,
    /// Null until a source has answered for the title.
    pub metadata: Option<crate::models::MediaMetadata>,
    /// The exception pinning the title to a category, if one does.
    #[serde(rename = "override")]
    pub exception: Option<PinnedCategory>,
}

/// The category an exception pins a title to, and why.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PinnedCategory {
    pub target_category: String,
    pub reason: Option<String>,
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<MediaDetail>> {
    let media = load_media(&state, &id).await?;

    let instance_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM instances WHERE id = ?")
            .bind(&media.instance_id)
            .fetch_optional(&state.pool)
            .await?;

    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    let override_entry: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT target_category, reason FROM overrides WHERE media_id = ?")
            .bind(&media.id)
            .fetch_optional(&state.pool)
            .await?;

    Ok(Json(MediaDetail {
        media: media.into(),
        instance_name,
        metadata,
        exception: override_entry
            .map(|(target_category, reason)| PinnedCategory { target_category, reason }),
    }))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Explanation {
    pub media: MediaView,
    pub metadata: Option<MediaMetadata>,
    pub override_category: Option<String>,
    pub target_category: String,
    pub target_root_folder: Option<String>,
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["move", "none", "skip"]))))]
    pub action: DecisionAction,
    pub confidence: f32,
    pub winning_rule: Option<String>,
    /// Per-condition outcome for every rule that was considered.
    pub rule_traces: Vec<RuleTrace>,
    /// False when the title's instance is switched off: no run reads it, and
    /// the move shown is only what the rules would do.
    pub instance_enabled: bool,
}

/// How the engine read one rule for the title.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceOutcome {
    /// It decided where the title goes.
    Winner,
    /// Its conditions held and one of its exclusions set it aside.
    Excluded,
    /// Its conditions held and a rule tried before it won.
    MatchedLowerPriority,
    NotMatched,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RuleTrace {
    pub rule_id: String,
    pub rule_name: String,
    pub priority: i64,
    pub category: String,
    pub matched: bool,
    pub excluded_by: Option<String>,
    #[schema(
        value_type = String,
        extensions(("x-extensible-enum" = json!(["winner", "excluded", "matched_lower_priority", "not_matched"])))
    )]
    pub outcome: TraceOutcome,
    /// Every condition, with its wording already resolved for this language.
    pub conditions: Vec<rule_engine::ConditionOutcome>,
}

/// Explain, condition by condition, why a media item lands where it does.
///
/// Evaluated live rather than read from a stored decision, so it also explains
/// rules that were edited since the last run. The category, the folder and the
/// action come from `routing::route_one`, the simulation's own decision, and
/// only the trace is built here.
pub async fn explain(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Explanation>> {
    let media = load_media(&state, &id).await?;
    let localizer: Localizer = state.localizer().await;
    let instance_enabled: bool = sqlx::query_scalar("SELECT enabled FROM instances WHERE id = ?")
        .bind(&media.instance_id)
        .fetch_one(&state.pool)
        .await?;

    let now = Utc::now();
    let routing::ItemRoute { rules, override_category, route } =
        routing::route_one(&state.pool, &media, now).await?;
    let routing::Route { metadata, evaluation, category, target, action } = route;
    let ctx = EvalContext { media: &media, metadata: metadata.as_ref(), now };

    // Trace every applicable rule, not just the winner: seeing why the *other*
    // rules did not fire is usually the actual question.
    let winner_id = evaluation.winner.as_ref().map(|w| w.rule_id.clone());
    let mut rule_traces = Vec::new();

    // The rules the engine considers, in its order, and its own reading of
    // each: a second definition here would call matched what it never fires.
    for rule in rule_engine::in_order(&rules, &media) {
        let (matched, outcomes) =
            rule_engine::evaluate_conditions(&rule.conditions, rule.match_mode, ctx);
        let conditions: Vec<rule_engine::ConditionOutcome> =
            outcomes.into_iter().map(|outcome| localizer.localize_outcome(outcome)).collect();

        // The engine's own veto, read for a title an exception pins too: the
        // engine then reads no rule, and the trace is what each would do.
        let excluded_by = matched
            .then(|| rule_engine::veto(rule, ctx))
            .flatten()
            .map(|veto| localizer.localize_outcome(veto).expected);

        let outcome = if Some(&rule.id) == winner_id.as_ref() {
            TraceOutcome::Winner
        } else if excluded_by.is_some() {
            TraceOutcome::Excluded
        } else if matched {
            TraceOutcome::MatchedLowerPriority
        } else {
            TraceOutcome::NotMatched
        };

        rule_traces.push(RuleTrace {
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            priority: rule.priority,
            category: rule.target_category.clone(),
            matched,
            excluded_by,
            outcome,
            conditions,
        });
    }

    Ok(Json(Explanation {
        media: media.into(),
        metadata,
        override_category,
        target_category: category,
        target_root_folder: target,
        action,
        confidence: evaluation.winner.as_ref().map(|w| w.confidence).unwrap_or(0.0),
        winning_rule: evaluation.winner.as_ref().map(|w| {
            if w.rule_id == rule_engine::OVERRIDE_RULE_ID {
                localizer.translate("ManualOverrideRuleName", &[])
            } else {
                w.rule_name.clone()
            }
        }),
        rule_traces,
        instance_enabled,
    }))
}

pub(crate) async fn load_media(state: &AppState, id: &str) -> AppResult<Media> {
    sqlx::query_as::<_, Media>(AssertSqlSafe(format!(
        "SELECT {MEDIA_COLUMNS} FROM media WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("Media {id} not found")))
}

/// The closed vocabularies, as (value, the name to show for it).
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Vocabularies {
    pub original_languages: Vec<Facet>,
    pub origin_countries: Vec<Facet>,
}

/// One value a condition can hold, and how many items carry it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Facet {
    /// What a rule stores and the engine compares. Never the displayed text: a
    /// language is matched on `ja`, however it is shown.
    pub value: String,
    /// What to show instead of `value`, where the two differ. Absent for a
    /// genre, which is its own label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub count: i64,
    /// What the value means, where several values mean the same thing: codes
    /// of several countries rate a film alike, and a rule wants every one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// What the library holds on each axis a condition reads, each value with
/// how many titles carry it: what a rule can be written against.
// Without it, writing a rule means guessing which values the library holds,
// and a rule written against one that is not there matches nothing, reading
// on screen exactly like a rule that correctly matches nothing. Derived from
// columns the sync already writes: no request leaves the process, and nothing
// is cached, the answer being only as old as the last sync either way.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct LibraryFacets {
    pub total_media: i64,
    /// Every language and country a rule may name, whether the library holds
    /// it or not: their codes are a closed vocabulary.
    pub vocabularies: Vocabularies,
    /// Titles no enabled source describes, which no condition reading
    /// metadata can match.
    pub without_metadata: i64,
    pub genres: Vec<Facet>,
    pub original_languages: Vec<Facet>,
    /// The countries the library's titles come from, counted like the
    /// languages.
    pub origin_countries: Vec<Facet>,
    pub certifications: Vec<Facet>,
    pub tags: Vec<Facet>,
    pub series_types: Vec<Facet>,
    pub root_folders: Vec<Facet>,
    /// The statuses the Arrs give the titles, as they write them.
    pub statuses: Vec<Facet>,
}

/// A fixed table as the picker consumes it: the value a rule stores, labelled
/// with the spelling a reader recognises. `count` is 0 throughout: these are
/// not observations, and the picker renders no figure for them.
fn vocabulary(table: &[(&str, &[&str])], localizer: &Localizer) -> Vec<Facet> {
    let mut out: Vec<Facet> = table
        .iter()
        .filter_map(|(code, spellings)| {
            // A country that no longer exists in the reader's language: the
            // browser, which names every other one, names these after the
            // countries that took their codes.
            let name = if language::RETIRED.contains(code) {
                localizer.translate(&format!("CountryRetired{code}"), &[])
            } else {
                capitalise(spellings.first()?)
            };
            Some(Facet {
                value: (*code).to_string(),
                label: Some(format!("{name} ({code})")),
                count: 0,
                group: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The statuses Radarr and Sonarr give a title, as they write them, and the
/// key naming each in words.
const ARR_STATUSES: &[(&str, &str)] = &[
    ("tba", "ArrStatusTba"),
    ("announced", "ArrStatusAnnounced"),
    ("inCinemas", "ArrStatusInCinemas"),
    ("released", "ArrStatusReleased"),
    ("continuing", "ArrStatusContinuing"),
    ("upcoming", "ArrStatusUpcoming"),
    ("ended", "ArrStatusEnded"),
    ("deleted", "ArrStatusDeleted"),
];

/// Name each status the library holds in the reader's words, where it is one
/// the Arrs are known to write.
fn name_statuses(held: Vec<Facet>, localizer: &Localizer) -> Vec<Facet> {
    held.into_iter()
        .map(|facet| {
            let key = ARR_STATUSES.iter().find(|(status, _)| *status == facet.value);
            let label = key.map(|(_, key)| localizer.translate(key, &[]));
            Facet { label, ..facet }
        })
        .collect()
}

/// Count the distinct values of one column, commonest first.
async fn column_facets(pool: &sqlx::SqlitePool, column: &str) -> AppResult<Vec<Facet>> {
    // `column` is interpolated, so it must stay a literal from this file and
    // never anything a caller can influence, the same rule the metadata
    // addressing follows.
    let sql = format!(
        "SELECT {column} AS value, COUNT(*) AS n FROM media
         WHERE {column} IS NOT NULL AND {column} != ''
         GROUP BY {column} ORDER BY n DESC, value"
    );
    Ok(sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql.as_str()))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count)| Facet { value, label: None, count, group: None })
        .collect())
}

/// The same, for a column holding a JSON array.
async fn json_facets(pool: &sqlx::SqlitePool, column: &str) -> AppResult<Vec<Facet>> {
    let sql = format!(
        "SELECT j.value AS value, COUNT(*) AS n
         FROM media, json_each(media.{column}) j
         WHERE media.{column} IS NOT NULL AND media.{column} != ''
         GROUP BY j.value ORDER BY n DESC, value"
    );
    Ok(sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql.as_str()))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count)| Facet { value, label: None, count, group: None })
        .collect())
}

/// Say what a certification code stands for, where that is not in dispute.
///
/// `U`, `TP` and `TV-PG` say nothing to most readers, and this panel exists to
/// show what the library holds. The code stays the *value*, since it is what a
/// rule matches on, and the name is only ever what is shown, which is why it goes
/// in `label` beside it rather than replacing it.
///
/// `held` pairs each code with the systems that gave it. A code is named only
/// when every one of them gives it the same meaning, and one MyAnimeList alone
/// gives is labelled by MyAnimeList's own words, still grouped by its age.
fn name_certifications(
    held: Vec<(Facet, Vec<Option<String>>)>,
    localizer: &Localizer,
) -> Vec<Facet> {
    use crate::integrations::certification::{MAL, Meaning, mal_words, meaning};
    let mut named: Vec<(u16, Facet)> = held
        .into_iter()
        .map(|(facet, scales)| {
            let meanings: Vec<Option<Meaning>> =
                scales.iter().map(|scale| meaning(&facet.value, scale.as_deref())).collect();
            let meant = meanings
                .first()
                .copied()
                .flatten()
                .filter(|first| meanings.iter().all(|other| *other == Some(*first)));
            // The youngest audience first, a code nobody can name last.
            let rank = match meant {
                Some(Meaning::AllAges) => 0,
                Some(Meaning::Guidance) => 1,
                Some(Meaning::From(age)) => 2 + u16::from(age),
                Some(Meaning::NotRated) => 900,
                None => 1000,
            };
            let group = meant.map(|m| match m {
                Meaning::AllAges => localizer.translate("CertAllAges", &[]),
                Meaning::Guidance => localizer.translate("CertGuidance", &[]),
                Meaning::From(age) => {
                    localizer.translate("CertFromAge", &[("age", &age.to_string())])
                }
                Meaning::NotRated => localizer.translate("CertNotRated", &[]),
            });
            let words = scales
                .iter()
                .all(|scale| scale.as_deref() == Some(MAL))
                .then(|| mal_words(&facet.value))
                .flatten()
                .map(|key| localizer.translate(key, &[]));
            // The code first and the meaning after it: the reader is looking
            // for the value their rule will carry.
            let label =
                words.as_ref().or(group.as_ref()).map(|name| format!("{} ({name})", facet.value));
            (rank, Facet { label, group, ..facet })
        })
        .collect();
    // Stable, so the most frequent code still leads its group: codes of
    // several countries mean the same thing, and gathered they read as the
    // choice they are.
    named.sort_by_key(|(rank, _)| *rank);
    named.into_iter().map(|(_, facet)| facet).collect()
}

/// Everything the *enabled* sources say about one axis, not just the Arr row.
///
/// The rule builder offers this list, so it has to hold what the engine can
/// actually match: a genre TMDb supplied is matched by a rule and would be
/// missing from a list read off `media` alone. The `arr` branch is the media
/// row, the others `metadata_cache`, and a source the user disabled
/// contributes to none, exactly as `routing::load_context` reads them.
///
/// Counted with `COUNT(DISTINCT m.id)`, since one item is described by several
/// sources at once. Spellings that differ only by case or by a separator are
/// one value, as they are to
/// [`normalise_value`](crate::services::rule_engine::normalise_value). Accents
/// are not folded here, SQLite having no way to, so `Comédie` and `Comedie`
/// stay two entries in the list while the engine still matches both.
/// `media_column` is where the sync puts the Arr's own answer, and `None` for
/// an axis the Arr does not report at all: origin countries live in the cache
/// and nowhere else, and naming a column the table has not got fails the whole
/// endpoint rather than that one axis.
async fn metadata_facets(
    pool: &sqlx::SqlitePool,
    sources: &[&'static ProviderInfo],
    media_column: Option<&str>,
    cache_column: &str,
    json: bool,
) -> AppResult<Vec<Facet>> {
    let Some((rows, binds)) = facet_rows(sources, media_column, cache_column, json, false) else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "SELECT MIN(v.value) AS value, COUNT(DISTINCT v.media_id) AS n
           FROM ({rows}) v
          WHERE v.value IS NOT NULL AND v.value != ''
          GROUP BY lower(replace(replace(v.value, '-', ' '), '_', ' '))
          ORDER BY n DESC, value"
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql.as_str()));
    for source in binds {
        query = query.bind(source);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count)| Facet { value, label: None, count, group: None })
        .collect())
}

/// The certifications the enabled sources give, each with the systems that
/// gave it: whether `R` is seventeen or eighteen and over depends on the
/// system, and a MyAnimeList code is named by MyAnimeList's own words.
async fn certification_facets(
    pool: &sqlx::SqlitePool,
    sources: &[&'static ProviderInfo],
    localizer: &Localizer,
) -> AppResult<Vec<Facet>> {
    let Some((rows, binds)) =
        facet_rows(sources, Some("certification"), "certification", false, true)
    else {
        return Ok(Vec::new());
    };
    // A scale, a country code or MAL, holds no comma.
    let sql = format!(
        "SELECT MIN(v.value) AS value, COUNT(DISTINCT v.media_id) AS n,
                GROUP_CONCAT(DISTINCT coalesce(v.scale, '')) AS scales
           FROM ({rows}) v
          WHERE v.value IS NOT NULL AND v.value != ''
          GROUP BY lower(replace(replace(v.value, '-', ' '), '_', ' '))
          ORDER BY n DESC, value"
    );
    let mut query = sqlx::query_as::<_, (String, i64, String)>(AssertSqlSafe(sql.as_str()));
    for source in binds {
        query = query.bind(source);
    }
    let held: Vec<(Facet, Vec<Option<String>>)> = query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count, scales)| {
            let scales = scales
                .split(',')
                .map(|scale| Some(scale.to_string()).filter(|scale| !scale.is_empty()))
                .collect();
            (Facet { value, label: None, count, group: None }, scales)
        })
        .collect();
    Ok(name_certifications(held, localizer))
}

/// Every `(media_id, value, scale)` the enabled sources hold on one axis, as
/// one SQL union and the source ids it binds, in order. `None` when no source
/// can answer it.
///
/// Each source is joined on its own identifier: a TMDb id and a TheTVDB id
/// share no namespace, and a source found by title (AniList, MyAnimeList) is
/// joined through what `source_identifiers` resolved, by the key
/// [`metadata::local_key_of`] gives an item. An item known to no id is keyed by
/// its normalised title, which SQL cannot spell, so its search answers are
/// left out of the counts. `scale` is the system a certification belongs to,
/// and empty unless asked for.
fn facet_rows(
    sources: &[&'static ProviderInfo],
    media_column: Option<&str>,
    cache_column: &str,
    json: bool,
    scale: bool,
) -> Option<(String, Vec<&'static str>)> {
    let unnest = |alias: &str, column: &str| {
        if json { format!(", json_each({alias}.{column}) j") } else { String::new() }
    };
    let value = |alias: &str, column: &str| {
        if json { "j.value".to_string() } else { format!("{alias}.{column}") }
    };
    let cache_scale = if scale { "c.certification_scale" } else { "NULL" };

    let mut branches = Vec::new();
    let mut binds = Vec::new();
    for source in sources {
        let cached = |join_media: &str, guard: &str| {
            format!(
                "SELECT m.id AS media_id, {v} AS value, {cache_scale} AS scale
                   FROM metadata_cache c{join_media}{join}
                  WHERE c.source = ? AND {guard}
                    AND c.{cache_column} IS NOT NULL AND c.{cache_column} != ''",
                v = value("c", cache_column),
                join = unnest("c", cache_column),
            )
        };
        match source.addressing {
            metadata::Addressing::Local => {
                let Some(media_column) = media_column else { continue };
                let arr_scale = if scale { "i.certification_country" } else { "NULL" };
                branches.push(format!(
                    "SELECT m.id AS media_id, {v} AS value, {arr_scale} AS scale
                       FROM media m JOIN instances i ON i.id = m.instance_id{join}
                      WHERE m.{media_column} IS NOT NULL AND m.{media_column} != ''",
                    v = value("m", media_column),
                    join = unnest("m", media_column),
                ));
            }
            metadata::Addressing::Column(column) => {
                // A literal of `PROVIDERS`, never anything a user sent.
                let (on, guard) = match column {
                    "imdb_id" => ("m.imdb_id = c.external_id", "c.external_id GLOB 'tt*'"),
                    other => (
                        if other == "tmdb_id" {
                            "m.tmdb_id = CAST(c.external_id AS INTEGER)"
                        } else {
                            "m.tvdb_id = CAST(c.external_id AS INTEGER)"
                        },
                        "c.external_id GLOB '[0-9]*'",
                    ),
                };
                branches.push(cached(
                    &format!(" JOIN media m ON m.media_type = c.media_type AND {on}"),
                    guard,
                ));
                binds.push(source.id);
            }
            metadata::Addressing::Search => {
                // One branch per kind of key, each served by an index on
                // `media`, in the order `local_key_of` tries them.
                for (on, guard) in [
                    (
                        "m.tmdb_id = CAST(substr(si.local_key, 6) AS INTEGER)",
                        "si.local_key GLOB 'tmdb:*'",
                    ),
                    (
                        "m.tvdb_id = CAST(substr(si.local_key, 6) AS INTEGER) AND m.tmdb_id IS NULL",
                        "si.local_key GLOB 'tvdb:*'",
                    ),
                    (
                        "m.imdb_id = substr(si.local_key, 6)
                         AND m.tmdb_id IS NULL AND m.tvdb_id IS NULL",
                        "si.local_key GLOB 'imdb:*'",
                    ),
                ] {
                    branches.push(cached(
                        &format!(
                            " JOIN source_identifiers si ON si.source = c.source
                                  AND si.media_type = c.media_type
                                  AND si.external_id = c.external_id
                              JOIN media m ON m.media_type = si.media_type AND {on}"
                        ),
                        guard,
                    ));
                    binds.push(source.id);
                }
            }
        }
    }
    (!branches.is_empty()).then(|| (branches.join(" UNION ALL "), binds))
}

pub async fn facets(State(state): State<AppState>) -> AppResult<Json<LibraryFacets>> {
    let pool = &state.pool;
    // The order the engine reads, so the list offered and the list matched are
    // the same one.
    let sources = state.metadata_order().await;
    let localizer = state.localizer().await;
    let total_media: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(pool).await?;
    let known = metadata_predicate(&sources);
    let without_metadata: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM media m WHERE NOT ({known})"
    )))
    .fetch_one(pool)
    .await?;

    Ok(Json(LibraryFacets {
        total_media,
        vocabularies: Vocabularies {
            original_languages: vocabulary(language::LANGUAGES, &localizer),
            origin_countries: vocabulary(language::COUNTRIES, &localizer),
        },
        without_metadata,
        genres: metadata_facets(pool, &sources, Some("genres"), "genres", true).await?,
        tags: json_facets(pool, "tags").await?,
        original_languages: metadata_facets(
            pool,
            &sources,
            Some("original_language"),
            "original_language",
            false,
        )
        .await?,
        // No media column: the sync does not read countries off the Arr's
        // payload, so the cache is the only place they exist.
        origin_countries: metadata_facets(pool, &sources, None, "origin_countries", true).await?,
        certifications: certification_facets(pool, &sources, &localizer).await?,
        series_types: column_facets(pool, "series_type").await?,
        root_folders: column_facets(pool, "current_root_folder").await?,
        statuses: name_statuses(column_facets(pool, "status").await?, &localizer),
    }))
}
