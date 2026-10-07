//! TMDB API v3 client.
//!
//! Details, keywords and certifications come back in a single request through
//! `append_to_response`. Fetching them separately costs a call per item and
//! leaves the certification unpopulated, so `certification_in` matches
//! nothing.

use std::collections::BTreeMap;

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::{language, send_json};
use crate::error::{AppError, AppResult};

const SERVICE: &str = "TMDB";

#[derive(Debug, Clone)]
pub struct TmdbClient {
    client: Client,
    api_key: String,
    /// API root, so a mirror or caching proxy can be used instead.
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TmdbGenre {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TmdbKeyword {
    pub name: String,
}

#[derive(Debug, Deserialize, Default)]
struct KeywordsBlock {
    #[serde(default)]
    keywords: Vec<TmdbKeyword>,
    /// TV shows return the same payload under `results`.
    #[serde(default)]
    results: Vec<TmdbKeyword>,
}

#[derive(Debug, Deserialize, Default)]
struct ReleaseDatesBlock {
    #[serde(default)]
    results: Vec<ReleaseDatesEntry>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDatesEntry {
    iso_3166_1: String,
    #[serde(default)]
    release_dates: Vec<ReleaseDate>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDate {
    #[serde(default)]
    certification: String,
}

#[derive(Debug, Deserialize, Default)]
struct ContentRatingsBlock {
    #[serde(default)]
    results: Vec<ContentRating>,
}

#[derive(Debug, Deserialize)]
struct ContentRating {
    iso_3166_1: String,
    #[serde(default)]
    rating: String,
}

#[derive(Debug, Deserialize)]
struct ProductionCountry {
    iso_3166_1: String,
}

/// Raw movie payload with the appended blocks.
#[derive(Debug, Deserialize)]
struct RawMovie {
    #[serde(default)]
    genres: Vec<TmdbGenre>,
    original_language: Option<String>,
    #[serde(default)]
    origin_country: Vec<String>,
    #[serde(default)]
    production_countries: Vec<ProductionCountry>,
    status: Option<String>,
    overview: Option<String>,
    #[serde(default)]
    keywords: KeywordsBlock,
    #[serde(default)]
    release_dates: ReleaseDatesBlock,
}

/// Raw TV payload with the appended blocks.
#[derive(Debug, Deserialize)]
struct RawTv {
    #[serde(default)]
    genres: Vec<TmdbGenre>,
    original_language: Option<String>,
    #[serde(default)]
    origin_country: Vec<String>,
    #[serde(default)]
    production_countries: Vec<ProductionCountry>,
    status: Option<String>,
    overview: Option<String>,
    #[serde(default)]
    keywords: KeywordsBlock,
    #[serde(default)]
    content_ratings: ContentRatingsBlock,
}

/// Normalized metadata, identical for movies and series.
#[derive(Debug, Clone)]
pub struct TmdbDetails {
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    /// Every country's rating, by upper-case country code.
    pub certifications: BTreeMap<String, String>,
    pub status: Option<String>,
    pub overview: Option<String>,
}

impl TmdbClient {
    pub fn new(client: Client, api_key: &str, base_url: &str) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// TMDB accepts a v3 key as an `api_key` query parameter and a v4 token as a
    /// bearer. Users paste either, and sending *both* forms every time would
    /// put a v4 token in the query string, where a mirror, a caching proxy or
    /// an access log would record it. A v4 token is a JWT, so telling them
    /// apart is unambiguous, and each is sent only the way it is meant to be.
    fn get(&self, path: &str, query: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let request = self.client.get(format!("{}{path}", self.base_url));
        let request = if is_v4_token(&self.api_key) {
            request.bearer_auth(&self.api_key)
        } else {
            request.query(&[("api_key", self.api_key.as_str())])
        };
        request.query(query)
    }

    /// Cheap reachability probe used by the health page.
    pub async fn test_connection(&self) -> AppResult<bool> {
        match send_json::<serde_json::Value>(SERVICE, self.get("/configuration", &[])).await {
            Ok(_) => Ok(true),
            Err(AppError::ExternalApi { status, .. }) if status == 401 || status == 403 => {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// Fetch everything Routarr needs about a movie in one request.
    pub async fn get_movie(&self, tmdb_id: i64) -> AppResult<TmdbDetails> {
        debug!("Fetching TMDB movie {tmdb_id}");
        let raw: RawMovie = send_json(
            SERVICE,
            self.get(
                &format!("/movie/{tmdb_id}"),
                &[("append_to_response", "keywords,release_dates")],
            ),
        )
        .await?;

        Ok(TmdbDetails {
            genres: raw.genres.into_iter().map(|g| g.name).collect(),
            keywords: merge_keywords(raw.keywords),
            original_language: raw.original_language.as_deref().and_then(language::from_tmdb),
            origin_countries: countries(raw.origin_country, raw.production_countries),
            certifications: movie_certifications(&raw.release_dates),
            status: raw.status,
            overview: raw.overview,
        })
    }

    /// Fetch everything Routarr needs about a series in one request.
    pub async fn get_tv(&self, tmdb_id: i64) -> AppResult<TmdbDetails> {
        debug!("Fetching TMDB series {tmdb_id}");
        let raw: RawTv = send_json(
            SERVICE,
            self.get(
                &format!("/tv/{tmdb_id}"),
                &[("append_to_response", "keywords,content_ratings")],
            ),
        )
        .await?;

        Ok(TmdbDetails {
            genres: raw.genres.into_iter().map(|g| g.name).collect(),
            keywords: merge_keywords(raw.keywords),
            original_language: raw.original_language.as_deref().and_then(language::from_tmdb),
            origin_countries: countries(raw.origin_country, raw.production_countries),
            certifications: tv_certifications(&raw.content_ratings),
            status: raw.status,
            overview: raw.overview,
        })
    }

    /// Dispatch on Routarr's own media type discriminator.
    pub async fn get_details(&self, tmdb_id: i64, media_type: &str) -> AppResult<TmdbDetails> {
        match media_type {
            "movie" => self.get_movie(tmdb_id).await,
            "series" => self.get_tv(tmdb_id).await,
            other => Err(AppError::BadRequest(format!("Unknown media type '{other}'"))),
        }
    }
}

/// A v4 read access token is a JWT: three base64url segments after a `eyJ`
/// header. A v3 key is 32 hex characters and can never look like one.
fn is_v4_token(credential: &str) -> bool {
    credential.starts_with("eyJ") && credential.matches('.').count() == 2
}

fn merge_keywords(block: KeywordsBlock) -> Vec<String> {
    let mut all: Vec<String> =
        block.keywords.into_iter().chain(block.results).map(|k| k.name).collect();
    all.sort();
    all.dedup();
    all
}

/// `origin_country` is only populated on some TMDB records. The production
/// countries stand in for it, so `origin_country` conditions still have data to
/// work with.
fn countries(origin: Vec<String>, production: Vec<ProductionCountry>) -> Vec<String> {
    if !origin.is_empty() {
        return origin;
    }
    production.into_iter().map(|c| c.iso_3166_1).collect()
}

/// Each country's rating of a film: the first of its releases there that is
/// rated, since TMDB lists a release before its rating is known.
fn movie_certifications(block: &ReleaseDatesBlock) -> BTreeMap<String, String> {
    rated(block.results.iter().map(|entry| {
        let rating = entry.release_dates.iter().map(|r| r.certification.as_str());
        (entry.iso_3166_1.as_str(), rating.map(str::trim).find(|c| !c.is_empty()))
    }))
}

/// Each country's rating of a series.
fn tv_certifications(block: &ContentRatingsBlock) -> BTreeMap<String, String> {
    rated(block.results.iter().map(|entry| (entry.iso_3166_1.as_str(), Some(entry.rating.trim()))))
}

/// The countries rated, by upper-case code, the first rating of each kept. A
/// blank rating rates nothing.
fn rated<'a>(
    ratings: impl Iterator<Item = (&'a str, Option<&'a str>)>,
) -> BTreeMap<String, String> {
    let mut by_country = BTreeMap::new();
    for (country, rating) in ratings {
        if let Some(rating) = rating.filter(|rating| !rating.is_empty()) {
            by_country.entry(country.trim().to_ascii_uppercase()).or_insert(rating.to_string());
        }
    }
    by_country
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_country(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(country, rating)| (country.to_string(), rating.to_string())).collect()
    }

    #[test]
    fn a_v4_token_is_told_apart_from_a_v3_key() {
        // Sending a v4 token as a query parameter would write it into every
        // access log between here and TMDB.
        assert!(is_v4_token("eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ4In0.c2lnbmF0dXJl"));
        assert!(!is_v4_token("0123456789abcdef0123456789abcdef"));
        assert!(!is_v4_token(""));
        // Shaped like the start of a token but not one.
        assert!(!is_v4_token("eyJnotajwt"));
    }

    /// A captured-shape TMDB movie, appended blocks included, through the
    /// real types: the fields the client ignores, a release with an empty
    /// certification before the rated one, and `origin_country` beside the
    /// production countries. A change in TMDB's shape surfaces here.
    #[test]
    fn a_real_shape_movie_payload_deserialises() {
        let json = r#"{
          "adult": false, "id": 8392, "imdb_id": "tt0096283",
          "title": "My Neighbor Totoro", "original_title": "となりのトトロ",
          "original_language": "ja", "origin_country": ["JP"],
          "genres": [{"id": 16, "name": "Animation"}, {"id": 10751, "name": "Family"}],
          "production_countries": [{"iso_3166_1": "JP", "name": "Japan"}],
          "status": "Released", "overview": "Two sisters move to the country.",
          "poster_path": "/rtGDOeG9LzoerkDGZF9dnVeLppL.jpg", "runtime": 86,
          "keywords": {"keywords": [{"id": 1721, "name": "fight"}, {"id": 9663, "name": "sequel"}]},
          "release_dates": {"results": [
            {"iso_3166_1": "US", "release_dates": [
              {"certification": "", "iso_639_1": "", "note": "", "release_date": "1989-05-07T00:00:00.000Z", "type": 3},
              {"certification": "G", "iso_639_1": "", "note": "", "release_date": "1993-05-07T00:00:00.000Z", "type": 3}
            ]},
            {"iso_3166_1": "JP", "release_dates": [
              {"certification": "G", "iso_639_1": "ja", "note": "", "release_date": "1988-04-16T00:00:00.000Z", "type": 3}
            ]}
          ]}
        }"#;

        let raw: RawMovie = serde_json::from_str(json).unwrap();

        assert_eq!(
            raw.genres.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["Animation", "Family"]
        );
        assert_eq!(raw.original_language.as_deref(), Some("ja"));
        assert_eq!(countries(raw.origin_country, raw.production_countries), ["JP"]);
        assert_eq!(merge_keywords(raw.keywords), ["fight", "sequel"]);
        assert_eq!(
            movie_certifications(&raw.release_dates),
            by_country(&[("JP", "G"), ("US", "G")])
        );
    }

    /// A blank TV rating rates nothing, and a country listed twice keeps its
    /// first rating, under its upper-case code.
    #[test]
    fn a_blank_tv_rating_rates_nothing() {
        let ratings = |pairs: &[(&str, &str)]| ContentRatingsBlock {
            results: pairs
                .iter()
                .map(|(country, rating)| ContentRating {
                    iso_3166_1: country.to_string(),
                    rating: rating.to_string(),
                })
                .collect(),
        };

        let block = ratings(&[("US", "  "), ("FR", "-12"), ("fr", "16"), ("gb", "15")]);
        assert_eq!(tv_certifications(&block), by_country(&[("FR", "-12"), ("GB", "15")]));
        assert_eq!(tv_certifications(&ratings(&[("US", "")])), by_country(&[]));
    }

    #[test]
    fn merges_movie_and_tv_keyword_shapes() {
        let block = KeywordsBlock {
            keywords: vec![TmdbKeyword { name: "anime".into() }],
            results: vec![TmdbKeyword { name: "concert".into() }],
        };
        assert_eq!(merge_keywords(block), vec!["anime", "concert"]);
    }

    #[test]
    fn falls_back_to_production_countries() {
        let out = countries(vec![], vec![ProductionCountry { iso_3166_1: "JP".into() }]);
        assert_eq!(out, vec!["JP"]);
    }

    #[test]
    fn prefers_origin_country_when_present() {
        let out = countries(vec!["KR".into()], vec![ProductionCountry { iso_3166_1: "US".into() }]);
        assert_eq!(out, vec!["KR"]);
    }

    /// TMDB lists a release before its rating is known: the first rated
    /// release of each country is its rating.
    #[test]
    fn a_film_is_rated_by_its_first_rated_release_in_each_country() {
        let block = ReleaseDatesBlock {
            results: vec![
                ReleaseDatesEntry {
                    iso_3166_1: "FR".into(),
                    release_dates: vec![
                        ReleaseDate { certification: "  ".into() },
                        ReleaseDate { certification: "12".into() },
                        ReleaseDate { certification: "16".into() },
                    ],
                },
                ReleaseDatesEntry {
                    iso_3166_1: "DE".into(),
                    release_dates: vec![ReleaseDate { certification: String::new() }],
                },
            ],
        };
        assert_eq!(movie_certifications(&block), by_country(&[("FR", "12")]));
    }
}
