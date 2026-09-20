//! TMDb API v3 client.
//!
//! Details, keywords and certifications come back in a single request through
//! `append_to_response`. Fetching them separately costs a call per item and
//! leaves the certification unpopulated, so `certification_in` matches
//! nothing.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::send_json;
use crate::error::{AppError, AppResult};

const SERVICE: &str = "TMDb";

#[derive(Debug, Clone)]
pub struct TmdbClient {
    client: Client,
    api_key: String,
    /// API root, so a mirror or caching proxy can be used instead.
    base_url: String,
    /// Preferred certification regions, most preferred first.
    regions: Vec<String>,
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
    poster_path: Option<String>,
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
    poster_path: Option<String>,
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
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

impl TmdbClient {
    pub fn new(client: Client, api_key: &str, base_url: &str, regions: &[String]) -> Self {
        let regions = if regions.is_empty() {
            vec!["US".to_string()]
        } else {
            regions.iter().map(|r| r.trim().to_uppercase()).collect()
        };
        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            regions,
        }
    }

    /// TMDb accepts a v3 key as an `api_key` query parameter and a v4 token as a
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
        debug!("Fetching TMDb movie {tmdb_id}");
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
            original_language: raw.original_language,
            origin_countries: countries(raw.origin_country, raw.production_countries),
            certification: pick_movie_certification(&raw.release_dates, &self.regions),
            status: raw.status,
            overview: raw.overview,
            poster_path: raw.poster_path,
        })
    }

    /// Fetch everything Routarr needs about a series in one request.
    pub async fn get_tv(&self, tmdb_id: i64) -> AppResult<TmdbDetails> {
        debug!("Fetching TMDb series {tmdb_id}");
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
            original_language: raw.original_language,
            origin_countries: countries(raw.origin_country, raw.production_countries),
            certification: pick_tv_certification(&raw.content_ratings, &self.regions),
            status: raw.status,
            overview: raw.overview,
            poster_path: raw.poster_path,
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

/// `origin_country` is only populated on some TMDb records; fall back to the
/// production countries so `origin_country` conditions still have data to work
/// with.
fn countries(origin: Vec<String>, production: Vec<ProductionCountry>) -> Vec<String> {
    if !origin.is_empty() {
        return origin;
    }
    production.into_iter().map(|c| c.iso_3166_1).collect()
}

fn pick_movie_certification(block: &ReleaseDatesBlock, regions: &[String]) -> Option<String> {
    for region in regions {
        let found = block
            .results
            .iter()
            .find(|entry| entry.iso_3166_1.eq_ignore_ascii_case(region))
            .and_then(|entry| {
                entry.release_dates.iter().map(|r| r.certification.trim()).find(|c| !c.is_empty())
            });
        if let Some(cert) = found {
            return Some(cert.to_string());
        }
    }
    None
}

fn pick_tv_certification(block: &ContentRatingsBlock, regions: &[String]) -> Option<String> {
    for region in regions {
        let found = block
            .results
            .iter()
            .find(|entry| entry.iso_3166_1.eq_ignore_ascii_case(region))
            .map(|entry| entry.rating.trim())
            .filter(|r| !r.is_empty());
        if let Some(cert) = found {
            return Some(cert.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions() -> Vec<String> {
        vec!["FR".into(), "US".into()]
    }

    #[test]
    fn a_v4_token_is_told_apart_from_a_v3_key() {
        // Sending a v4 token as a query parameter would write it into every
        // access log between here and TMDb.
        assert!(is_v4_token("eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ4In0.c2lnbmF0dXJl"));
        assert!(!is_v4_token("0123456789abcdef0123456789abcdef"));
        assert!(!is_v4_token(""));
        // Shaped like the start of a token but not one.
        assert!(!is_v4_token("eyJnotajwt"));
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

    #[test]
    fn picks_certification_in_region_preference_order() {
        let block = ReleaseDatesBlock {
            results: vec![
                ReleaseDatesEntry {
                    iso_3166_1: "US".into(),
                    release_dates: vec![ReleaseDate { certification: "PG".into() }],
                },
                ReleaseDatesEntry {
                    iso_3166_1: "FR".into(),
                    release_dates: vec![ReleaseDate { certification: "Tous publics".into() }],
                },
            ],
        };
        assert_eq!(pick_movie_certification(&block, &regions()).as_deref(), Some("Tous publics"));
        assert_eq!(pick_movie_certification(&block, &["US".to_string()]).as_deref(), Some("PG"));
    }

    #[test]
    fn skips_blank_certifications() {
        let block = ReleaseDatesBlock {
            results: vec![ReleaseDatesEntry {
                iso_3166_1: "FR".into(),
                release_dates: vec![
                    ReleaseDate { certification: "  ".into() },
                    ReleaseDate { certification: "12".into() },
                ],
            }],
        };
        assert_eq!(pick_movie_certification(&block, &regions()).as_deref(), Some("12"));
    }

    #[test]
    fn returns_none_when_no_region_matches() {
        let block = ReleaseDatesBlock {
            results: vec![ReleaseDatesEntry {
                iso_3166_1: "DE".into(),
                release_dates: vec![ReleaseDate { certification: "16".into() }],
            }],
        };
        assert_eq!(pick_movie_certification(&block, &regions()), None);
    }

    #[test]
    fn reads_tv_content_ratings() {
        let block = ContentRatingsBlock {
            results: vec![ContentRating { iso_3166_1: "US".into(), rating: "TV-14".into() }],
        };
        assert_eq!(pick_tv_certification(&block, &regions()).as_deref(), Some("TV-14"));
    }
}
