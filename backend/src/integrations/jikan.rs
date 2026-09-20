//! Jikan, the unofficial MyAnimeList API.
//!
//! Also credential-free, and a useful second opinion on anime: its *themes* and
//! *demographics* ("Shounen", "Iyashikei", "Military") are the vocabulary anime
//! libraries are actually organised by, and neither TMDb nor AniList expresses
//! them the same way.
//!
//! Two things it is not: official, and generous with its rate limit. It allows a
//! few requests per second, which is why `ROUTARR_METADATA_CONCURRENCY` bounds every
//! fetched source and why a 429 backs the whole pass off rather than retrying.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::send_json;
use crate::error::AppResult;

const SERVICE: &str = "Jikan";
pub const DEFAULT_BASE_URL: &str = "https://api.jikan.moe/v4";

#[derive(Debug, Clone)]
pub struct JikanClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct JikanCandidate {
    pub id: i64,
    pub year: Option<i64>,
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct JikanDetails {
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct RawAnime {
    mal_id: i64,
    year: Option<i64>,
    title: Option<String>,
    title_english: Option<String>,
    title_japanese: Option<String>,
    #[serde(default)]
    titles: Vec<RawTitle>,
    #[serde(default)]
    genres: Vec<Named>,
    #[serde(default)]
    themes: Vec<Named>,
    #[serde(default)]
    demographics: Vec<Named>,
    /// "R - 17+ (violence & profanity)" and friends.
    rating: Option<String>,
    status: Option<String>,
    synopsis: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTitle {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

impl JikanClient {
    /// Where this client is pointed, so a caller can tell the public API from a
    /// mirror or a proxy.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(client: Client, base_url: &str) -> Self {
        Self { client, base_url: base_url.trim_end_matches('/').to_string() }
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        let probe = self.client.get(format!("{}/anime/1", self.base_url));
        Ok(send_json::<serde_json::Value>(SERVICE, probe).await.is_ok())
    }

    pub async fn search(&self, title: &str, media_type: &str) -> AppResult<Vec<JikanCandidate>> {
        debug!("Searching Jikan for {title}");
        let mut request = self
            .client
            .get(format!("{}/anime", self.base_url))
            .query(&[("q", title), ("limit", "5")]);
        if media_type == "movie" {
            request = request.query(&[("type", "movie")]);
        }

        let response: Envelope<Vec<RawAnime>> = send_json(SERVICE, request).await?;

        Ok(response
            .data
            .into_iter()
            .map(|raw| JikanCandidate { id: raw.mal_id, year: raw.year, titles: titles_of(&raw) })
            .collect())
    }

    pub async fn get_details(&self, id: i64) -> AppResult<JikanDetails> {
        debug!("Fetching Jikan {id}");
        let response: Envelope<RawAnime> =
            send_json(SERVICE, self.client.get(format!("{}/anime/{id}/full", self.base_url)))
                .await?;
        let raw = response.data;

        // Themes and demographics are what an anime library is actually sorted
        // by; as genres they would collide with TMDb's much coarser list, so
        // they land in the keywords where a rule can name them precisely.
        let mut keywords: Vec<String> = raw
            .themes
            .into_iter()
            .chain(raw.demographics)
            .map(|entry| entry.name.to_lowercase())
            .collect();
        keywords.sort();
        keywords.dedup();

        Ok(JikanDetails {
            genres: raw.genres.into_iter().map(|entry| entry.name).collect(),
            keywords,
            certification: raw.rating.map(|rating| shorten_rating(&rating)),
            status: raw.status,
            overview: raw.synopsis,
        })
    }
}

/// `"R - 17+ (violence & profanity)"` -> `"R - 17+"`.
///
/// A `certification_in` rule is written against a rating, not against a
/// parenthesised justification.
fn shorten_rating(rating: &str) -> String {
    rating.split('(').next().unwrap_or(rating).trim().to_string()
}

fn titles_of(raw: &RawAnime) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    all.extend(raw.title.clone());
    all.extend(raw.title_english.clone());
    all.extend(raw.title_japanese.clone());
    all.extend(raw.titles.iter().filter_map(|entry| entry.title.clone()));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured-shape Jikan `/anime/{id}/full` response through the real
    /// types. The edges are the ones the live API actually has: `year` is null
    /// on many entries (the date lives in `aired` instead), `titles[]` is the
    /// modern field while `title_*` are the deprecated ones still returned, and
    /// there are far more fields than the client reads.
    #[test]
    fn a_real_shape_details_payload_deserialises_and_maps() {
        let json = r#"{
          "data": {
            "mal_id": 523,
            "url": "https://myanimelist.net/anime/523/Tonari_no_Totoro",
            "title": "Tonari no Totoro",
            "title_english": "My Neighbor Totoro",
            "title_japanese": "となりのトトロ",
            "titles": [
              { "type": "Default", "title": "Tonari no Totoro" },
              { "type": "English", "title": "My Neighbor Totoro" }
            ],
            "type": "Movie",
            "episodes": 1,
            "status": "Finished Airing",
            "year": null,
            "rating": "G - All Ages",
            "synopsis": "Two sisters move to the country.",
            "genres": [{ "mal_id": 2, "name": "Adventure" }],
            "themes": [{ "mal_id": 63, "name": "Iyashikei" }],
            "demographics": [{ "mal_id": 15, "name": "Kids" }]
          }
        }"#;

        let response: Envelope<RawAnime> = serde_json::from_str(json).unwrap();
        let raw = response.data;

        assert_eq!(raw.mal_id, 523);
        // A null year must not break the row; resolution simply cannot use it.
        assert_eq!(raw.year, None);
        assert!(titles_of(&raw).contains(&"My Neighbor Totoro".to_string()));
        assert_eq!(raw.genres.len(), 1);
        assert_eq!(shorten_rating(raw.rating.as_deref().unwrap()), "G - All Ages");
        // Themes and demographics are the keyword vocabulary.
        assert_eq!(raw.themes[0].name, "Iyashikei");
        assert_eq!(raw.demographics[0].name, "Kids");
    }

    #[test]
    fn a_rating_keeps_only_what_a_rule_can_name() {
        assert_eq!(shorten_rating("R - 17+ (violence & profanity)"), "R - 17+");
        assert_eq!(shorten_rating("PG-13 - Teens 13 or older"), "PG-13 - Teens 13 or older");
    }
}
