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
    /// When a sync last found the Arr's data for it changed.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_synced_at: Option<String>,
    /// The category the last stored run sends the title to, whether it moves
    /// or already sits there. Null until a run has evaluated it.
    pub computed_category: Option<String>,
    pub override_category: Option<String>,
    pub has_metadata: bool,
}

/// "Something a rule could read is known about this item", in SQL. It answers as
/// `MediaMetadata::merge` does over the sources `routing::load_context` reads, or the
/// library and the engine disagree. The spliced source ids are catalogue literals.
pub(crate) fn metadata_predicate(settings: &crate::state::Settings) -> String {
    let providers = AppState::metadata_order_from(settings);
    let cache = metadata::rated_cache(&AppState::certification_regions_from(settings));
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
                        "EXISTS (SELECT 1 FROM {cache} c
                                  WHERE c.source = '{source}' AND c.external_id = {held}
                                    AND c.media_type = m.media_type AND {answers})"
                    )
                }
                metadata::Addressing::Search => format!(
                    "EXISTS (SELECT 1 FROM source_identifiers si
                               JOIN {cache} c ON c.source = si.source
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
/// JSON list, a JSON object or a plain value, and the merge counts none when it
/// is blank, so `''`, `'[]'`, `'{}'` and NULL are all empty.
fn holds_any(alias: &str, fields: &[MetadataField]) -> String {
    let any: Vec<String> = fields
        .iter()
        .map(|field| {
            format!("COALESCE(TRIM({alias}.{}), '') NOT IN ('', '[]', '{{}}')", field.as_str())
        })
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
    // Both read the last run's outcome, since the decisions hold only what a run proposed.
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

    let has_metadata = metadata_predicate(&state.settings().await);

    // Correlated sub-selects keep this to two queries, with no lookup per row.
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
#[serde(deny_unknown_fields)]
pub struct ExternalTitle {
    /// `movie` or `series`.
    #[serde(rename = "type")]
    pub media_type: String,
    /// The title's TMDB id. Name the title by exactly one of `tmdb`, `tvdb`
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
        // An Arr writes 0 for a missing id, so 0 would name every such title at once.
        if matches!(id, ExternalId::Tmdb(n) | ExternalId::Tvdb(n) if n <= 0) {
            return Err(AppError::BadRequest("An id is a number above 0.".into()));
        }
        // TheTVDB knows series alone, and no movie carries its id.
        if self.media_type == "movie" && matches!(id, ExternalId::Tvdb(_)) {
            return Err(AppError::BadRequest(
                "A movie is named by its `tmdb` or `imdb` id.".into(),
            ));
        }
        Ok((&self.media_type, id))
    }
}

/// What `GET /route` reads from one query string: the title, named as
/// `ExternalTitle` names it, and how it would be added.
// Its own fields, since a flattened query reads every value as text and no number parses.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct RouteQuery {
    /// `movie` or `series`.
    #[serde(rename = "type")]
    pub media_type: String,
    /// The title's TMDB id. Name the title by exactly one of `tmdb`, `tvdb`
    /// and `imdb`.
    pub tmdb: Option<i64>,
    /// The title's TheTVDB id.
    pub tvdb: Option<i64>,
    /// The title's IMDb id, as `tt0133093`.
    pub imdb: Option<String>,
    /// Only the copy on this instance.
    pub instance: Option<String>,
    /// The tag labels a title the Arr does not hold would be added with,
    /// separated by commas, for the rules that read tags.
    pub tags: Option<String>,
    /// The series type a series Sonarr does not hold would be added with:
    /// `standard`, `daily` or `anime`. Defaults to what Sonarr's lookup says.
    pub series_type: Option<String>,
    /// Whether a title the Arr does not hold would be added monitored.
    /// Defaults to true, as Radarr and Sonarr add one.
    pub monitored: Option<bool>,
    /// The name of the quality profile a title the Arr does not hold would be added with, for
    /// the rules that read it.
    pub quality_profile: Option<String>,
    /// Ask every source that can answer and has no cached answer now, storing
    /// nothing. Slower, and paced by each source's limits. Defaults to false.
    pub enrich: Option<bool>,
}

impl RouteQuery {
    fn title(&self) -> ExternalTitle {
        ExternalTitle {
            media_type: self.media_type.clone(),
            tmdb: self.tmdb,
            tvdb: self.tvdb,
            imdb: self.imdb.clone(),
            instance: self.instance.clone(),
        }
    }
}

/// Where a title another service names would go, on each Arr that holds it or
/// knows it.
pub async fn place(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::api::auth::Identity>,
    Query(options): Query<RouteQuery>,
) -> AppResult<Json<crate::services::placement::Placement>> {
    let title = options.title();
    let enrich = options.enrich.unwrap_or(false);
    // Asking the sources spends the owner's quotas and needs `operate`, the cache only `read`.
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
    let quality_profile =
        options.quality_profile.as_deref().map(str::trim).filter(|name| !name.is_empty());
    let added = crate::services::placement::AddedWith {
        tags,
        series_type,
        monitored: options.monitored,
        quality_profile: quality_profile.map(str::to_string),
    };
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

/// The category an exception forces on a title, and why.
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
    /// What the rules alone decide, leaving the exception out, as a rule test pinned from
    /// this title expects. Equal to `target_category` when no exception is in force.
    pub rules_category: String,
    pub target_root_folder: Option<String>,
    /// The category the title's current root folder leads to, null when it leads to none: what
    /// `POST /library/{id}/keep` pins it to.
    pub current_category: Option<String>,
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["move", "none", "skip"]))))]
    pub action: DecisionAction,
    pub confidence: f32,
    pub winning_rule: Option<String>,
    /// Per-condition outcome for every rule that was considered.
    pub rule_traces: Vec<RuleTrace>,
    /// False when the title's instance is switched off: no run reads it, and
    /// the move shown is only what the rules would do.
    pub instance_enabled: bool,
    /// How each source that finds a work by search found this title, by source id: `mapping`,
    /// read from the anime identifier table, `shared`, the MyAnimeList id another source found,
    /// or `search`, by its title and year, which can name another work of the same name.
    pub identified_by: std::collections::BTreeMap<String, String>,
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

/// Explain, condition by condition, why a media item lands where it does. Evaluated live,
/// so it covers rules edited since the last run, the decision coming from `routing::route_one`.
pub async fn explain(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Explanation>> {
    let media = load_media(&state, &id).await?;
    let localizer: Localizer = state.localizer().await;
    let identified_by: std::collections::BTreeMap<String, String> = sqlx::query_as(
        "SELECT source, found_by FROM source_identifiers
          WHERE media_type = ? AND local_key = ? AND external_id IS NOT NULL",
    )
    .bind(&media.media_type)
    .bind(crate::services::metadata::local_key(&media))
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect();
    let instance_enabled: bool = sqlx::query_scalar("SELECT enabled FROM instances WHERE id = ?")
        .bind(&media.instance_id)
        .fetch_one(&state.pool)
        .await?;

    let now = Utc::now();
    let routing::ItemRoute { rules, override_category, route } =
        routing::route_one(&state.pool, &media, now).await?;
    let routing::Route { metadata, evaluation, category, target, action } = route;
    let ctx = EvalContext { media: &media, metadata: metadata.as_ref(), now };
    let rules_category = match override_category {
        Some(_) => {
            let default_category = AppState::default_category(&state.pool).await?;
            crate::services::rule_tests::decided_by_rules(ctx, &rules, &default_category).0
        }
        None => category.clone(),
    };

    // Trace every applicable rule: why the others did not fire is usually the question.
    let winner_id = evaluation.winner.as_ref().map(|w| w.rule_id.clone());
    let mut rule_traces = Vec::new();

    // The engine's own order and reading, or the trace calls matched what never fires.
    for rule in rule_engine::in_order(&rules, &media) {
        let (matched, outcomes) =
            rule_engine::evaluate_conditions(&rule.conditions, rule.match_mode, ctx);
        let conditions: Vec<rule_engine::ConditionOutcome> =
            outcomes.into_iter().map(|outcome| localizer.localize_outcome(outcome)).collect();

        // The engine's veto, read even under an exception, so the trace shows what each would do.
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

    let current_category = match media.current_root_folder.as_deref() {
        Some(folder) => {
            crate::api::overrides::folder_category(&state.pool, &media.instance_id, folder).await?
        }
        None => None,
    };
    Ok(Json(Explanation {
        current_category,
        media: media.into(),
        metadata,
        override_category,
        target_category: category,
        rules_category,
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
        identified_by,
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
// Derived from columns the sync writes: no request leaves the process and nothing is cached.
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
    /// Every quality profile the Arrs define, unused ones counted 0, so a rule can name a new
    /// one before any title has it.
    pub quality_profiles: Vec<Facet>,
    pub studios: Vec<Facet>,
    pub networks: Vec<Facet>,
    pub collections: Vec<Facet>,
    pub root_folders: Vec<Facet>,
    /// The statuses the Arrs give the titles, as they write them.
    pub statuses: Vec<Facet>,
}

/// A fixed table as the picker consumes it, each code labelled with the spelling a reader
/// knows. `count` is 0: these are not observations, and the picker shows no figure for them.
fn vocabulary(table: &[(&str, &[&str])], localizer: &Localizer) -> Vec<Facet> {
    let mut out: Vec<Facet> = table
        .iter()
        .filter_map(|(code, spellings)| {
            // A retired country, which the browser names after the one that took its code.
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
async fn column_facets(pool: &sqlx::SqlitePool, column: &'static str) -> AppResult<Vec<Facet>> {
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

/// The quality profiles of every instance, by name, with the titles holding each.
async fn profile_facets(pool: &sqlx::SqlitePool) -> AppResult<Vec<Facet>> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        "SELECT p.name AS value, COALESCE(c.n, 0) AS n
           FROM (SELECT name FROM arr_quality_profiles
                 UNION SELECT quality_profile FROM media WHERE quality_profile IS NOT NULL) p
           LEFT JOIN (SELECT quality_profile, COUNT(*) AS n FROM media
                       WHERE quality_profile IS NOT NULL GROUP BY quality_profile) c
             ON c.quality_profile = p.name
          ORDER BY n DESC, value",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(value, count)| Facet { value, label: None, count, group: None })
    .collect())
}

/// The folders the titles sit in, one entry per folder however its Arr wrote
/// it: `D:\Media\Movies\` and `d:/media/movies` are one, as the `path`
/// collation compares them.
async fn folder_facets(pool: &sqlx::SqlitePool) -> AppResult<Vec<Facet>> {
    // Grouped as written, a handful of rows, and merged here: the collation sorts every title.
    let written: Vec<(String, i64)> = sqlx::query_as(
        "SELECT current_root_folder, COUNT(*) FROM media
          WHERE current_root_folder IS NOT NULL AND current_root_folder != ''
          GROUP BY current_root_folder",
    )
    .fetch_all(pool)
    .await?;
    let mut folders: std::collections::HashMap<String, (String, i64)> = Default::default();
    for (folder, count) in written {
        let entry = folders.entry(crate::paths::key(&folder)).or_insert((folder.clone(), 0));
        entry.0 = entry.0.clone().min(folder);
        entry.1 += count;
    }
    let mut facets: Vec<Facet> = folders
        .into_values()
        .map(|(value, count)| Facet { value, label: None, count, group: None })
        .collect();
    facets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    Ok(facets)
}

/// Count the distinct values of a column holding a JSON array, commonest first.
pub(crate) async fn json_facets(
    pool: &sqlx::SqlitePool,
    column: &'static str,
) -> AppResult<Vec<Facet>> {
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

/// Say what a certification code stands for, in `label` beside the code a rule matches.
/// A code is named only when every system that gave it means the same, and one MyAnimeList
/// alone gives takes MyAnimeList's own words.
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
            // The code first: the reader is looking for the value their rule will carry.
            let label =
                words.as_ref().or(group.as_ref()).map(|name| format!("{} ({name})", facet.value));
            (rank, Facet { label, group, ..facet })
        })
        .collect();
    // A stable sort, so the most frequent code still leads its group.
    named.sort_by_key(|(rank, _)| *rank);
    named.into_iter().map(|(_, facet)| facet).collect()
}

/// Everything the sources `routing::load_context` reads say about one axis, counted per
/// title, folding case and separators but not accents. `media_column` is `None` for an axis
/// the Arr does not report, since a column `media` lacks fails the whole endpoint.
async fn metadata_facets(
    pool: &sqlx::SqlitePool,
    sources: &[&'static ProviderInfo],
    media_column: Option<&'static str>,
    cache_column: &'static str,
    json: bool,
) -> AppResult<Vec<Facet>> {
    let Some((rows, binds)) =
        facet_rows(sources, "metadata_cache", media_column, cache_column, json, false)
    else {
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

/// The certifications the enabled sources give, each with the systems that gave it, since
/// `R` means seventeen or eighteen by system. A source rating many countries gives the
/// rating `regions` pick.
async fn certification_facets(
    pool: &sqlx::SqlitePool,
    sources: &[&'static ProviderInfo],
    regions: &[String],
    localizer: &Localizer,
) -> AppResult<Vec<Facet>> {
    let cache = metadata::rated_cache(regions);
    let Some((rows, binds)) =
        facet_rows(sources, &cache, Some("certification"), "certification", false, true)
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

/// Every `(media_id, value, scale)` the enabled sources hold on one axis, as one SQL union
/// and the source ids it binds in order, or `None` when no source answers. Each source joins
/// on its own identifier, and a title known by no id is left out: SQL cannot spell its key.
fn facet_rows(
    sources: &[&'static ProviderInfo],
    cache: &str,
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
                   FROM {cache} c{join_media}{join}
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
                // A branch per kind of key, each on an index of `media`, in `local_key_of` order.
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
    // The order and regions the engine reads, so the list offered is the list matched.
    let settings = state.settings().await;
    let sources = AppState::metadata_order_from(&settings);
    let regions = AppState::certification_regions_from(&settings);
    let localizer = state.localizer().await;
    let total_media: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(pool).await?;
    let known = metadata_predicate(&settings);
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
        // No media column: the sync reads no countries off the Arr, only the cache holds them.
        origin_countries: metadata_facets(pool, &sources, None, "origin_countries", true).await?,
        certifications: certification_facets(pool, &sources, &regions, &localizer).await?,
        series_types: column_facets(pool, "series_type").await?,
        quality_profiles: profile_facets(pool).await?,
        studios: column_facets(pool, "studio").await?,
        networks: column_facets(pool, "network").await?,
        collections: column_facets(pool, "collection").await?,
        root_folders: folder_facets(pool).await?,
        statuses: name_statuses(column_facets(pool, "status").await?, &localizer),
    }))
}
