//! A single façade over Radarr and Sonarr.
//!
//! One common model with an adapter per media type, so the logic is not
//! duplicated: nothing outside this module branches on the instance type.
//! Sync, health and the executor all talk to this enum instead of branching on
//! `instance_type` and repeating the same code twice.

use reqwest::Client;

use crate::error::{AppError, AppResult};
use crate::integrations::radarr::RadarrClient;
use crate::integrations::sonarr::SonarrClient;
use crate::models::Instance;

/// Routarr's media type discriminators, shared with the database schema.
pub const MOVIE: &str = "movie";
pub const SERIES: &str = "series";

/// A media item as seen by either Arr, normalized for Routarr's schema.
#[derive(Debug, Clone)]
pub struct ArrMedia {
    pub arr_id: i64,
    pub media_type: &'static str,
    pub title: String,
    pub sort_title: Option<String>,
    pub year: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub path: Option<String>,
    pub root_folder_path: Option<String>,
    pub monitored: bool,
    pub has_files: bool,
    pub status: Option<String>,
    pub added: Option<String>,
    /// Sonarr's own classification (`standard` / `anime` / `daily`). `None` for
    /// movies: Radarr has no equivalent.
    pub series_type: Option<String>,
    pub size_on_disk: Option<i64>,
    /// Seasons excluding specials, so "more than five seasons" means what the
    /// user means. `None` for movies.
    pub season_count: Option<i64>,
    /// Tag ids as the Arr reports them; resolved to labels at sync time.
    pub tag_ids: Vec<i64>,
    /// Metadata the Arr already carries. It makes the `arr` source of
    /// `services::metadata` answer without a single extra request, which is
    /// what lets Routarr classify a library with no TMDb key at all.
    pub genres: Vec<String>,
    /// Normalised to the ISO 639-1 code TMDb would have returned, so both
    /// sources speak the same vocabulary.
    pub original_language: Option<String>,
    pub certification: Option<String>,
}

/// A tag as defined in an Arr.
#[derive(Debug, Clone)]
pub struct ArrTag {
    pub arr_id: i64,
    pub label: String,
}

/// A root folder as seen by either Arr.
#[derive(Debug, Clone)]
pub struct ArrRootFolder {
    pub arr_id: i64,
    pub path: String,
    pub free_space: Option<i64>,
    pub accessible: bool,
}

/// Connectivity probe result.
#[derive(Debug, Clone)]
pub struct ArrStatus {
    pub version: String,
    pub app_name: Option<String>,
}

/// Dispatching wrapper around the two concrete clients.
#[derive(Debug, Clone)]
pub enum ArrAdapter {
    Radarr(RadarrClient),
    Sonarr(SonarrClient),
}

impl ArrAdapter {
    /// Build the adapter for a stored instance, decrypting its API key.
    pub fn for_instance(client: Client, instance: &Instance, api_key: &str) -> AppResult<Self> {
        match instance.instance_type.as_str() {
            "radarr" => Ok(Self::Radarr(RadarrClient::new(client, &instance.base_url, api_key))),
            "sonarr" => Ok(Self::Sonarr(SonarrClient::new(client, &instance.base_url, api_key))),
            other => Err(AppError::BadRequest(format!("Unknown instance type '{other}'"))),
        }
    }

    pub async fn test_connection(&self) -> AppResult<ArrStatus> {
        match self {
            Self::Radarr(c) => {
                let s = c.test_connection().await?;
                Ok(ArrStatus { version: s.version, app_name: s.app_name })
            }
            Self::Sonarr(c) => {
                let s = c.test_connection().await?;
                Ok(ArrStatus { version: s.version, app_name: s.app_name })
            }
        }
    }

    /// Whether the Arr can see this directory, in *its* filesystem namespace.
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        match self {
            Self::Radarr(c) => c.directory_exists(path).await,
            Self::Sonarr(c) => c.directory_exists(path).await,
        }
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolder>> {
        match self {
            Self::Radarr(c) => Ok(c
                .get_root_folders()
                .await?
                .into_iter()
                .map(|rf| ArrRootFolder {
                    arr_id: rf.id,
                    path: rf.path,
                    free_space: rf.free_space,
                    accessible: rf.accessible.unwrap_or(true),
                })
                .collect()),
            Self::Sonarr(c) => Ok(c
                .get_root_folders()
                .await?
                .into_iter()
                .map(|rf| ArrRootFolder {
                    arr_id: rf.id,
                    path: rf.path,
                    free_space: rf.free_space,
                    accessible: rf.accessible.unwrap_or(true),
                })
                .collect()),
        }
    }

    pub async fn get_media(&self) -> AppResult<Vec<ArrMedia>> {
        match self {
            Self::Radarr(c) => Ok(c.get_movies().await?.into_iter().map(movie_to_media).collect()),
            Self::Sonarr(c) => Ok(c.get_series().await?.into_iter().map(series_to_media).collect()),
        }
    }

    /// One item by its Arr id, `None` when the Arr no longer has it.
    ///
    /// The webhook path reads one item per event, and Radarr sends one event
    /// per imported file: listing the whole library there would cost a full
    /// download per episode of a season.
    pub async fn get_media_one(&self, arr_id: i64) -> AppResult<Option<ArrMedia>> {
        let found = match self {
            Self::Radarr(c) => c.get_movie(arr_id).await.map(|m| m.map(movie_to_media)),
            Self::Sonarr(c) => c.get_series_one(arr_id).await.map(|s| s.map(series_to_media)),
        };
        match found {
            Err(AppError::ExternalApi { status: 404, .. }) => Ok(None),
            other => other,
        }
    }

    /// Get the instance's tag catalogue, so ids can be resolved to labels.
    ///
    /// An Arr that predates tags, or one whose endpoint fails, yields an empty
    /// catalogue rather than failing the sync: tags are one signal among many.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTag>> {
        let tags = match self {
            Self::Radarr(c) => c.get_tags().await?,
            Self::Sonarr(c) => c.get_tags().await?,
        };
        Ok(tags.into_iter().map(|t| ArrTag { arr_id: t.id, label: t.label }).collect())
    }

    /// Move a batch of items to a root folder.
    ///
    /// Radarr accepts the whole batch in one call; Sonarr has no bulk editor, so
    /// each series is patched individually and per-item failures are reported
    /// rather than aborting the rest of the batch.
    pub async fn move_to_root_folder(
        &self,
        arr_ids: &[i64],
        root_folder_path: &str,
        move_files: bool,
    ) -> Vec<(i64, AppResult<()>)> {
        match self {
            Self::Radarr(c) => {
                match c.update_movies_root_folder(arr_ids, root_folder_path, move_files).await {
                    Ok(()) => arr_ids.iter().map(|id| (*id, Ok(()))).collect(),
                    Err(e) => arr_ids.iter().map(|id| (*id, Err(clone_error(&e)))).collect(),
                }
            }
            Self::Sonarr(c) => {
                let mut results = Vec::with_capacity(arr_ids.len());
                for id in arr_ids {
                    results
                        .push((*id, c.update_series_path(*id, root_folder_path, move_files).await));
                }
                results
            }
        }
    }

    /// Ask the Arr to rescan the moved items.
    pub async fn refresh(&self, arr_ids: &[i64]) -> AppResult<()> {
        match self {
            Self::Radarr(c) => c.refresh_movies(arr_ids).await,
            Self::Sonarr(c) => {
                for id in arr_ids {
                    c.refresh_series(*id).await?;
                }
                Ok(())
            }
        }
    }
}

/// `AppError` is not `Clone` (it wraps `sqlx`/`reqwest` errors); rebuild the
/// variants the Arr clients can actually produce so a bulk failure can be
/// reported per item.
fn clone_error(e: &AppError) -> AppError {
    match e {
        AppError::ExternalApi { service, status, message, retry_after } => AppError::ExternalApi {
            service: service.clone(),
            status: *status,
            message: message.clone(),
            retry_after: *retry_after,
        },
        other => AppError::Internal(other.to_string()),
    }
}

fn movie_to_media(m: crate::integrations::radarr::RadarrMovie) -> ArrMedia {
    ArrMedia {
        arr_id: m.id,
        media_type: MOVIE,
        title: m.title,
        sort_title: m.sort_title,
        year: m.year,
        tmdb_id: m.tmdb_id,
        tvdb_id: None,
        imdb_id: m.imdb_id,
        path: m.path,
        root_folder_path: m.root_folder_path,
        monitored: m.monitored,
        has_files: m.has_file,
        status: m.status,
        added: m.added,
        series_type: None,
        size_on_disk: m.size_on_disk,
        season_count: None,
        tag_ids: m.tags,
        genres: m.genres,
        original_language: m
            .original_language
            .and_then(|l| l.name)
            .and_then(|name| super::language::normalise(&name)),
        certification: m.certification,
    }
}

fn series_to_media(s: crate::integrations::sonarr::SonarrSeries) -> ArrMedia {
    let has_files = s.has_files();
    let season_count = s.season_count();
    ArrMedia {
        arr_id: s.id,
        media_type: SERIES,
        title: s.title,
        sort_title: s.sort_title,
        year: s.year,
        tmdb_id: s.tmdb_id,
        tvdb_id: s.tvdb_id,
        imdb_id: s.imdb_id,
        path: s.path,
        root_folder_path: s.root_folder_path,
        monitored: s.monitored,
        has_files,
        status: s.status,
        added: s.added,
        series_type: s.series_type,
        size_on_disk: s.statistics.as_ref().and_then(|st| st.size_on_disk),
        season_count: Some(season_count),
        tag_ids: s.tags,
        genres: s.genres,
        original_language: s
            .original_language
            .and_then(|l| l.name)
            .and_then(|name| super::language::normalise(&name)),
        certification: s.certification,
    }
}
