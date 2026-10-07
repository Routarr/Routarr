//! OMDb.
//!
//! Keyed on the **IMDb** identifier, which is the reason to have it: it answers
//! for an item TMDB does not know, or one the library carries without a TMDB id.
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
/// The requests a free key may send in a UTC day.
pub const FREE_DAILY_REQUESTS: i64 = 1_000;
/// The title a probe asks for.
const PROBED: &str = "tt0096283";

#[derive(Debug, Clone)]
pub struct OmdbClient {
    client: Client,
    api_key: String,
    base_url: String,
    daily_requests: i64,
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

    /// `daily_requests` is what the key may send in a UTC day, which
    /// [`FetchingSource::daily_quota`](crate::services::metadata::FetchingSource::daily_quota)
    /// counts.
    pub fn new(client: Client, api_key: &str, base_url: &str, daily_requests: i64) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            daily_requests,
        }
    }

    pub fn daily_requests(&self) -> i64 {
        self.daily_requests
    }

    fn get(&self, query: &[(&str, &str)]) -> reqwest::RequestBuilder {
        self.client.get(&self.base_url).query(&[("apikey", self.api_key.as_str())]).query(query)
    }

    /// The body for one IMDb id. OMDb refuses a spent quota with a 401, as it
    /// refuses a wrong key, and only its words tell the two apart.
    async fn ask(&self, imdb_id: &str) -> AppResult<RawResponse> {
        send_json(SERVICE, self.get(&[("i", imdb_id)])).await.map_err(|error| match &error {
            AppError::ExternalApi { status: 401, message, .. } if is_quota(message) => {
                super::quota_spent(SERVICE)
            }
            _ => error,
        })
    }

    /// Whether the key is accepted. A wrong key is a rejection, not an
    /// outage, and a spent quota says so.
    pub async fn test_connection(&self) -> AppResult<bool> {
        let raw = self.ask(PROBED).await?;
        refusal(&raw).map_or(Ok(is_found(&raw)), Err)
    }

    /// Everything Routarr needs about one IMDb id.
    pub async fn get_details(&self, imdb_id: &str) -> AppResult<OmdbDetails> {
        debug!("Fetching OMDb {imdb_id}");
        details_of(self.ask(imdb_id).await?, imdb_id)
    }
}

/// The error a refusing body says, a spent quota as one: a refused key or a
/// spent quota is refused for every title alike, which the enrichment breaker
/// counts, never a miss cached empty for days.
fn refusal(raw: &RawResponse) -> Option<AppError> {
    let error = raw.error.as_deref().filter(|error| !is_found(raw) && is_refusal(error))?;
    Some(if is_quota(error) {
        super::quota_spent(SERVICE)
    } else {
        AppError::ExternalApi {
            service: SERVICE.into(),
            status: 401,
            message: error.to_string(),
            retry_after: None,
        }
    })
}

/// The details a body carries, nothing for a miss, and an error for a refusal.
fn details_of(raw: RawResponse, imdb_id: &str) -> AppResult<OmdbDetails> {
    if let Some(error) = refusal(&raw) {
        return Err(error);
    }
    if !is_found(&raw) {
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
    error.to_ascii_lowercase().contains("api key") || is_quota(error)
}

fn is_quota(error: &str) -> bool {
    error.to_ascii_lowercase().contains("limit")
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

    /// A refused key or a spent quota is an error, a spent quota its own,
    /// whether OMDb sends it with a 401 or a 200. A miss is an empty answer,
    /// which is cached.
    #[test]
    fn a_refusal_is_an_error_and_a_miss_is_an_empty_answer() {
        for refused in ["Invalid API key!", "No API key provided."] {
            let outcome = details_of(answering(refused), "tt0096283");
            assert!(
                matches!(outcome, Err(AppError::ExternalApi { status: 401, .. })),
                "{refused}: {outcome:?}"
            );
        }
        let spent = details_of(answering("Request limit reached!"), "tt0096283").unwrap_err();
        assert!(crate::integrations::is_quota_spent(&spent), "{spent:?}");
        for missed in ["Incorrect IMDb ID.", "Movie not found!"] {
            let outcome = details_of(answering(missed), "tt0000001").expect(missed);
            assert!(outcome.genres.is_empty() && outcome.certification.is_none(), "{missed}");
        }
    }
}
