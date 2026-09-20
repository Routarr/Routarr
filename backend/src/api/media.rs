//! Media explorer and per-item explainability.

use super::Json;
use axum::extract::{Path, Query, State};
use chrono::Utc;
use serde::Serialize;
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

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MediaListItem {
    pub id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub arr_id: i64,
    pub media_type: String,
    pub title: String,
    pub year: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub current_root_folder: Option<String>,
    pub monitored: bool,
    pub has_files: bool,
    pub status: Option<String>,
    pub last_synced_at: Option<String>,
    /// Category proposed by the most recent, still-current decision.
    pub computed_category: Option<String>,
    pub override_category: Option<String>,
    pub has_metadata: bool,
}

/// SQL predicate for "at least one enabled source describes this item".
///
/// Built from the source list rather than hard-coded: with every source off the
/// answer is `0`, not "whatever happens to be left in the cache".
/// "Something a rule could read is known about this item", in SQL.
///
/// Four places asked this and three of them spelled it differently: the library
/// column, the diagnostics count, the warning beside it. They must agree — a
/// badge saying "metadata missing" over a count saying otherwise is how a
/// diagnostic stops being read — so it is written once and they all splice it.
///
/// Three things it has to get right. Only the *enabled* sources count, exactly
/// as `routing::load_context` reads them, or a source switched off yesterday
/// still speaks for an item. All three identifier namespaces count, or a series
/// enriched by TheTVDB and carrying no `tmdb_id` reads as undescribed for ever.
/// And a cached row is not a cached *answer*: a synopsis is readable by no
/// condition, so the five fields `MetadataField` names are what is looked for.
///
/// The source ids are `&'static str` from the catalogue, never anything a
/// caller sent, which is what makes splicing them safe.
pub(crate) fn metadata_predicate(
    providers: &[&'static crate::services::metadata::ProviderInfo],
) -> String {
    const MATCHABLE: &str = "(COALESCE(c.genres, '[]') != '[]'
                             OR COALESCE(c.keywords, '[]') != '[]'
                             OR c.original_language IS NOT NULL
                             OR COALESCE(c.origin_countries, '[]') != '[]'
                             OR c.certification IS NOT NULL)";

    let mut clauses: Vec<String> = Vec::new();
    if providers.iter().any(|p| p.id == metadata::ARR) {
        clauses.push("(m.genres IS NOT NULL AND m.genres != '[]')".to_string());
    }
    let fetched: Vec<&'static str> =
        providers.iter().map(|p| p.id).filter(|id| *id != metadata::ARR).collect();
    if !fetched.is_empty() {
        let sources = fetched.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM metadata_cache c
                      WHERE c.source IN ({sources})
                        AND c.media_type = m.media_type
                        AND (c.external_id = CAST(m.tmdb_id AS TEXT)
                             OR c.external_id = CAST(m.tvdb_id AS TEXT)
                             OR c.external_id = m.imdb_id)
                        AND {MATCHABLE})"
        ));
    }
    if clauses.is_empty() { "0".to_string() } else { clauses.join(" OR ") }
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
    if let Some(v) = &query.search {
        filters.push_str(" AND m.title LIKE ? ESCAPE '\\'");
        binds.push(format!("%{}%", crate::db::escape_like(v)));
    }
    if let Some(v) = &query.category {
        // Filter on the category the engine last proposed, not on a stored column.
        filters.push_str(
            " AND EXISTS (SELECT 1 FROM decisions d
                          WHERE d.media_id = m.id AND d.superseded = 0 AND d.target_category = ?)",
        );
        binds.push(v.clone());
    }
    if query.unmatched.unwrap_or(false) {
        filters.push_str(
            " AND NOT EXISTS (SELECT 1 FROM decisions d
                              WHERE d.media_id = m.id AND d.superseded = 0 AND d.matched_rule_id IS NOT NULL)",
        );
    }

    // "Something is known about this item", counted over the *enabled* sources
    // only. Reading it off any cached row would contradict `has_metadata` in the
    // engine, which honours the order — a list saying "metadata" next to a rule
    // saying there is none is the kind of disagreement nobody debugs twice.
    let has_metadata = metadata_predicate(&state.metadata_order().await);

    // The correlated sub-selects keep this to two queries instead of the
    // per-row lookups the explorer would otherwise need.
    let list_sql = format!(
        "SELECT m.id, m.instance_id, i.name AS instance_name, m.arr_id, m.media_type, m.title,
                m.year, m.tmdb_id, m.current_root_folder, m.monitored, m.has_files, m.status,
                m.last_synced_at,
                (SELECT d.target_category FROM decisions d
                  WHERE d.media_id = m.id AND d.superseded = 0
                  ORDER BY d.decided_at DESC LIMIT 1) AS computed_category,
                (SELECT o.target_category FROM overrides o WHERE o.media_id = m.id) AS override_category,
                ({has_metadata}) AS has_metadata
         FROM media m
         JOIN instances i ON m.instance_id = i.id
         WHERE 1=1{filters}
         ORDER BY m.title ASC LIMIT ? OFFSET ?"
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

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let media = load_media(&state, &id).await?;

    let instance_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM instances WHERE id = ?")
            .bind(&media.instance_id)
            .fetch_optional(&state.pool)
            .await?;

    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    let override_entry: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT target_category, reason, locked FROM overrides WHERE media_id = ?")
            .bind(&media.id)
            .fetch_optional(&state.pool)
            .await?;

    Ok(Json(serde_json::json!({
        "media": media,
        "instance_name": instance_name,
        "metadata": metadata,
        "override": override_entry.map(|(category, reason, locked)| serde_json::json!({
            "target_category": category,
            "reason": reason,
            "locked": locked,
        })),
    })))
}

#[derive(Debug, Serialize)]
pub struct Explanation {
    pub media: Media,
    pub metadata: Option<MediaMetadata>,
    pub override_category: Option<String>,
    pub target_category: String,
    pub target_root_folder: Option<String>,
    pub action: String,
    pub confidence: f32,
    pub winning_rule: Option<String>,
    /// Per-condition outcome for every rule that was considered.
    pub rule_traces: Vec<RuleTrace>,
}

#[derive(Debug, Serialize)]
pub struct RuleTrace {
    pub rule_id: String,
    pub rule_name: String,
    pub priority: i64,
    pub category: String,
    pub matched: bool,
    pub excluded_by: Option<String>,
    pub outcome: String,
    /// Every condition, with its wording already resolved for this language.
    pub conditions: Vec<rule_engine::ConditionOutcome>,
}

/// Explain, condition by condition, why a media item lands where it does.
///
/// Evaluated live rather than read from a stored decision, so it also explains
/// rules that were edited since the last run.
pub async fn explain(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Explanation>> {
    let media = load_media(&state, &id).await?;
    let localizer: Localizer = state.localizer().await;

    let metadata = enrichment::resolve_for_media(&state, &media).await?;

    let rules = routing::load_rules(&state.pool).await?;
    let override_category: Option<String> =
        sqlx::query_scalar("SELECT target_category FROM overrides WHERE media_id = ?")
            .bind(&media.id)
            .fetch_optional(&state.pool)
            .await?;

    let ctx = EvalContext { media: &media, metadata: metadata.as_ref(), now: Utc::now() };
    let evaluation = rule_engine::evaluate_rules(ctx, &rules, override_category.as_deref());

    let default_category = AppState::default_category(&state.pool).await;

    let target_category =
        evaluation.winner.as_ref().map(|w| w.category.clone()).unwrap_or(default_category);

    let target_root_folder: Option<String> = sqlx::query_scalar(
        "SELECT path FROM root_folders WHERE instance_id = ? AND category = ? LIMIT 1",
    )
    .bind(&media.instance_id)
    .bind(&target_category)
    .fetch_optional(&state.pool)
    .await?;

    let action = match &target_root_folder {
        None => "skip",
        Some(target) => {
            let current = media.current_root_folder.as_deref().unwrap_or("");
            if rule_engine::normalize_path(current) == rule_engine::normalize_path(target) {
                "none"
            } else {
                "move"
            }
        }
    };

    // Trace every applicable rule, not just the winner: seeing why the *other*
    // rules did not fire is usually the actual question.
    let winner_id = evaluation.winner.as_ref().map(|w| w.rule_id.clone());
    let mut rule_traces = Vec::new();

    for rule in &rules {
        if !rule.enabled
            || !rule.covers_media_type(&media.media_type)
            || !rule.covers_instance(&media.instance_id)
        {
            continue;
        }

        let conditions: Vec<rule_engine::ConditionOutcome> = rule
            .conditions
            .iter()
            .map(|c| localizer.localize_outcome(rule_engine::evaluate_single_condition(c, ctx)))
            .collect();

        let matched = match rule.match_mode {
            MatchMode::All => conditions.iter().all(|c| c.matched),
            MatchMode::Any => conditions.iter().any(|c| c.matched),
        };

        let excluded_by = if matched {
            rule.exclusions
                .iter()
                .map(|c| rule_engine::evaluate_single_condition(c, ctx))
                .find(|c| c.matched)
                .map(|c| localizer.localize_outcome(c).expected)
        } else {
            None
        };

        let outcome = if Some(&rule.id) == winner_id.as_ref() {
            "winner"
        } else if excluded_by.is_some() {
            "excluded"
        } else if matched {
            "matched_lower_priority"
        } else {
            "not_matched"
        };

        rule_traces.push(RuleTrace {
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            priority: rule.priority,
            category: rule.target_category.clone(),
            matched,
            excluded_by,
            outcome: outcome.to_string(),
            conditions,
        });
    }

    Ok(Json(Explanation {
        media,
        metadata,
        override_category,
        target_category,
        target_root_folder,
        action: action.to_string(),
        confidence: evaluation.winner.as_ref().map(|w| w.confidence).unwrap_or(0.0),
        winning_rule: evaluation.winner.as_ref().map(|w| {
            if w.rule_id == rule_engine::OVERRIDE_RULE_ID {
                localizer.translate("ManualOverrideRuleName", &[])
            } else {
                w.rule_name.clone()
            }
        }),
        rule_traces,
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
#[derive(Debug, serde::Serialize)]
pub struct Vocabularies {
    pub original_languages: Vec<Facet>,
    pub origin_countries: Vec<Facet>,
}

/// One value a condition can hold, and how many items carry it.
#[derive(Debug, serde::Serialize)]
pub struct Facet {
    /// What a rule stores and the engine compares. Never the displayed text: a
    /// language is matched on `ja`, however it is shown.
    pub value: String,
    /// What to show instead of `value`, where the two differ. Absent for a
    /// genre, which is its own label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub count: i64,
}

/// What the library actually contains, per axis a condition can read.
///
/// Without it, writing a rule means guessing which genres, languages and
/// certifications the library holds. A rule written against a value that is not
/// there matches nothing, and reads on screen exactly like a rule that
/// correctly matches nothing.
///
/// Read-only and derived entirely from columns the sync already writes: no
/// request leaves the process, and nothing here is cached, because the answer
/// is only as old as the last sync either way.
#[derive(Debug, serde::Serialize)]
pub struct LibraryFacets {
    pub total_media: i64,
    /// Values a condition may hold that the library does not define.
    ///
    /// Genres, certifications and tags mean what the library says they mean, so
    /// what it carries is the whole answer. A language or a country does not:
    /// the rule is written against an ISO code, the vocabulary is fixed
    /// elsewhere, and offering only the five languages that happen to be synced
    /// would hide the other forty-eight — and leave the code to be guessed.
    pub vocabularies: Vocabularies,
    /// Carrying neither a genre nor an original language — invisible to every
    /// condition that reads metadata, which is the failure that looks like a
    /// broken rule.
    pub without_metadata: i64,
    pub genres: Vec<Facet>,
    pub original_languages: Vec<Facet>,
    /// Counted like the languages, and for the same reason: the closed
    /// vocabulary says what a code *means*, the library says which ones it
    /// actually holds, and a rule is written far better against the second.
    pub origin_countries: Vec<Facet>,
    pub certifications: Vec<Facet>,
    pub tags: Vec<Facet>,
    pub series_types: Vec<Facet>,
    pub root_folders: Vec<Facet>,
}

/// A fixed table as the picker consumes it: the value a rule stores, labelled
/// with the spelling a reader recognises. `count` is 0 throughout — these are
/// not observations, and the picker renders no figure for them.
fn vocabulary(table: &[(&str, &[&str])]) -> Vec<Facet> {
    let mut out: Vec<Facet> = table
        .iter()
        .filter_map(|(code, spellings)| {
            spellings.first().map(|name| Facet {
                value: (*code).to_string(),
                label: Some(format!("{} ({code})", capitalise(name))),
                count: 0,
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

/// Count the distinct values of one column, commonest first.
async fn column_facets(pool: &sqlx::SqlitePool, column: &str) -> AppResult<Vec<Facet>> {
    // `column` is interpolated, so it must stay a literal from this file and
    // never anything a caller can influence — the same rule the metadata
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
        .map(|(value, count)| Facet { value, label: None, count })
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
        .map(|(value, count)| Facet { value, label: None, count })
        .collect())
}

/// Everything the *enabled* sources say about one axis, not just the Arr row.
///
/// The rule builder offers this list, so it has to hold what the engine can
/// actually match: a genre TMDb supplied is matched by a rule and would be
/// missing from a list read off `media` alone. The `arr` branch is the media
/// row, the other is `metadata_cache`, and a source the user disabled
/// contributes to neither — exactly as `routing::load_context` reads them.
///
/// Counted with `COUNT(DISTINCT m.id)`, since one item is described by several
/// sources at once. Spellings that differ only by case or by a separator are one
/// value, as they are to [`normalise_value`](crate::services::rule_engine::normalise_value); accents are not folded here, SQLite
/// having no way to, so `Comédie` and `Comedie` stay two entries in the list
/// while the engine still matches both.
/// Say what a certification code stands for, where that is not in dispute.
///
/// `U`, `TP` and `TV-PG` say nothing to most readers, and this panel exists to
/// show what the library holds. The code stays the *value* — it is what a rule
/// matches on — and the name is only ever what is shown, which is why it goes
/// in `label` beside it rather than replacing it.
fn name_certifications(facets: Vec<Facet>, localizer: &Localizer) -> Vec<Facet> {
    use crate::integrations::certification::{Meaning, meaning};
    facets
        .into_iter()
        .map(|facet| {
            let name = meaning(&facet.value).map(|m| match m {
                Meaning::AllAges => localizer.translate("CertAllAges", &[]),
                Meaning::Guidance => localizer.translate("CertGuidance", &[]),
                Meaning::From(age) => {
                    localizer.translate("CertFromAge", &[("age", &age.to_string())])
                }
                Meaning::NotRated => localizer.translate("CertNotRated", &[]),
            });
            // The code first and the meaning after it: the reader is looking
            // for the value their rule will carry.
            Facet { label: name.map(|name| format!("{} ({name})", facet.value)), ..facet }
        })
        .collect()
}

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
    let fetched: Vec<&str> =
        sources.iter().map(|p| p.id).filter(|id| *id != metadata::ARR).collect();
    let arr = sources.iter().any(|p| p.id == metadata::ARR) && media_column.is_some();
    if !arr && fetched.is_empty() {
        return Ok(Vec::new());
    }

    // Each branch yields (media id, raw value). A JSON column is expanded with
    // `json_each`, a scalar one read directly; every fragment below is a literal
    // from this file and every value is bound.
    let unnest = |alias: &str, column: &str| {
        if json { format!(", json_each({alias}.{column}) j") } else { String::new() }
    };
    let value = |alias: &str, column: &str| {
        if json { "j.value".to_string() } else { format!("{alias}.{column}") }
    };

    let mut branches: Vec<String> = Vec::new();
    if let Some(media_column) = media_column.filter(|_| arr) {
        branches.push(format!(
            "SELECT m.id AS media_id, {v} AS value FROM media m{join}
              WHERE m.{media_column} IS NOT NULL AND m.{media_column} != ''",
            v = value("m", media_column),
            join = unnest("m", media_column),
        ));
    }
    if !fetched.is_empty() {
        // One equality per identifier namespace, each served by an index on
        // `media`. An OR across the three, with the cast on the media side, is
        // a scan of the library for every cache row — 130 s on 20 000 titles.
        // The GLOB guards keep `CAST('tt0111161' AS INTEGER)`, which is 0, from
        // meeting a real id.
        let holes = crate::db::placeholders(fetched.len());
        for (on, guard) in [
            ("m.tmdb_id = CAST(c.external_id AS INTEGER)", "c.external_id GLOB '[0-9]*'"),
            ("m.tvdb_id = CAST(c.external_id AS INTEGER)", "c.external_id GLOB '[0-9]*'"),
            ("m.imdb_id = c.external_id", "c.external_id GLOB 'tt*'"),
        ] {
            branches.push(format!(
                "SELECT m.id AS media_id, {v} AS value
                   FROM metadata_cache c
                   JOIN media m ON m.media_type = c.media_type AND {on}{join}
                  WHERE c.source IN ({holes}) AND {guard}
                    AND c.{cache_column} IS NOT NULL AND c.{cache_column} != ''",
                v = value("c", cache_column),
                join = unnest("c", cache_column),
            ));
        }
    }

    let sql = format!(
        "SELECT MIN(v.value) AS value, COUNT(DISTINCT v.media_id) AS n
           FROM ({}) v
          WHERE v.value IS NOT NULL AND v.value != ''
          GROUP BY lower(replace(replace(v.value, '-', ' '), '_', ' '))
          ORDER BY n DESC, value",
        branches.join(" UNION ALL ")
    );

    let mut query = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql.as_str()));
    for source in &fetched {
        query = query.bind(*source);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count)| Facet { value, label: None, count })
        .collect())
}

pub async fn facets(State(state): State<AppState>) -> AppResult<Json<LibraryFacets>> {
    let pool = &state.pool;
    // The order the engine reads, so the list offered and the list matched are
    // the same one.
    let sources = state.metadata_order().await;
    let total_media: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(pool).await?;
    // No *enabled* source describes these, which is what makes them invisible to
    // every condition reading metadata. Counting the media row alone would call
    // an item enriched by TMDb undescribed, and contradict the lists below.
    let fetched: Vec<&str> =
        sources.iter().map(|p| p.id).filter(|id| *id != metadata::ARR).collect();
    let arr_clause = if sources.iter().any(|p| p.id == metadata::ARR) {
        "(m.genres IS NULL OR m.genres = '' OR m.genres = '[]')
         AND (m.original_language IS NULL OR m.original_language = '')"
    } else {
        "1 = 1"
    };
    let cache_clause = if fetched.is_empty() {
        "1 = 1".to_string()
    } else {
        // Three seeks rather than one OR: the cache index is led by
        // `external_id`, which each equality supplies from the media row.
        let holes = crate::db::placeholders(fetched.len());
        ["CAST(m.tmdb_id AS TEXT)", "CAST(m.tvdb_id AS TEXT)", "m.imdb_id"]
            .iter()
            .map(|key| {
                format!(
                    "NOT EXISTS (
                       SELECT 1 FROM metadata_cache c
                        WHERE c.external_id = {key} AND c.media_type = m.media_type
                          AND c.source IN ({holes})
                          AND ((c.genres != '' AND c.genres != '[]')
                            OR (c.original_language IS NOT NULL AND c.original_language != '')))"
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let sql = format!("SELECT COUNT(*) FROM media m WHERE {arr_clause} AND {cache_clause}");
    let mut query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql.as_str()));
    // Once per NOT EXISTS: the placeholders repeat with the clause.
    for _ in 0..3 {
        for source in &fetched {
            query = query.bind(*source);
        }
    }
    let without_metadata: i64 = query.fetch_one(pool).await?;

    Ok(Json(LibraryFacets {
        total_media,
        vocabularies: Vocabularies {
            original_languages: vocabulary(language::LANGUAGES),
            origin_countries: vocabulary(language::COUNTRIES),
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
        certifications: name_certifications(
            metadata_facets(pool, &sources, Some("certification"), "certification", false).await?,
            &state.localizer().await,
        ),
        series_types: column_facets(pool, "series_type").await?,
        root_folders: column_facets(pool, "current_root_folder").await?,
    }))
}
