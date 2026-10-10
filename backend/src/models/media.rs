use serde::{Deserialize, Serialize};

/// A media item synchronized from an Arr instance.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Media {
    pub id: String,
    pub instance_id: String,
    pub arr_id: i64,
    pub media_type: String,
    pub title: String,
    pub sort_title: Option<String>,
    pub year: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub current_path: Option<String>,
    pub current_root_folder: Option<String>,
    pub monitored: bool,
    pub has_files: bool,
    pub status: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub added_at: Option<String>,
    /// Sonarr's own classification (`standard`, `anime` or `daily`), null for
    /// movies.
    pub series_type: Option<String>,
    pub size_on_disk: Option<i64>,
    /// Seasons excluding specials, null for movies.
    pub season_count: Option<i64>,
    /// The Arr's tag labels, as a JSON array in a string.
    // Denormalised from the Arr, and read in bulk on every rule evaluation and
    // by the library's tag facet.
    pub tags: Option<String>,
    /// The genres Radarr or Sonarr reports, as a JSON array in a string.
    // The `arr` metadata source reads this column, the language and the
    // certification, which cost no request and go stale only when the library
    // does.
    pub genres: Option<String>,
    /// ISO 639-1, normalised from the Arr's language *name* at sync time.
    pub original_language: Option<String>,
    pub certification: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_synced_at: Option<String>,
}

impl Media {
    /// Tag labels, lowercased for comparison.
    ///
    /// A malformed value yields no tags rather than an error: a tag is one
    /// signal among many and must not be able to break an evaluation.
    pub fn tag_labels(&self) -> Vec<String> {
        self.tags
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|label| label.trim().to_lowercase())
            .collect()
    }

    /// Genres as stored by the sync.
    pub fn genre_list(&self) -> Vec<String> {
        genres_from(self.genres.as_deref())
    }
}

/// The genres a `genres` column holds. Malformed JSON yields none, like
/// `tag_labels`: a metadata field must never break an evaluation.
pub fn genres_from(column: Option<&str>) -> Vec<String> {
    column.and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok()).unwrap_or_default()
}

/// A media row as the API shows one, its lists read out of the JSON strings
/// the row keeps them in.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MediaView {
    #[serde(flatten)]
    pub media: Media,
    /// The Arr's tag labels: `tags` holds the same as a JSON string.
    pub tag_list: Vec<String>,
    /// The genres the Arr reports: `genres` holds the same as a JSON string.
    pub genre_list: Vec<String>,
}

impl From<Media> for MediaView {
    fn from(media: Media) -> Self {
        let tag_list = media
            .tags
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .unwrap_or_default();
        let genre_list = media.genre_list();
        MediaView { media, tag_list, genre_list }
    }
}

/// Query parameters for media listing.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct MediaQuery {
    /// Only the titles of this instance.
    pub instance_id: Option<String>,
    /// `movie` or `series`.
    pub media_type: Option<String>,
    /// Only the titles the last run sends to this category.
    pub category: Option<String>,
    /// Part of the title, ASCII letters in any case.
    pub search: Option<String>,
    /// Only the titles with this TMDB id.
    pub tmdb_id: Option<i64>,
    /// Only the titles with this TheTVDB id.
    pub tvdb_id: Option<i64>,
    /// Only the titles with this IMDb id, as `tt0133093`.
    pub imdb_id: Option<String>,
    /// Only the titles no rule matched at the last run, and those no run has
    /// read yet, for the "unclassified" view.
    pub unmatched: Option<bool>,
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
}

/// The id another service gives a title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalId {
    Tmdb(i64),
    Tvdb(i64),
    Imdb(String),
}

impl ExternalId {
    /// The one id among these, or `None` when there is none or more than one.
    pub fn one_of(tmdb: Option<i64>, tvdb: Option<i64>, imdb: Option<&str>) -> Option<Self> {
        match (tmdb, tvdb, imdb.map(str::trim)) {
            (Some(tmdb), None, None) => Some(Self::Tmdb(tmdb)),
            (None, Some(tvdb), None) => Some(Self::Tvdb(tvdb)),
            (None, None, Some(imdb)) if !imdb.is_empty() => Some(Self::Imdb(imdb.to_string())),
            _ => None,
        }
    }

    /// The `media` column holding this id.
    pub fn column(&self) -> &'static str {
        match self {
            Self::Tmdb(_) => "tmdb_id",
            Self::Tvdb(_) => "tvdb_id",
            Self::Imdb(_) => "imdb_id",
        }
    }

    /// The id as that column compares it.
    pub fn value(&self) -> String {
        match self {
            Self::Tmdb(id) | Self::Tvdb(id) => id.to_string(),
            Self::Imdb(id) => id.clone(),
        }
    }
}
