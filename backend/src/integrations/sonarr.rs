//! Sonarr API v3 client.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{send_json, send_ok};
use crate::error::{AppError, AppResult};

const SERVICE: &str = "Sonarr";

#[derive(Debug, Clone)]
pub struct SonarrClient {
    client: Client,
    base_url: String,
    api_key: String,
}

/// Series data from Sonarr API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonarrSeries {
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

pub use super::radarr::{ArrLanguage, ArrRootFolderDto, ArrStatusDto, ArrTagDto};

impl SonarrClient {
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

    pub async fn get_series(&self) -> AppResult<Vec<SonarrSeries>> {
        debug!("Fetching series from {}", crate::http::masked(&self.base_url));
        send_json(SERVICE, self.get("/api/v3/series")).await
    }

    /// One series by id. A 404 surfaces as `ExternalApi { status: 404 }`.
    pub async fn get_series_one(&self, id: i64) -> AppResult<SonarrSeries> {
        send_json(SERVICE, self.get(&format!("/api/v3/series/{id}"))).await
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolderDto>> {
        send_json(SERVICE, self.get("/api/v3/rootfolder")).await
    }

    /// Whether Sonarr can see this directory (`integrations::directory_exists`).
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        super::directory_exists(SERVICE, self.get("/api/v3/filesystem"), path).await
    }

    /// Get the tag catalogue: a media row only carries numeric ids.
    pub async fn get_tags(&self) -> AppResult<Vec<ArrTagDto>> {
        send_json(SERVICE, self.get("/api/v3/tag")).await
    }

    /// Move one series to a new root folder.
    ///
    /// One request per series, although Sonarr's `PUT /api/v3/series/editor`
    /// takes a list: the executor records each series from its own answer, and
    /// records its new path as the target plus the folder name it already had
    /// (`relocate`). The editor answers once for the whole list, and names each
    /// destination folder from Sonarr's naming format instead of keeping the
    /// one on disk.
    ///
    /// The full series object is read back, patched and re-sent, since anything
    /// less drops fields the PUT expects. The existing folder name is kept:
    /// deriving it from `titleSlug` would silently *rename* the on-disk
    /// directory during what was asked to be a move.
    pub async fn update_series_path(
        &self,
        series_id: i64,
        new_root_folder_path: &str,
        move_files: bool,
    ) -> AppResult<()> {
        let mut series: serde_json::Value =
            send_json(SERVICE, self.get(&format!("/api/v3/series/{series_id}"))).await?;

        // Assigning a key of a `Value` that is not an object panics, and the two
        // assignments below do that. A proxy answering 200 with a cached `[]`,
        // or a base URL pointing at another service on the same host, is enough
        // to get one, and the apply would abort on a 500 naming nothing instead
        // of a 502 naming Sonarr.
        if !series.is_object() {
            return Err(AppError::ExternalApi {
                service: SERVICE.to_string(),
                status: 0,
                message: format!(
                    "series {series_id} came back as {}, not an object",
                    kind_of(&series)
                ),
                retry_after: None,
            });
        }

        let folder_name = current_folder_name(&series).unwrap_or_else(|| slugify_fallback(&series));
        let new_path = join_path(new_root_folder_path, &folder_name);

        series["rootFolderPath"] = serde_json::Value::String(new_root_folder_path.to_string());
        series["path"] = serde_json::Value::String(new_path);

        send_ok(
            SERVICE,
            self.client
                .put(format!("{}/api/v3/series/{series_id}?moveFiles={move_files}", self.base_url))
                .header("X-Api-Key", &self.api_key)
                .json(&series),
        )
        .await
    }

    /// Trigger a rescan/refresh so Sonarr picks up the new location.
    pub async fn refresh_series(&self, series_id: i64) -> AppResult<()> {
        #[derive(Serialize)]
        struct CommandRequest<'a> {
            name: &'a str,
            #[serde(rename = "seriesId")]
            series_id: i64,
        }

        send_ok(
            SERVICE,
            self.client
                .post(format!("{}/api/v3/command", self.base_url))
                .header("X-Api-Key", &self.api_key)
                .json(&CommandRequest { name: "RefreshSeries", series_id }),
        )
        .await
    }
}

/// A payload's shape, for a message that says what came back instead.
fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// The last path segment of the series' current folder, which is the name the
/// user (or Sonarr's naming config) already chose.
fn current_folder_name(series: &serde_json::Value) -> Option<String> {
    let path = series.get("path")?.as_str()?.trim_end_matches(['/', '\\']);
    let name = path.rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Only used when the series has no path yet (never scanned).
fn slugify_fallback(series: &serde_json::Value) -> String {
    series
        .get("titleSlug")
        .and_then(|v| v.as_str())
        .or_else(|| series.get("title").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string()
}

fn join_path(root: &str, name: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{name}", root.trim_end_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_the_existing_folder_name() {
        let series = json!({
            "path": "/tv/standard/Cowboy Bebop (1998)",
            "titleSlug": "cowboy-bebop",
        });
        assert_eq!(current_folder_name(&series).as_deref(), Some("Cowboy Bebop (1998)"));
    }

    #[test]
    fn tolerates_a_trailing_slash() {
        let series = json!({ "path": "/tv/standard/Dark/" });
        assert_eq!(current_folder_name(&series).as_deref(), Some("Dark"));
    }

    #[test]
    fn falls_back_to_the_slug_when_never_scanned() {
        let series = json!({ "titleSlug": "cowboy-bebop" });
        assert_eq!(current_folder_name(&series), None);
        assert_eq!(slugify_fallback(&series), "cowboy-bebop");
    }

    #[test]
    fn builds_unix_paths() {
        assert_eq!(join_path("/tv/anime/", "Dark"), "/tv/anime/Dark");
        assert_eq!(join_path("/tv/anime", "Dark"), "/tv/anime/Dark");
    }

    #[test]
    fn builds_windows_paths() {
        assert_eq!(join_path("D:\\tv\\anime", "Dark"), "D:\\tv\\anime\\Dark");
    }
}
