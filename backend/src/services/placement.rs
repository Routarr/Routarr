//! Where a title another service names would go, on each Arr that holds or
//! knows it.
//!
//! A title the library holds is decided as the simulation decides it. One it
//! does not hold yet is looked up in the Arr, which answers with the same
//! fields a sync reads, and decided the same way. Nothing is stored either
//! way: the question is asked before a request is approved, and a title asked
//! about is not a title in the library.

use chrono::Utc;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrMedia;
use std::collections::HashMap;

use crate::models::{ExternalId, Instance, Media, MediaMetadata};
use crate::services::metadata::{self, Addressing};
use crate::services::rate_limit::honour_retry_after;
use crate::services::routing::{self, ItemRoute};
use crate::services::rule_engine::{OVERRIDE_RULE_ID, in_order};
use crate::state::AppState;

/// What `GET /route` answers: one placement per instance that holds or knows
/// the title, and what went wrong on the others.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct Placement {
    pub answers: Vec<PlacementAnswer>,
    /// An instance that could not be asked, which stops none of the others.
    pub errors: Vec<InstanceFailure>,
}

/// Where one instance would put the title.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlacementAnswer {
    pub instance_id: String,
    pub instance_name: String,
    /// `library` when Routarr holds the title for this instance, `lookup`
    /// when only the Arr knows it.
    pub source: &'static str,
    /// The title's id in Routarr, for a title the library holds.
    pub media_id: Option<String>,
    pub title: String,
    pub year: Option<i64>,
    /// The category the rules or an exception choose, or the default one.
    pub category: String,
    /// The folder mapped to that category on this instance, null when none is.
    pub root_folder: Option<PlacementFolder>,
    pub current_root_folder: Option<String>,
    /// `add` for a title the Arr does not hold yet, else `move` or `none`.
    /// `skip` when the category has no folder on this instance.
    pub action: &'static str,
    /// The winning rule's name, null when none matched or an exception decided.
    pub rule: Option<String>,
    pub is_override: bool,
    pub confidence: f32,
    /// The metadata fields a rule tried up to the winner reads that no source
    /// answered: `genres`, `keywords`, `original_language`, `origin_countries`
    /// or `certification`. A source answering one may change the placement.
    pub unanswered_fields: Vec<&'static str>,
}

/// A destination, and whether the Arr reports it or only Routarr declares it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlacementFolder {
    pub path: String,
    /// `arr` for a root folder the Arr reports, `declared` for one only
    /// Routarr knows, which the Arr may refuse until it is added there.
    pub origin: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InstanceFailure {
    pub instance_id: String,
    pub instance_name: String,
    pub error: String,
}

/// How many placements are worked out at once. Each asks an Arr, which asks
/// its own metadata service, and with `enrich` every source: a bot looping on
/// this route would otherwise hold them all at once.
static AT_ONCE: Semaphore = Semaphore::const_new(2);

/// Where the title goes on each enabled Arr of its kind, or on `instance`.
///
/// What a title the Arr does not hold yet would be added with, which the rules
/// read: the lookup answers the Arr's defaults instead, a series not monitored
/// and of the standard type, never added.
#[derive(Debug, Default)]
pub struct AddedWith {
    pub tags: Vec<String>,
    pub series_type: Option<String>,
    pub monitored: Option<bool>,
}

/// `added` is what a title the Arr does not hold would be added with. With
/// `enrich`, every source that can answer and has no cached answer is asked
/// now.
pub async fn place(
    state: &AppState,
    media_type: &str,
    id: &ExternalId,
    instance: Option<&str>,
    added: &AddedWith,
    enrich: bool,
) -> AppResult<Placement> {
    let _turn = AT_ONCE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("the placement queue closed: {e}")))?;
    let kind = if media_type == "series" { "sonarr" } else { "radarr" };
    let instances: Vec<Instance> = state
        .instances(true)
        .await?
        .into_iter()
        .filter(|candidate| candidate.instance_type == kind)
        .filter(|candidate| instance.is_none_or(|wanted| wanted == candidate.id))
        .collect();
    let held = routing::media_by_external_id(&state.pool, media_type, id, instance).await?;

    let mut placement = Placement::default();
    for instance in instances {
        let (media, source) = match held.iter().find(|media| media.instance_id == instance.id) {
            Some(media) => (media.clone(), "library"),
            None => match looked_up(state, &instance, id).await {
                Ok(Some(item)) => (unheld(state, &instance, id, item, added).await?, "lookup"),
                Ok(None) => continue,
                Err(e) => {
                    if e.is_internal() {
                        tracing::warn!(instance = %instance.name, "A placement lookup failed: {e}");
                    }
                    placement.errors.push(InstanceFailure {
                        instance_id: instance.id.clone(),
                        instance_name: instance.name.clone(),
                        error: e.public_message(),
                    });
                    continue;
                }
            },
        };
        let fresh =
            if enrich { answered_now(state, &media).await } else { routing::Fresh::default() };
        let decided = routing::route_one_with(&state.pool, &media, Utc::now(), fresh).await?;
        placement.answers.push(answer(state, &instance, &media, source, decided).await?);
    }

    if placement.answers.is_empty() && placement.errors.is_empty() {
        return Err(AppError::NotFound(format!(
            "No enabled {kind} instance knows the {media_type} with {} {}.",
            id.column(),
            id.value()
        )));
    }
    Ok(placement)
}

async fn looked_up(
    state: &AppState,
    instance: &Instance,
    id: &ExternalId,
) -> AppResult<Option<ArrMedia>> {
    state.adapter(instance)?.lookup(id).await
}

/// The title as the Arr describes it, as a media row that is never written.
///
/// Its id names the instance and the title, so no exception, which is keyed by
/// a real row, can apply to it. A title the Arr already holds keeps the
/// folder, the files and the state the Arr reports, its tags named through the
/// instance's tag catalogue: the sync has only not read it yet. One it does not
/// hold is judged on what adding it sets.
async fn unheld(
    state: &AppState,
    instance: &Instance,
    id: &ExternalId,
    item: ArrMedia,
    added: &AddedWith,
) -> AppResult<Media> {
    let (tags, monitored, added_at, series_type) = if item.arr_id != 0 {
        let catalogue: HashMap<i64, String> =
            sqlx::query_as("SELECT arr_id, label FROM arr_tags WHERE instance_id = ?")
                .bind(&instance.id)
                .fetch_all(&state.pool)
                .await?
                .into_iter()
                .collect();
        let labels: Vec<String> =
            item.tag_ids.iter().filter_map(|tag| catalogue.get(tag).cloned()).collect();
        (labels, item.monitored, item.added, item.series_type)
    } else {
        (
            added.tags.clone(),
            added.monitored.unwrap_or(true),
            Some(routing::format_timestamp(Utc::now())),
            added.series_type.clone().or(item.series_type),
        )
    };
    Ok(Media {
        id: format!("lookup:{}:{}:{}", instance.id, id.column(), id.value()),
        instance_id: instance.id.clone(),
        arr_id: item.arr_id,
        media_type: item.media_type.to_string(),
        title: item.title,
        sort_title: item.sort_title,
        year: item.year,
        tmdb_id: item.tmdb_id,
        tvdb_id: item.tvdb_id,
        imdb_id: item.imdb_id,
        current_path: item.path,
        current_root_folder: item.root_folder_path,
        monitored,
        // Radarr's lookup carries no `hasFile`, which reads as a file: a title
        // the Arr does not hold has none.
        has_files: item.arr_id != 0 && item.has_files,
        status: item.status,
        added_at,
        series_type,
        size_on_disk: item.size_on_disk,
        season_count: item.season_count,
        tags: serde_json::to_string(&tags).ok(),
        genres: serde_json::to_string(&item.genres).ok(),
        original_language: item.original_language,
        certification: item.certification,
        last_synced_at: None,
    })
}

/// What each source able to answer says now about a title it has no cached
/// answer for. A source that fails is left out: the placement is worked out
/// with what the others said, and `unanswered_fields` shows the gap.
async fn answered_now(state: &AppState, media: &Media) -> routing::Fresh {
    let mut fresh = routing::Fresh::default();
    let providers = state.metadata_order().await;
    let Ok(identifiers) = metadata::load_identifiers_of(&state.pool, media).await else {
        return fresh;
    };
    let Ok(cached) = metadata::load_cache_of(&state.pool, media, &providers, &identifiers).await
    else {
        return fresh;
    };
    for source in state.metadata_sources().await {
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
        let pace = source.pace();
        let external = match (known, source.addressing()) {
            (Some(external), _) => external,
            (None, Addressing::Search) if missed => continue,
            (None, Addressing::Search) => {
                pace.acquire().await;
                let resolved = source.resolve(&media.title, media.year, &media.media_type).await;
                honour_retry_after(&pace, &resolved).await;
                match resolved {
                    Ok(Some(external)) => {
                        fresh.identifiers.insert(searched, Some(external.clone()));
                        external
                    }
                    _ => continue,
                }
            }
            (None, _) => continue,
        };
        pace.acquire().await;
        let fetched = source.fetch(&external, &media.media_type).await;
        honour_retry_after(&pace, &fetched).await;
        if let Ok(answer) = fetched {
            fresh.metadata.insert(key(&external), answer);
        }
    }
    fresh
}

async fn answer(
    state: &AppState,
    instance: &Instance,
    media: &Media,
    source: &'static str,
    decided: ItemRoute,
) -> AppResult<PlacementAnswer> {
    let ItemRoute { rules, route, .. } = decided;
    let winner = route.evaluation.winner.as_ref();
    let is_override = winner.is_some_and(|winner| winner.rule_id == OVERRIDE_RULE_ID);
    let root_folder = match &route.target {
        Some(path) => Some(PlacementFolder {
            path: path.clone(),
            origin: folder_origin(state, &instance.id, path).await?,
        }),
        None => None,
    };
    let action = match (route.action, media.arr_id) {
        ("skip", _) => "skip",
        (_, 0) => "add",
        (action, _) => action,
    };
    Ok(PlacementAnswer {
        instance_id: instance.id.clone(),
        instance_name: instance.name.clone(),
        source,
        media_id: (source == "library").then(|| media.id.clone()),
        title: media.title.clone(),
        year: media.year,
        category: route.category.clone(),
        root_folder,
        current_root_folder: media.current_root_folder.clone(),
        action,
        rule: winner.filter(|_| !is_override).map(|winner| winner.rule_name.clone()),
        is_override,
        confidence: winner.map_or(0.0, |winner| winner.confidence),
        unanswered_fields: if is_override {
            Vec::new()
        } else {
            unanswered(&rules, media, route.metadata.as_ref(), winner.map(|w| w.rule_id.as_str()))
        },
    })
}

/// The fields the rules tried up to the winner read, and no source answered.
/// Past the winner nothing is read that could change the placement: a rule
/// below it loses on priority whatever it finds.
fn unanswered(
    rules: &[crate::models::Rule],
    media: &Media,
    metadata: Option<&MediaMetadata>,
    winner: Option<&str>,
) -> Vec<&'static str> {
    let answered = |field: &str| metadata.is_some_and(|m| m.field_sources.contains_key(field));
    let mut missing: Vec<&'static str> = Vec::new();
    for rule in in_order(rules, media) {
        for condition in rule.conditions.iter().chain(&rule.exclusions) {
            if let Some(field) = condition.metadata_field()
                && !answered(field.as_str())
                && !missing.contains(&field.as_str())
            {
                missing.push(field.as_str());
            }
        }
        if Some(rule.id.as_str()) == winner {
            break;
        }
    }
    missing.sort_unstable();
    missing
}

async fn folder_origin(state: &AppState, instance_id: &str, path: &str) -> AppResult<String> {
    let origin: Option<String> = sqlx::query_scalar(
        "SELECT origin FROM root_folders
          WHERE instance_id = ? AND path = ? COLLATE path",
    )
    .bind(instance_id)
    .bind(path)
    .fetch_optional(&state.pool)
    .await?;
    Ok(origin.unwrap_or_else(|| "arr".to_string()))
}
