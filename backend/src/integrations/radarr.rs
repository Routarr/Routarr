//! Radarr API v3 client.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::arr_moves::{Api, MoveCommand};
use super::send_json;
use crate::error::{AppError, AppResult};
use crate::models::ExternalId;

const SERVICE: &str = "Radarr";

#[derive(Debug, Clone)]
pub struct RadarrClient {
    client: Client,
    base_url: String,
    api_key: String,
    /// How long the whole library may take to list.
    library_timeout: std::time::Duration,
}

/// Movie data from Radarr API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarrMovie {
    /// 0 for a movie a lookup found and the library does not hold.
    #[serde(default)]
    pub id: i64,
    pub title: String,
    #[serde(rename = "sortTitle")]
    pub sort_title: Option<String>,
    pub year: Option<i64>,
    #[serde(rename = "tmdbId")]
    pub tmdb_id: Option<i64>,
    #[serde(rename = "imdbId")]
    pub imdb_id: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "rootFolderPath")]
    pub root_folder_path: Option<String>,
    #[serde(default)]
    pub monitored: bool,
    /// Read after `statistics`, which Radarr fills it from and keeps it for.
    #[serde(rename = "hasFile")]
    has_file: Option<bool>,
    pub status: Option<String>,
    pub added: Option<String>,
    /// Read after `statistics`, as `hasFile` is.
    #[serde(rename = "sizeOnDisk")]
    size_on_disk: Option<i64>,
    statistics: Option<MovieStatistics>,
    /// Tag ids the user attached in Radarr.
    #[serde(default)]
    pub tags: Vec<i64>,
    /// Metadata Radarr already holds, so there is no reason to ask TMDb for it
    /// a second time. Free: it is in this very payload.
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(rename = "originalLanguage")]
    pub original_language: Option<ArrLanguage>,
    /// The rating for whichever certification country Radarr is set to.
    pub certification: Option<String>,
}

/// What Radarr counts of a movie on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MovieStatistics {
    #[serde(rename = "movieFileCount")]
    movie_file_count: Option<i64>,
    #[serde(rename = "sizeOnDisk")]
    size_on_disk: Option<i64>,
}

impl RadarrMovie {
    /// A payload silent on its files is read as a movie with a file:
    /// auto-apply moves a movie without its file, and only an explicit count
    /// of none makes that safe.
    pub fn has_files(&self) -> bool {
        let counted = self.statistics.as_ref().and_then(|s| s.movie_file_count).map(|n| n > 0);
        counted.or(self.has_file).unwrap_or(true)
    }

    pub fn size_on_disk(&self) -> Option<i64> {
        self.statistics.as_ref().and_then(|s| s.size_on_disk).or(self.size_on_disk)
    }
}

/// A language as an Arr reports it: an id and a name, never a code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrLanguage {
    pub name: Option<String>,
}

/// A tag as defined in the Arr's settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrTagDto {
    pub id: i64,
    pub label: String,
}

/// A root folder as either Arr reports it on `/api/v3/rootfolder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrRootFolderDto {
    pub id: i64,
    pub path: String,
    #[serde(rename = "freeSpace", default, deserialize_with = "super::lenient_bytes")]
    pub free_space: Option<i64>,
    pub accessible: Option<bool>,
}

/// What either Arr answers on `/api/v3/system/status`.
#[derive(Debug, Clone, Deserialize)]
pub struct ArrStatusDto {
    pub version: String,
    #[serde(rename = "appName")]
    pub app_name: Option<String>,
}

impl RadarrClient {
    /// Build a client sharing the application-wide connection pool and timeouts.
    pub fn new(client: Client, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            library_timeout: crate::http::LIBRARY_TIMEOUT,
        }
    }

    /// The same client, listing the library within `timeout`.
    pub fn with_library_timeout(self, timeout: std::time::Duration) -> Self {
        Self { library_timeout: timeout, ..self }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base_url)).header("X-Api-Key", &self.api_key)
    }

    pub async fn test_connection(&self) -> AppResult<ArrStatusDto> {
        send_json(SERVICE, self.get("/api/v3/system/status")).await
    }

    pub async fn get_movies(&self) -> AppResult<Vec<RadarrMovie>> {
        debug!("Fetching movies from {}", crate::http::masked(&self.base_url));
        // Without the parameter Radarr looks up and hashes every cover of the
        // library for an answer Routarr reads no image of.
        let request = self.get("/api/v3/movie").query(&[("excludeLocalCovers", "true")]);
        send_json(SERVICE, request.timeout(self.library_timeout)).await
    }

    /// One movie by id. A 404 surfaces as `ExternalApi { status: 404 }`.
    pub async fn get_movie(&self, id: i64) -> AppResult<RadarrMovie> {
        send_json(SERVICE, self.get(&format!("/api/v3/movie/{id}"))).await
    }

    /// A movie as TMDb or IMDb names it, whether the library holds it or not,
    /// `None` when Radarr knows no such movie.
    pub async fn lookup_movie(&self, id: &ExternalId) -> AppResult<Option<RadarrMovie>> {
        let request = match id {
            ExternalId::Tmdb(tmdb) => {
                self.get("/api/v3/movie/lookup/tmdb").query(&[("tmdbId", tmdb.to_string())])
            }
            ExternalId::Imdb(imdb) => {
                self.get("/api/v3/movie/lookup/imdb").query(&[("imdbId", imdb.as_str())])
            }
            ExternalId::Tvdb(_) => {
                return Err(AppError::BadRequest(
                    "Radarr looks a movie up by its TMDb or IMDb id.".into(),
                ));
            }
        };
        let found: RadarrMovie = match send_json(SERVICE, request).await {
            Ok(movie) => movie,
            Err(AppError::ExternalApi { status: 404, .. }) => return Ok(None),
            // Radarr answers an id TMDb does not know with a 500 naming its
            // `MovieNotFoundException`: "Movie with tmdbId 1 was not found, it
            // may have been removed from TMDb."
            Err(AppError::ExternalApi { status: 500, ref message, .. })
                if message.starts_with("Movie with ") && message.contains(" was not found") =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        // The lookup builds the film afresh from TMDb, with no id, folder or
        // tags even when the library holds it: the library is asked for that,
        // and its answer held to the id asked, since a proxy or a fork that
        // ignores the filter answers the whole library.
        let Some(tmdb) = found.tmdb_id else {
            return Ok(Some(found));
        };
        let held: Vec<RadarrMovie> =
            send_json(SERVICE, self.get("/api/v3/movie").query(&[("tmdbId", tmdb.to_string())]))
                .await?;
        Ok(Some(held.into_iter().find(|movie| movie.tmdb_id == Some(tmdb)).unwrap_or(found)))
    }

    /// Get the tag catalogue: a media row only carries numeric ids.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTagDto>> {
        send_json(SERVICE, self.get("/api/v3/tag")).await
    }

    /// The country Radarr's metadata settings rate films for, upper-case, as
    /// its ratings are then that country's.
    pub async fn certification_country(&self) -> AppResult<Option<String>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MetadataConfig {
            certification_country: Option<String>,
        }
        let config: MetadataConfig =
            send_json(SERVICE, self.get("/api/v3/config/metadata")).await?;
        Ok(config
            .certification_country
            .map(|country| country.trim().to_uppercase())
            .filter(|country| !country.is_empty()))
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolderDto>> {
        send_json(SERVICE, self.get("/api/v3/rootfolder")).await
    }

    /// Whether Radarr can see this directory (`integrations::directory_exists`).
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        super::directory_exists(SERVICE, self.get("/api/v3/filesystem"), path).await
    }

    /// Point one film at `root`, keeping its folder name (`Api::repoint`),
    /// and answer the path it now has.
    pub async fn update_movie_path(
        &self,
        movie_id: i64,
        root: &str,
        move_files: bool,
    ) -> AppResult<String> {
        self.api().repoint("movie", movie_id, root, move_files).await
    }

    /// The move commands Radarr lists (`Api::move_commands`).
    pub async fn move_commands(&self) -> AppResult<Vec<MoveCommand>> {
        self.api().move_commands().await
    }

    fn api(&self) -> Api<'_> {
        Api {
            service: SERVICE,
            client: &self.client,
            base_url: &self.base_url,
            api_key: &self.api_key,
        }
    }
}
