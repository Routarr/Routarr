//! A single façade over Radarr and Sonarr.
//!
//! One common model with an adapter per media type. Sync, health and the
//! executor all talk to this enum, so none of them branches on
//! `instance_type` and repeats the same code for each Arr.

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
    /// Tag ids as the Arr reports them, resolved to labels at sync time.
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
    /// Build the adapter for a stored instance, given its API key already
    /// decrypted (`AppState::adapter` opens it).
    pub fn for_instance(client: Client, instance: &Instance, api_key: &str) -> AppResult<Self> {
        Self::new(client, &instance.instance_type, &instance.base_url, api_key)
    }

    /// Build the adapter for values not saved yet, as the instance form tries them.
    pub fn new(client: Client, kind: &str, base_url: &str, api_key: &str) -> AppResult<Self> {
        match kind {
            "radarr" => Ok(Self::Radarr(RadarrClient::new(client, base_url, api_key))),
            "sonarr" => Ok(Self::Sonarr(SonarrClient::new(client, base_url, api_key))),
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

    /// One item by its Arr id, `None` when the Arr answers 404 for it.
    ///
    /// The webhook path reads one item per event, and an Arr sends one event
    /// per imported file: listing the whole library there would cost a full
    /// download per episode of a season.
    pub async fn get_media_one(&self, arr_id: i64) -> AppResult<Option<ArrMedia>> {
        let found = match self {
            Self::Radarr(c) => c.get_movie(arr_id).await.map(movie_to_media),
            Self::Sonarr(c) => c.get_series_one(arr_id).await.map(series_to_media),
        };
        match found {
            Err(AppError::ExternalApi { status: 404, .. }) => Ok(None),
            other => other.map(Some),
        }
    }

    /// Get the instance's tag catalogue, so ids can be resolved to labels.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTag>> {
        let tags = match self {
            Self::Radarr(c) => c.get_tags().await?,
            Self::Sonarr(c) => c.get_tags().await?,
        };
        Ok(tags.into_iter().map(|t| ArrTag { arr_id: t.id, label: t.label }).collect())
    }

    /// Move a batch of items to a root folder.
    ///
    /// Radarr accepts the whole batch in one call. Sonarr has no bulk editor, so
    /// each series is patched individually and a failure is reported for its
    /// item rather than aborting the rest of the batch.
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
            // One command per series: each is tried, whatever the one before
            // it answered, and the first refusal is what is reported.
            Self::Sonarr(c) => {
                let mut first_failure = None;
                for id in arr_ids {
                    if let Err(e) = c.refresh_series(*id).await {
                        first_failure.get_or_insert(e);
                    }
                }
                first_failure.map_or(Ok(()), Err)
            }
        }
    }
}

/// `AppError` is not `Clone`, since it wraps `sqlx` and `serde_json` errors.
/// This rebuilds the variants the Arr clients can actually produce, so a bulk
/// failure can be reported per item.
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
    let has_files = m.has_files();
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
        has_files,
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

#[cfg(test)]
mod tests {
    use super::{movie_to_media, series_to_media};
    use serde_json::json;

    fn movie_has_files(body: serde_json::Value) -> bool {
        movie_to_media(serde_json::from_value(body).expect("a movie")).has_files
    }

    fn series_has_files(body: serde_json::Value) -> bool {
        series_to_media(serde_json::from_value(body).expect("a series")).has_files
    }

    /// Auto-apply moves an item without its files, so it may only do so when
    /// the Arr said there are none. A payload silent on the question is read as
    /// an item with files.
    #[test]
    fn an_item_is_without_files_only_when_the_arr_says_so() {
        assert!(!movie_has_files(json!({ "id": 10, "title": "Totoro", "hasFile": false })));
        assert!(movie_has_files(json!({ "id": 10, "title": "Totoro", "hasFile": true })));
        assert!(movie_has_files(json!({ "id": 10, "title": "Totoro" })));

        let with_statistics = |statistics: serde_json::Value| {
            series_has_files(json!({ "id": 20, "title": "Cowboy Bebop", "statistics": statistics }))
        };
        assert!(!with_statistics(json!({ "episodeFileCount": 0 })));
        assert!(with_statistics(json!({ "episodeFileCount": 26 })));
        assert!(with_statistics(json!({})));
        assert!(series_has_files(json!({ "id": 20, "title": "Cowboy Bebop" })));
    }
}
