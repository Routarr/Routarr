//! A single façade over Radarr and Sonarr.
//!
//! One common model with an adapter per media type. Sync, health and the
//! executor all talk to this enum, so none of them branches on
//! `instance_type` and repeats the same code for each Arr.

use reqwest::Client;

use crate::error::{AppError, AppResult};
use crate::integrations::arr_moves::MoveCommand;
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
    /// what lets Routarr classify a library with no TMDB key at all.
    pub genres: Vec<String>,
    /// Normalised to the ISO 639-1 code TMDB would have returned, so both
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

/// A mount and its free space, as either Arr reports it.
#[derive(Debug, Clone)]
pub struct ArrDiskSpace {
    pub path: String,
    pub free_space: Option<i64>,
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

    /// The same adapter, listing the library within `timeout` and `cap` bytes.
    pub fn with_library_limits(self, timeout: std::time::Duration, cap: usize) -> Self {
        match self {
            Self::Radarr(c) => Self::Radarr(c.with_library_limits(timeout, cap)),
            Self::Sonarr(c) => Self::Sonarr(c.with_library_limits(timeout, cap)),
        }
    }

    pub async fn test_connection(&self) -> AppResult<ArrStatus> {
        let status = match self {
            Self::Radarr(c) => c.test_connection().await?,
            Self::Sonarr(c) => c.test_connection().await?,
        };
        Ok(ArrStatus { version: status.version, app_name: status.app_name })
    }

    /// Whether the Arr can see this directory, in *its* filesystem namespace.
    pub async fn directory_exists(&self, path: &str) -> AppResult<bool> {
        match self {
            Self::Radarr(c) => c.directory_exists(path).await,
            Self::Sonarr(c) => c.directory_exists(path).await,
        }
    }

    pub async fn get_root_folders(&self) -> AppResult<Vec<ArrRootFolder>> {
        let folders = match self {
            Self::Radarr(c) => c.get_root_folders().await?,
            Self::Sonarr(c) => c.get_root_folders().await?,
        };
        Ok(folders
            .into_iter()
            .map(|rf| ArrRootFolder {
                arr_id: rf.id,
                path: rf.path,
                free_space: rf.free_space,
                accessible: rf.accessible.unwrap_or(true),
            })
            .collect())
    }

    /// The free space of each mount the Arr sees, read without listing the
    /// folders inside the root folders as `get_root_folders` makes the Arr do.
    pub async fn get_disk_space(&self) -> AppResult<Vec<ArrDiskSpace>> {
        let mounts = match self {
            Self::Radarr(c) => c.get_disk_space().await?,
            Self::Sonarr(c) => c.get_disk_space().await?,
        };
        Ok(mounts
            .into_iter()
            .map(|mount| ArrDiskSpace { path: mount.path, free_space: mount.free_space })
            .collect())
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

    /// A title as another service names it, whether this Arr's library holds
    /// it or not. `None` when the Arr knows no such title.
    pub async fn lookup(&self, id: &crate::models::ExternalId) -> AppResult<Option<ArrMedia>> {
        match self {
            Self::Radarr(c) => Ok(c.lookup_movie(id).await?.map(movie_to_media)),
            Self::Sonarr(c) => Ok(c.lookup_series(id).await?.map(series_to_media)),
        }
    }

    /// The country the Arr's ratings belong to: Radarr's metadata settings
    /// name it, and Sonarr rates for the United States.
    pub async fn certification_country(&self) -> AppResult<Option<String>> {
        match self {
            Self::Radarr(c) => c.certification_country().await,
            Self::Sonarr(_) => Ok(Some("US".to_string())),
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

    /// Point one title at another root folder, keeping its folder name, and
    /// answer the path it now has. With `move_files` the Arr answers once it
    /// has queued the move, and carries the files in a command of its own
    /// ([`ArrAdapter::move_commands`]).
    pub async fn move_item(
        &self,
        arr_id: i64,
        root_folder_path: &str,
        move_files: bool,
    ) -> AppResult<String> {
        match self {
            Self::Radarr(c) => c.update_movie_path(arr_id, root_folder_path, move_files).await,
            Self::Sonarr(c) => c.update_series_path(arr_id, root_folder_path, move_files).await,
        }
    }

    /// The move commands the Arr lists, one entry per title they carry.
    pub async fn move_commands(&self) -> AppResult<Vec<MoveCommand>> {
        match self {
            Self::Radarr(c) => c.move_commands().await,
            Self::Sonarr(c) => c.move_commands().await,
        }
    }
}

/// The oldest release of each Arr Routarr supports in full, and what is lost
/// below it: Radarr sends `MovieAdded` from 4.2, and Sonarr sends `SeriesAdd`
/// and describes a series' language from 4.
pub fn below_minimum(kind: &str, version: &str) -> Option<&'static str> {
    let minimum: &[u64] = match kind {
        "radarr" => &[4, 2],
        "sonarr" => &[4],
        _ => return None,
    };
    let parts: Vec<u64> = version.split('.').map_while(|part| part.trim().parse().ok()).collect();
    if parts.is_empty() {
        return None;
    }
    (parts.as_slice() < minimum).then_some(if kind == "radarr" { "4.2" } else { "4" })
}

#[cfg(test)]
mod versions {
    #[test]
    fn a_release_below_the_minimum_is_named_and_one_at_or_above_it_is_not() {
        use super::below_minimum;
        assert_eq!(below_minimum("radarr", "4.1.0.6175"), Some("4.2"));
        assert_eq!(below_minimum("radarr", "4.2.0.6370"), None);
        assert_eq!(below_minimum("radarr", "6.0.4.10291"), None);
        assert_eq!(below_minimum("sonarr", "3.0.10.1567"), Some("4"));
        assert_eq!(below_minimum("sonarr", "4.0.0.738"), None);
        assert_eq!(below_minimum("sonarr", "not a version"), None);
    }
}

/// Radarr and Sonarr write a year they do not know as 0, which as a year
/// would satisfy every `year_range` with a maximum.
fn known_year(year: Option<i64>) -> Option<i64> {
    year.filter(|year| *year > 0)
}

/// An id the Arr writes as 0 is one it does not know: Sonarr sends a series
/// with no TMDB id as `"tmdbId": 0`. Taken for an id, every such title would
/// share one search key, and with it the one answer found for the first.
fn known_id(id: Option<i64>) -> Option<i64> {
    id.filter(|id| *id > 0)
}

fn movie_to_media(m: crate::integrations::radarr::RadarrMovie) -> ArrMedia {
    let (has_files, size_on_disk) = (m.has_files(), m.size_on_disk());
    ArrMedia {
        arr_id: m.id,
        media_type: MOVIE,
        title: m.title,
        sort_title: m.sort_title,
        year: known_year(m.year),
        tmdb_id: known_id(m.tmdb_id),
        tvdb_id: None,
        imdb_id: m.imdb_id,
        path: m.path,
        root_folder_path: m.root_folder_path,
        monitored: m.monitored,
        has_files,
        status: m.status,
        added: m.added,
        series_type: None,
        size_on_disk,
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
        year: known_year(s.year),
        tmdb_id: known_id(s.tmdb_id),
        tvdb_id: known_id(s.tvdb_id),
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
        // Radarr counts the files in `statistics`, and fills `hasFile` from it.
        let counted = |count: i64| json!({ "id": 10, "title": "Totoro", "statistics": { "movieFileCount": count } });
        assert!(!movie_has_files(counted(0)));
        assert!(movie_has_files(counted(1)));

        let with_statistics = |statistics: serde_json::Value| {
            series_has_files(json!({ "id": 20, "title": "Cowboy Bebop", "statistics": statistics }))
        };
        assert!(!with_statistics(json!({ "episodeFileCount": 0 })));
        assert!(with_statistics(json!({ "episodeFileCount": 26 })));
        assert!(with_statistics(json!({})));
        assert!(series_has_files(json!({ "id": 20, "title": "Cowboy Bebop" })));
    }

    /// Sonarr writes a series with no TMDB id as `"tmdbId": 0`, its field being
    /// a number it cannot leave out. Taken for an id, every such series shares
    /// the key `tmdb:0` and the one answer a search gave the first of them.
    #[test]
    fn an_id_the_arr_writes_as_zero_is_no_id() {
        let series = series_to_media(
            serde_json::from_value(json!({ "id": 20, "title": "A", "tvdbId": 76885, "tmdbId": 0 }))
                .unwrap(),
        );
        assert_eq!((series.tmdb_id, series.tvdb_id), (None, Some(76885)));
        let unknown = series_to_media(
            serde_json::from_value(json!({ "id": 21, "title": "B", "tvdbId": 0 })).unwrap(),
        );
        assert_eq!(unknown.tvdb_id, None);
        let film = movie_to_media(
            serde_json::from_value(json!({ "id": 10, "title": "C", "tmdbId": 0 })).unwrap(),
        );
        assert_eq!(film.tmdb_id, None);
    }

    /// A Radarr v5 `/api/v3/movie` item in full, through the real types: every
    /// field the client ignores, the language as an id and a name, the files
    /// counted in `statistics` beside the older `hasFile`. A change in Radarr's
    /// shape surfaces here, where a fake written to fit the mapping hides it.
    #[test]
    fn a_real_shape_radarr_movie_maps() {
        let movie = movie_to_media(
            serde_json::from_str(
                r#"{
          "title": "My Neighbor Totoro", "originalTitle": "となりのトトロ",
          "originalLanguage": { "id": 8, "name": "Japanese" },
          "alternateTitles": [{ "sourceType": "tmdb", "movieMetadataId": 12,
            "title": "Tonari no Totoro", "cleanTitle": "tonarinototoro", "id": 31 }],
          "secondaryYearSourceId": 0, "sortTitle": "my neighbor totoro",
          "sizeOnDisk": 4389203851, "status": "released",
          "overview": "Two sisters move to the country.",
          "inCinemas": "1988-04-16T00:00:00Z", "physicalRelease": "2006-03-07T00:00:00Z",
          "digitalRelease": "2020-02-01T00:00:00Z", "releaseDate": "2006-03-07T00:00:00Z",
          "images": [{ "coverType": "poster", "url": "/MediaCover/12/poster.jpg?lastWrite=638",
            "remoteUrl": "https://image.tmdb.org/t/p/original/rtGDOeG9LzoerkDGZF9dnVeLppL.jpg" }],
          "website": "", "year": 1988, "youTubeTrailerId": "92a7Hj0ijLs",
          "studio": "Studio Ghibli", "path": "/movies/anime/My Neighbor Totoro (1988)",
          "qualityProfileId": 4, "hasFile": true, "movieFileId": 7, "monitored": true,
          "minimumAvailability": "released", "isAvailable": true,
          "folderName": "/movies/anime/My Neighbor Totoro (1988)", "runtime": 86,
          "cleanTitle": "myneighbortotoro", "imdbId": "tt0096283", "tmdbId": 8392,
          "titleSlug": "8392-my-neighbor-totoro", "rootFolderPath": "/movies/anime/",
          "certification": "G", "genres": ["Animation", "Family", "Fantasy"],
          "keywords": ["forest", "spirit"], "tags": [2, 5],
          "added": "2024-01-04T18:22:41Z",
          "ratings": {
            "imdb": { "votes": 393000, "value": 8.1, "type": "user" },
            "tmdb": { "votes": 8100, "value": 8.06, "type": "user" },
            "rottenTomatoes": { "votes": 0, "value": 94, "type": "user" }
          },
          "collection": { "title": "Studio Ghibli Collection", "tmdbId": 1000 },
          "popularity": 46.2, "lastSearchTime": "2024-01-04T18:23:00Z",
          "statistics": { "movieFileCount": 1, "sizeOnDisk": 4389203851,
            "releaseGroups": ["GHIBLI"] },
          "id": 12
        }"#,
            )
            .expect("a Radarr movie"),
        );

        assert_eq!(
            (movie.arr_id, movie.title.as_str(), movie.year, movie.tmdb_id),
            (12, "My Neighbor Totoro", Some(1988), Some(8392))
        );
        assert_eq!(movie.imdb_id.as_deref(), Some("tt0096283"));
        assert_eq!(movie.root_folder_path.as_deref(), Some("/movies/anime/"));
        assert_eq!(movie.path.as_deref(), Some("/movies/anime/My Neighbor Totoro (1988)"));
        assert!(movie.monitored && movie.has_files);
        assert_eq!(movie.size_on_disk, Some(4389203851));
        assert_eq!(movie.status.as_deref(), Some("released"));
        assert_eq!(movie.tag_ids, [2, 5]);
        assert_eq!(movie.genres, ["Animation", "Family", "Fantasy"]);
        assert_eq!(movie.original_language.as_deref(), Some("ja"));
        assert_eq!(movie.certification.as_deref(), Some("G"));
    }

    /// A Sonarr v4 `/api/v3/series` item in full, for a series TheTVDB maps to
    /// no TMDB id, which Sonarr writes as 0: the seasons with their own
    /// statistics, the series' statistics, its ratings and its alternate
    /// titles, none of which the client reads beyond its counts.
    #[test]
    fn a_real_shape_sonarr_series_maps() {
        let series = series_to_media(
            serde_json::from_str(
                r#"{
          "title": "Mushi-Shi", "alternateTitles": [{ "title": "Mushishi", "sceneSeasonNumber": -1 }],
          "sortTitle": "mushishi", "status": "ended", "ended": true,
          "overview": "Ginko travels to study the mushi.", "previousAiring": "2014-12-20T15:00:00Z",
          "network": "Fuji TV", "airTime": "00:00",
          "images": [{ "coverType": "poster", "url": "/MediaCover/20/poster.jpg?lastWrite=638",
            "remoteUrl": "https://artworks.thetvdb.com/banners/posters/79570-1.jpg" }],
          "originalLanguage": { "id": 8, "name": "Japanese" },
          "seasons": [
            { "seasonNumber": 0, "monitored": false, "statistics": { "episodeFileCount": 0,
              "episodeCount": 0, "totalEpisodeCount": 2, "sizeOnDisk": 0, "releaseGroups": [],
              "percentOfEpisodes": 0.0 } },
            { "seasonNumber": 1, "monitored": true, "statistics": {
              "previousAiring": "2006-06-19T15:00:00Z", "episodeFileCount": 26, "episodeCount": 26,
              "totalEpisodeCount": 26, "sizeOnDisk": 30734221312, "releaseGroups": ["Coalgirls"],
              "percentOfEpisodes": 100.0 } },
            { "seasonNumber": 2, "monitored": true, "statistics": {
              "previousAiring": "2014-12-20T15:00:00Z", "episodeFileCount": 20, "episodeCount": 20,
              "totalEpisodeCount": 20, "sizeOnDisk": 24500000000, "releaseGroups": ["Coalgirls"],
              "percentOfEpisodes": 100.0 } }
          ],
          "year": 2005, "path": "/tv/anime/Mushi-Shi", "qualityProfileId": 6,
          "seasonFolder": true, "monitored": true, "monitorNewItems": "all",
          "useSceneNumbering": false, "runtime": 25, "tvdbId": 79570, "tvRageId": 0,
          "tvMazeId": 3184, "tmdbId": 0, "firstAired": "2005-10-22T00:00:00Z",
          "lastAired": "2014-12-20T00:00:00Z", "seriesType": "anime",
          "cleanTitle": "mushishi", "imdbId": "tt0807832", "titleSlug": "mushi-shi",
          "rootFolderPath": "/tv/anime/", "certification": "TV-14",
          "genres": ["Animation", "Anime", "Drama"], "tags": [3],
          "added": "2023-05-02T09:14:07Z", "ratings": { "votes": 1200, "value": 8.7 },
          "statistics": { "seasonCount": 2, "episodeFileCount": 46, "episodeCount": 46,
            "totalEpisodeCount": 48, "sizeOnDisk": 55234221312, "releaseGroups": ["Coalgirls"],
            "percentOfEpisodes": 100.0 },
          "languageProfileId": 1, "id": 20
        }"#,
            )
            .expect("a Sonarr series"),
        );

        assert_eq!((series.arr_id, series.year), (20, Some(2005)));
        assert_eq!((series.tmdb_id, series.tvdb_id), (None, Some(79570)));
        assert_eq!(series.root_folder_path.as_deref(), Some("/tv/anime/"));
        assert_eq!(series.season_count, Some(2), "the specials counted as a season");
        assert!(series.has_files);
        assert_eq!(series.size_on_disk, Some(55234221312));
        assert_eq!(series.series_type.as_deref(), Some("anime"));
        assert_eq!(series.status.as_deref(), Some("ended"));
        assert_eq!(series.original_language.as_deref(), Some("ja"));
        assert_eq!(series.certification.as_deref(), Some("TV-14"));
    }
}
