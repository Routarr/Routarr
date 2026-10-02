//! Radarr API v3 client.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use super::{send_json, send_ok};
use crate::error::{AppError, AppResult};
use crate::models::ExternalId;

const SERVICE: &str = "Radarr";

#[derive(Debug, Clone)]
pub struct RadarrClient {
    client: Client,
    base_url: String,
    api_key: String,
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
    #[serde(rename = "hasFile")]
    has_file: Option<bool>,
    pub status: Option<String>,
    pub added: Option<String>,
    #[serde(rename = "sizeOnDisk")]
    pub size_on_disk: Option<i64>,
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

impl RadarrMovie {
    /// A payload silent on `hasFile` is read as a movie with a file: auto-apply
    /// moves a movie without its file, and only an explicit `false` makes that
    /// safe.
    pub fn has_files(&self) -> bool {
        self.has_file.unwrap_or(true)
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
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base_url)).header("X-Api-Key", &self.api_key)
    }

    pub async fn test_connection(&self) -> AppResult<ArrStatusDto> {
        send_json(SERVICE, self.get("/api/v3/system/status")).await
    }

    pub async fn get_movies(&self) -> AppResult<Vec<RadarrMovie>> {
        debug!("Fetching movies from {}", crate::http::masked(&self.base_url));
        send_json(SERVICE, self.get("/api/v3/movie")).await
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
        match send_json(SERVICE, request).await {
            Ok(movie) => Ok(Some(movie)),
            Err(AppError::ExternalApi { status: 404, .. }) => Ok(None),
            // Radarr answers an id TMDb does not know with a 500 naming its
            // `MovieNotFoundException`: "Movie with tmdbId 1 was not found, it
            // may have been removed from TMDb."
            Err(AppError::ExternalApi { status: 500, ref message, .. })
                if message.starts_with("Movie with ") && message.contains(" was not found") =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Get the tag catalogue: a media row only carries numeric ids.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTagDto>> {
        send_json(SERVICE, self.get("/api/v3/tag")).await
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolderDto>> {
        send_json(SERVICE, self.get("/api/v3/rootfolder")).await
    }

    /// Whether Radarr can see this directory (`integrations::directory_exists`).
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        super::directory_exists(SERVICE, self.get("/api/v3/filesystem"), path).await
    }

    /// Bulk update movies: change root folder and optionally move files.
    ///
    /// Radarr's editor endpoint takes a list, so a batch of decisions targeting
    /// the same folder costs one call instead of one call per movie.
    ///
    /// Answers the path Radarr gave each movie, by id. With its files moved, a
    /// movie's folder is named from Radarr's naming format, which the caller
    /// cannot compose. An answer that lists no path leaves that movie out: the
    /// edit went through, and only the caller's own composition is left.
    pub async fn update_movies_root_folder(
        &self,
        movie_ids: &[i64],
        root_folder_path: &str,
        move_files: bool,
    ) -> AppResult<HashMap<i64, String>> {
        if movie_ids.is_empty() {
            return Ok(HashMap::new());
        }

        #[derive(Serialize)]
        struct MovieEditorRequest<'a> {
            #[serde(rename = "movieIds")]
            movie_ids: &'a [i64],
            #[serde(rename = "rootFolderPath")]
            root_folder_path: &'a str,
            #[serde(rename = "moveFiles")]
            move_files: bool,
        }

        debug!("Moving {} movie(s) to {}", movie_ids.len(), root_folder_path);

        #[derive(Deserialize)]
        struct Edited {
            id: i64,
            path: Option<String>,
        }

        let response = super::check_status(
            SERVICE,
            self.client
                .put(format!("{}/api/v3/movie/editor", self.base_url))
                .header("X-Api-Key", &self.api_key)
                .json(&MovieEditorRequest { movie_ids, root_folder_path, move_files }),
        )
        .await?;
        let edited = response.json::<Vec<Edited>>().await.unwrap_or_else(|_| {
            debug!("Radarr's movie editor answered without the movies it edited");
            Vec::new()
        });
        Ok(edited.into_iter().filter_map(|movie| Some((movie.id, movie.path?))).collect())
    }

    /// Trigger a rescan/refresh so Radarr picks up the new location.
    pub async fn refresh_movies(&self, movie_ids: &[i64]) -> AppResult<()> {
        if movie_ids.is_empty() {
            return Ok(());
        }

        #[derive(Serialize)]
        struct CommandRequest<'a> {
            name: &'a str,
            #[serde(rename = "movieIds")]
            movie_ids: &'a [i64],
        }

        send_ok(
            SERVICE,
            self.client
                .post(format!("{}/api/v3/command", self.base_url))
                .header("X-Api-Key", &self.api_key)
                .json(&CommandRequest { name: "RefreshMovie", movie_ids }),
        )
        .await
    }
}
