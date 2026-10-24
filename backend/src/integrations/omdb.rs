//! OMDb, keyed on the IMDb id, for an item TMDB does not know or the library holds with no
//! TMDB id. It refuses a request with no key, hence `needs_key`. Its prose (`"Japan, United
//! States"`) goes through `integrations::language`, which drops what it cannot map.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::{language, send_json};
use crate::error::{AppError, AppResult};

const SERVICE: &str = "OMDb";
pub const DEFAULT_BASE_URL: &str = "https://www.omdbapi.com";
/// The title a probe asks for.
const PROBED: &str = "tt0096283";
/// What OMDb answers for an id it does not hold.
const MISSES: [&str; 3] = ["Movie not found!", "Incorrect IMDb ID.", "Series or season not found!"];

#[derive(Clone)]
pub struct OmdbClient {
    client: Client,
    api_key: String,
    base_url: String,
    daily_requests: i64,
}

// The key stays out: a client printed in a log line or a failed assertion would carry it.
impl std::fmt::Debug for OmdbClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OmdbClient").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
pub struct OmdbDetails {
    pub genres: Vec<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
    pub overview: Option<String>,
    /// IMDb's score and votes, Metacritic's and Rotten Tomatoes', by scale.
    pub ratings: std::collections::BTreeMap<String, crate::models::Rating>,
}

/// One entry of OMDb's `Ratings`, as `{"Source": "Rotten Tomatoes", "Value": "94%"}`.
#[derive(Debug, Deserialize)]
struct RawRating {
    #[serde(rename = "Source")]
    source: Option<String>,
    #[serde(rename = "Value")]
    value: Option<String>,
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
    #[serde(rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(rename = "imdbVotes")]
    imdb_votes: Option<String>,
    #[serde(rename = "Metascore")]
    metascore: Option<String>,
    #[serde(rename = "Ratings", default)]
    ratings: Vec<RawRating>,
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
        super::upstream(SERVICE, 401, crate::error::Upstream::RefusedWith200, error.to_string())
    })
}

/// The details a body carries, nothing for a miss, and an error for a refusal.
fn details_of(raw: RawResponse, imdb_id: &str) -> AppResult<OmdbDetails> {
    if let Some(error) = refusal(&raw) {
        return Err(error);
    }
    if !is_found(&raw) {
        let said = raw.error.as_deref().unwrap_or_default();
        // A miss is an answer: cached empty, it is not asked again next pass.
        if MISSES.iter().any(|miss| said.trim().eq_ignore_ascii_case(miss)) {
            debug!("OMDb has nothing for {imdb_id}: {said}");
            return Ok(OmdbDetails::default());
        }
        // "Error getting data." and the like: neither cached as a miss nor counted against OMDb.
        return Err(super::upstream(
            SERVICE,
            200,
            crate::error::Upstream::RefusedWith200,
            said.to_string(),
        ));
    }

    let ratings = ratings(&raw);
    Ok(OmdbDetails {
        genres: split_list(raw.genre.as_deref()),
        original_language: raw.language.as_deref().and_then(language::first_language),
        origin_countries: raw.country.as_deref().map(language::country_codes).unwrap_or_default(),
        // OMDb writes "N/A" where it has nothing.
        certification: raw.rated.filter(|value| usable(value)),
        overview: raw.plot.filter(|value| usable(value)),
        ratings,
    })
}

/// The scores OMDb gives, written as text: `8.1`, `393,000`, `86`, `94%`, or `N/A`.
fn ratings(raw: &RawResponse) -> std::collections::BTreeMap<String, crate::models::Rating> {
    let number = |text: Option<&str>| -> Option<f64> {
        text.filter(|value| usable(value))?
            .trim()
            .trim_end_matches('%')
            .replace(',', "")
            .parse()
            .ok()
    };
    let critics = raw
        .ratings
        .iter()
        .find(|rating| rating.source.as_deref() == Some("Rotten Tomatoes"))
        .and_then(|rating| rating.value.as_deref());
    let votes = number(raw.imdb_votes.as_deref()).map(|votes| votes as i64);
    [
        crate::models::rating("imdb", number(raw.imdb_rating.as_deref()), votes),
        crate::models::rating("metacritic", number(raw.metascore.as_deref()), None),
        crate::models::rating("rotten_tomatoes", number(critics), None),
    ]
    .into_iter()
    .flatten()
    .collect()
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
          "Ratings": [
            { "Source": "Internet Movie Database", "Value": "8.1/10" },
            { "Source": "Rotten Tomatoes", "Value": "94%" },
            { "Source": "Metacritic", "Value": "86/100" }
          ],
          "Metascore": "86",
          "imdbRating": "8.1",
          "imdbVotes": "393,000",
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
        let scores = ratings(&raw);
        assert_eq!((scores["imdb"].value, scores["imdb"].votes), (8.1, Some(393_000)));
        assert_eq!((scores["metacritic"].value, scores["rotten_tomatoes"].value), (86.0, 94.0));
    }

    /// The same endpoint for a series with no rating: OMDb writes the string
    /// "N/A", which must not become a certification a rule could match.
    #[test]
    fn a_real_shape_na_payload_yields_absence() {
        let json = r#"{
          "Title": "Some Show", "Rated": "N/A", "Genre": "N/A",
          "Language": "N/A", "Country": "N/A", "Plot": "N/A",
          "imdbRating": "N/A", "imdbVotes": "N/A", "Metascore": "N/A",
          "Ratings": [{ "Source": "Rotten Tomatoes", "Value": "N/A" }],
          "Type": "series", "Response": "True"
        }"#;

        let raw: RawResponse = serde_json::from_str(json).unwrap();
        assert!(is_found(&raw));
        let details = details_of(raw, "tt0000001").unwrap();
        assert!(details.genres.is_empty(), "{:?}", details.genres);
        assert_eq!(details.original_language, None);
        assert!(details.origin_countries.is_empty(), "{:?}", details.origin_countries);
        assert_eq!(details.certification, None);
        assert_eq!(details.overview, None);
        assert!(details.ratings.is_empty(), "{:?}", details.ratings);
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
            imdb_rating: None,
            imdb_votes: None,
            metascore: None,
            ratings: Vec::new(),
        }
    }

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
        for missed in ["Incorrect IMDb ID.", "Movie not found!", "Series or season not found!"] {
            let outcome = details_of(answering(missed), "tt0000001").expect(missed);
            assert!(outcome.genres.is_empty() && outcome.certification.is_none(), "{missed}");
        }
        // OMDb failing on its side: neither a refusal of the key nor an answer to cache.
        for failed in ["Error getting data.", "Something new went wrong."] {
            let outcome = details_of(answering(failed), "tt7241928");
            assert!(
                matches!(&outcome, Err(AppError::ExternalApi { status: 200, .. })),
                "{failed}: {outcome:?}"
            );
        }
    }
}
