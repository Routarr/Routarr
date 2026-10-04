//! OMDb.
//!
//! Keyed on the **IMDb** identifier, which is the reason to have it: it answers
//! for an item TMDb does not know, or one the library carries without a TMDb id.
//! A free key is required: the API rejects an unauthenticated request outright
//! rather than degrading, so the source declares `needs_key`.
//!
//! Its vocabulary is prose: `"Japan, United States"` and `"Japanese, English"`
//! where the rest of Routarr speaks ISO codes. `integrations::language` does the
//! conversion, and drops what it cannot map rather than inventing a code.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::{language, send_json};
use crate::error::{AppError, AppResult};

const SERVICE: &str = "OMDb";
pub const DEFAULT_BASE_URL: &str = "https://www.omdbapi.com";

#[derive(Debug, Clone)]
pub struct OmdbClient {
    client: Client,
    api_key: String,
    base_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct OmdbDetails {
    pub genres: Vec<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawResponse {
    /// OMDb answers 200 with `"Response": "False"` for "not found" and for a
    /// rejected key alike, so success has to be read from the body.
    #[serde(rename = "Response")]
    response: Option<String>,
    #[serde(rename = "Error")]
    error: Option<String>,
    #[serde(rename = "Genre")]
    genre: Option<String>,
    #[serde(rename = "Language")]
    language: Option<String>,
    #[serde(rename = "Country")]
    country: Option<String>,
    #[serde(rename = "Rated")]
    rated: Option<String>,
    #[serde(rename = "Plot")]
    plot: Option<String>,
}

impl OmdbClient {
    /// Where this client is pointed, so a caller can tell the public API from a
    /// mirror or a proxy.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(client: Client, api_key: &str, base_url: &str) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn get(&self, query: &[(&str, &str)]) -> reqwest::RequestBuilder {
        self.client.get(&self.base_url).query(&[("apikey", self.api_key.as_str())]).query(query)
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        let raw: RawResponse = send_json(SERVICE, self.get(&[("i", "tt0096283")])).await?;
        // A wrong key is a rejection, not an outage: say so instead of raising.
        Ok(is_found(&raw))
    }

    /// Everything Routarr needs about one IMDb id.
    pub async fn get_details(&self, imdb_id: &str) -> AppResult<OmdbDetails> {
        debug!("Fetching OMDb {imdb_id}");
        let raw: RawResponse = send_json(SERVICE, self.get(&[("i", imdb_id)])).await?;
        details_of(raw, imdb_id)
    }
}

/// The details a body carries, nothing for a miss, and an error for a refusal.
fn details_of(raw: RawResponse, imdb_id: &str) -> AppResult<OmdbDetails> {
    if !is_found(&raw) {
        // A refused key or a spent quota is refused for every title alike, so
        // it is an error, which the enrichment breaker counts, never a miss
        // cached empty for days.
        if let Some(error) = raw.error.as_deref().filter(|error| is_refusal(error)) {
            return Err(AppError::ExternalApi {
                service: SERVICE.into(),
                status: 401,
                message: error.to_string(),
                retry_after: None,
            });
        }
        // "Movie not found" is an answer. Caching it empty is what stops the
        // next pass from asking again.
        debug!("OMDb has nothing for {imdb_id}: {:?}", raw.error);
        return Ok(OmdbDetails::default());
    }

    Ok(OmdbDetails {
        genres: split_list(raw.genre.as_deref()),
        original_language: raw.language.as_deref().and_then(language::first_language),
        origin_countries: raw.country.as_deref().map(language::country_codes).unwrap_or_default(),
        // OMDb writes "N/A" where it has nothing.
        certification: raw.rated.filter(|value| usable(value)),
        overview: raw.plot.filter(|value| usable(value)),
    })
}

/// Whether OMDb's `Error` refuses the key ("Invalid API key!", "No API key
/// provided.") or the quota ("Request limit reached!") rather than the title.
fn is_refusal(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("api key") || error.contains("limit")
}

fn is_found(raw: &RawResponse) -> bool {
    raw.response.as_deref().is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn usable(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && !value.eq_ignore_ascii_case("n/a")
}

fn split_list(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| usable(entry))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured-shape OMDb response through the real types, including the
    /// fields the client ignores and the `"N/A"` OMDb uses instead of null.
    /// This is where a change in its prose formatting would surface.
    #[test]
    fn a_real_shape_payload_deserialises_and_normalises() {
        let json = r#"{
          "Title": "My Neighbor Totoro",
          "Year": "1988",
          "Rated": "G",
          "Released": "16 Apr 1988",
          "Runtime": "86 min",
          "Genre": "Animation, Family, Fantasy",
          "Director": "Hayao Miyazaki",
          "Language": "Japanese, English",
          "Country": "Japan",
          "Awards": "6 wins & 1 nomination",
          "Poster": "https://example.invalid/poster.jpg",
          "Metascore": "86",
          "imdbRating": "8.1",
          "imdbID": "tt0096283",
          "Type": "movie",
          "Response": "True"
        }"#;

        let raw: RawResponse = serde_json::from_str(json).unwrap();
        assert!(is_found(&raw));
        assert_eq!(split_list(raw.genre.as_deref()), vec!["Animation", "Family", "Fantasy"]);
        // Prose, which `language` turns into codes (tested there and end to end).
        assert_eq!(raw.language.as_deref(), Some("Japanese, English"));
        assert_eq!(raw.country.as_deref(), Some("Japan"));
        assert_eq!(raw.rated.as_deref(), Some("G"));
    }

    /// The same endpoint for a series with no rating: OMDb writes the string
    /// "N/A", which must not become a certification a rule could match.
    #[test]
    fn a_real_shape_na_payload_yields_absence() {
        let json = r#"{
          "Title": "Some Show", "Rated": "N/A", "Genre": "N/A",
          "Language": "N/A", "Country": "N/A", "Plot": "N/A",
          "Type": "series", "Response": "True"
        }"#;

        let raw: RawResponse = serde_json::from_str(json).unwrap();
        assert!(is_found(&raw));
        // Every field read as the client reads it, so none of the "N/A" turns
        // into a value a rule could match.
        let details = details_of(raw, "tt0000001").unwrap();
        assert!(details.genres.is_empty(), "{:?}", details.genres);
        assert_eq!(details.original_language, None);
        assert!(details.origin_countries.is_empty(), "{:?}", details.origin_countries);
        assert_eq!(details.certification, None);
        assert_eq!(details.overview, None);
    }

    fn answering(error: &str) -> RawResponse {
        RawResponse {
            response: Some("False".into()),
            error: Some(error.into()),
            genre: None,
            language: None,
            country: None,
            rated: None,
            plot: None,
        }
    }

    /// A refused key or a spent quota is an error, which the breaker counts,
    /// whether OMDb sends it with a 401 or a 200. A miss is an empty answer,
    /// which is cached.
    #[test]
    fn a_refusal_is_an_error_and_a_miss_is_an_empty_answer() {
        for refused in ["Invalid API key!", "No API key provided.", "Request limit reached!"] {
            let outcome = details_of(answering(refused), "tt0096283");
            assert!(
                matches!(outcome, Err(AppError::ExternalApi { status: 401, .. })),
                "{refused}: {outcome:?}"
            );
        }
        for missed in ["Incorrect IMDb ID.", "Movie not found!"] {
            let outcome = details_of(answering(missed), "tt0000001").expect(missed);
            assert!(outcome.genres.is_empty() && outcome.certification.is_none(), "{missed}");
        }
    }
}
