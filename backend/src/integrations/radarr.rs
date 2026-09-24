//! Radarr API v3 client.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{send_json, send_ok};
use crate::error::AppResult;

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
    #[serde(rename = "hasFile", default)]
    pub has_file: bool,
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

/// Root folder from Radarr API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarrRootFolder {
    pub id: i64,
    pub path: String,
    #[serde(rename = "freeSpace", default, deserialize_with = "super::lenient_bytes")]
    pub free_space: Option<i64>,
    pub accessible: Option<bool>,
}

/// System status from Radarr API.
#[derive(Debug, Clone, Deserialize)]
pub struct RadarrStatus {
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

    /// Test the connection to Radarr.
    pub async fn test_connection(&self) -> AppResult<RadarrStatus> {
        send_json(SERVICE, self.get("/api/v3/system/status")).await
    }

    /// Get all movies from Radarr.
    pub async fn get_movies(&self) -> AppResult<Vec<RadarrMovie>> {
        debug!("Fetching movies from {}", self.base_url);
        send_json(SERVICE, self.get("/api/v3/movie")).await
    }

    /// One movie by id. A 404 surfaces as `ExternalApi { status: 404 }`.
    pub async fn get_movie(&self, id: i64) -> AppResult<Option<RadarrMovie>> {
        send_json(SERVICE, self.get(&format!("/api/v3/movie/{id}"))).await.map(Some)
    }

    /// Get the tag catalogue: a media row only carries numeric ids.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTagDto>> {
        send_json(SERVICE, self.get("/api/v3/tag")).await
    }

    /// Get all root folders from Radarr.
    pub async fn get_root_folders(&self) -> AppResult<Vec<RadarrRootFolder>> {
        send_json(SERVICE, self.get("/api/v3/rootfolder")).await
    }

    /// Whether the Arr can see this directory.
    ///
    /// Asked of the Arr rather than of Routarr's own filesystem: the two run in
    /// different containers as often as not, and `/media/films` existing here
    /// says nothing about whether the process that will do the writing can
    /// reach it. That mismatch is the commonest homelab fault of all, and it is
    /// otherwise discovered at apply time.
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        let query = super::directory_query(path);
        let listing: super::DirectoryListing =
            send_json(SERVICE, self.get("/api/v3/filesystem").query(&[("path", query)])).await?;
        Ok(listing.holds(path))
    }

    /// Bulk update movies: change root folder and optionally move files.
    ///
    /// Radarr's editor endpoint takes a list, so a batch of decisions targeting
    /// the same folder costs one call instead of one call per movie.
    pub async fn update_movies_root_folder(
        &self,
        movie_ids: &[i64],
        root_folder_path: &str,
        move_files: bool,
    ) -> AppResult<()> {
        if movie_ids.is_empty() {
            return Ok(());
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

        send_ok(
            SERVICE,
            self.client
                .put(format!("{}/api/v3/movie/editor", self.base_url))
                .header("X-Api-Key", &self.api_key)
                .json(&MovieEditorRequest { movie_ids, root_folder_path, move_files }),
        )
        .await
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
