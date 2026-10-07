//! Sonarr API v3 client.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::arr_moves::{Api, MoveCommand};
use super::{send_json, send_json_within};
use crate::error::AppResult;
use crate::models::ExternalId;

const SERVICE: &str = "Sonarr";

#[derive(Debug, Clone)]
pub struct SonarrClient {
    client: Client,
    base_url: String,
    api_key: String,
    /// How long the whole library may take to list.
    library_timeout: std::time::Duration,
    /// The largest library listing read, in bytes.
    library_cap: usize,
}

/// Series data from Sonarr API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonarrSeries {
    /// 0 for a series a lookup found and the library does not hold.
    #[serde(default)]
    pub id: i64,
    pub title: String,
    #[serde(rename = "sortTitle")]
    pub sort_title: Option<String>,
    pub year: Option<i64>,
    #[serde(rename = "tvdbId")]
    pub tvdb_id: Option<i64>,
    #[serde(rename = "imdbId")]
    pub imdb_id: Option<String>,
    #[serde(rename = "tmdbId")]
    pub tmdb_id: Option<i64>,
    pub path: Option<String>,
    #[serde(rename = "rootFolderPath")]
    pub root_folder_path: Option<String>,
    #[serde(default)]
    pub monitored: bool,
    pub status: Option<String>,
    pub added: Option<String>,
    pub statistics: Option<SonarrSeriesStatistics>,
    /// `standard` | `anime` | `daily`. Sonarr's own classification, and the most
    /// reliable anime signal available without a second metadata provider.
    #[serde(rename = "seriesType")]
    pub series_type: Option<String>,
    #[serde(default)]
    pub seasons: Vec<SonarrSeason>,
    /// Tag ids the user attached in Sonarr.
    #[serde(default)]
    pub tags: Vec<i64>,
    /// Same free metadata as Radarr's, under the same names.
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(rename = "originalLanguage")]
    pub original_language: Option<ArrLanguage>,
    pub certification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonarrSeason {
    #[serde(rename = "seasonNumber")]
    pub season_number: i64,
}

impl SonarrSeries {
    /// Sonarr has no `hasFile`, so the episode statistics answer. A payload with
    /// no count is read as a series with files: auto-apply moves a series
    /// without its files, and only a count of zero makes that safe.
    pub fn has_files(&self) -> bool {
        self.statistics.as_ref().and_then(|s| s.episode_file_count).is_none_or(|c| c > 0)
    }

    /// Seasons excluding specials.
    ///
    /// Season 0 holds bonus material, and counting it would make "more than
    /// five seasons" true for a five-season show with extras.
    pub fn season_count(&self) -> i64 {
        self.seasons.iter().filter(|s| s.season_number > 0).count() as i64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonarrSeriesStatistics {
    #[serde(rename = "episodeFileCount")]
    pub episode_file_count: Option<i64>,
    #[serde(rename = "sizeOnDisk")]
    pub size_on_disk: Option<i64>,
}

pub use super::radarr::{ArrDiskSpaceDto, ArrLanguage, ArrRootFolderDto, ArrStatusDto, ArrTagDto};

impl SonarrClient {
    pub fn new(client: Client, base_url: &str, api_key: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            library_timeout: crate::http::LIBRARY_TIMEOUT,
            library_cap: crate::integrations::MAX_BODY,
        }
    }

    /// The same client, listing the library within `timeout` and `cap` bytes.
    pub fn with_library_limits(self, timeout: std::time::Duration, cap: usize) -> Self {
        Self { library_timeout: timeout, library_cap: cap, ..self }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base_url)).header("X-Api-Key", &self.api_key)
    }

    pub async fn test_connection(&self) -> AppResult<ArrStatusDto> {
        send_json(SERVICE, self.get("/api/v3/system/status")).await
    }

    pub async fn get_series(&self) -> AppResult<Vec<SonarrSeries>> {
        debug!("Fetching series from {}", crate::http::masked(&self.base_url));
        let request = self.get("/api/v3/series").timeout(self.library_timeout);
        send_json_within(SERVICE, request, self.library_cap).await
    }

    /// One series by id. A 404 surfaces as `ExternalApi { status: 404 }`.
    pub async fn get_series_one(&self, id: i64) -> AppResult<SonarrSeries> {
        send_json(SERVICE, self.get(&format!("/api/v3/series/{id}"))).await
    }

    /// A series as TheTVDB or IMDb names it, whether the library holds it or
    /// not, `None` when Sonarr knows no such series.
    pub async fn lookup_series(&self, id: &ExternalId) -> AppResult<Option<SonarrSeries>> {
        let term = match id {
            ExternalId::Tvdb(tvdb) => format!("tvdb:{tvdb}"),
            ExternalId::Imdb(imdb) => format!("imdb:{imdb}"),
            ExternalId::Tmdb(tmdb) => format!("tmdb:{tmdb}"),
        };
        let request = self.get("/api/v3/series/lookup").query(&[("term", term)]);
        let found: Vec<SonarrSeries> = send_json(SERVICE, request).await?;
        // A term search, so every answer is held to the id it was asked for.
        let Some(found) = found.into_iter().find(|series| match id {
            ExternalId::Tvdb(tvdb) => series.tvdb_id == Some(*tvdb),
            ExternalId::Imdb(imdb) => series.imdb_id.as_deref() == Some(imdb.as_str()),
            ExternalId::Tmdb(tmdb) => series.tmdb_id == Some(*tmdb),
        }) else {
            return Ok(None);
        };
        // The lookup names a series the library holds by its id, with counts
        // of zero and no root folder: the library is asked for the rest.
        if found.id == 0 {
            return Ok(Some(found));
        }
        self.get_series_one(found.id).await.map(Some)
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolderDto>> {
        send_json(SERVICE, self.get("/api/v3/rootfolder")).await
    }

    pub async fn get_disk_space(&self) -> AppResult<Vec<ArrDiskSpaceDto>> {
        send_json(SERVICE, self.get("/api/v3/diskspace")).await
    }

    /// Whether Sonarr can see this directory (`integrations::directory_exists`).
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        super::directory_exists(SERVICE, self.get("/api/v3/filesystem"), path).await
    }

    /// Get the tag catalogue: a media row only carries numeric ids.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTagDto>> {
        send_json(SERVICE, self.get("/api/v3/tag")).await
    }

    /// Point one series at `root`, keeping its folder name (`Api::repoint`),
    /// and answer the path it now has.
    pub async fn update_series_path(
        &self,
        series_id: i64,
        root: &str,
        move_files: bool,
    ) -> AppResult<String> {
        self.api().repoint("series", series_id, root, move_files).await
    }

    /// The move commands Sonarr lists (`Api::move_commands`).
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
