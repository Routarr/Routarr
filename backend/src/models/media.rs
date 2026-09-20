use serde::{Deserialize, Serialize};

/// A media item synchronized from an Arr instance.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    pub added_at: Option<String>,
    /// Sonarr's own classification (`standard` / `anime` / `daily`); `None` for
    /// movies.
    pub series_type: Option<String>,
    pub size_on_disk: Option<i64>,
    /// Seasons excluding specials; `None` for movies.
    pub season_count: Option<i64>,
    /// Tag labels as a JSON array, denormalised from the Arr. Read in bulk on
    /// every rule evaluation and never queried on its own.
    pub tags: Option<String>,
    /// Genres as a JSON array, straight from Radarr or Sonarr. The `arr`
    /// metadata source reads these three columns; they cost no request and go
    /// stale only when the library does.
    pub genres: Option<String>,
    /// ISO 639-1, normalised from the Arr's language *name* at sync time.
    pub original_language: Option<String>,
    pub certification: Option<String>,
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

    /// Genres as stored by the sync. Malformed JSON yields none, like
    /// `tag_labels`: a metadata field must never break an evaluation.
    pub fn genre_list(&self) -> Vec<String> {
        self.genres
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .unwrap_or_default()
    }
}

/// Query parameters for media listing.
#[derive(Debug, Default, Deserialize)]
pub struct MediaQuery {
    pub instance_id: Option<String>,
    pub media_type: Option<String>,
    /// Filter on the category currently proposed by the engine.
    pub category: Option<String>,
    pub search: Option<String>,
    /// Only media that no rule matched — the "unclassified" view.
    pub unmatched: Option<bool>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}
