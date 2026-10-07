//! TheTVDB v4.
//!
//! The source Sonarr itself is built on, and the only one in the set that
//! authenticates with a **login** rather than a query parameter: `POST /login`
//! returns a bearer token valid for about a month. The token is fetched on first
//! use and kept in `AppState`, so one login serves every enrichment pass until
//! it expires, not one per item.
//!
//! Its key is free. A *user-supported* key additionally needs the subscriber PIN
//! its owner was given, which is why the PIN is a separate optional setting
//! rather than being folded into the key.

use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

use super::{language, send_json};
use crate::error::AppResult;

const SERVICE: &str = "TheTVDB";
pub const DEFAULT_BASE_URL: &str = "https://api4.thetvdb.com/v4";

/// The token of the last login, with the key it was obtained with: a token
/// answers for that key only, so a new key logs in again.
#[derive(Debug)]
pub struct CachedToken {
    key: String,
    token: String,
}

/// Shared by every client a state builds, so one login serves every pass.
pub type TokenCache = Arc<Mutex<Option<CachedToken>>>;

#[derive(Debug, Clone)]
pub struct TvdbClient {
    client: Client,
    api_key: String,
    /// Only user-supported keys need one.
    pin: Option<String>,
    base_url: String,
    /// Owned by `AppState`, so the token outlives the client and one login
    /// serves every pass until it expires.
    token: TokenCache,
}

#[derive(Debug, Clone, Default)]
pub struct TvdbDetails {
    pub genres: Vec<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    /// Every country's rating, by upper-case country code.
    pub certifications: BTreeMap<String, String>,
    pub status: Option<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    token: String,
}

#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(default)]
    genres: Vec<Named>,
    #[serde(rename = "originalCountry")]
    original_country: Option<String>,
    #[serde(rename = "originalLanguage")]
    original_language: Option<String>,
    #[serde(rename = "contentRatings", default)]
    content_ratings: Vec<ContentRating>,
    status: Option<Named>,
    overview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentRating {
    name: Option<String>,
    /// Alpha-3, like everything else TheTVDB calls a country.
    country: Option<String>,
}

impl TvdbClient {
    pub fn new(
        client: Client,
        api_key: &str,
        pin: Option<&str>,
        base_url: &str,
        token: TokenCache,
    ) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            pin: pin.map(str::to_string),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// The bearer token, logging in the first time it is needed.
    ///
    /// The lock is held across the login on purpose: two concurrent enrichment
    /// futures would otherwise both log in, and TheTVDB counts that.
    async fn token(&self) -> AppResult<String> {
        let mut guard = self.token.lock().await;
        if let Some(cached) = guard.as_ref().filter(|cached| cached.key == self.api_key) {
            return Ok(cached.token.clone());
        }

        let mut body = serde_json::json!({ "apikey": self.api_key });
        if let Some(pin) = &self.pin {
            body["pin"] = serde_json::Value::String(pin.clone());
        }

        let response: Envelope<LoginData> =
            send_json(SERVICE, self.client.post(format!("{}/login", self.base_url)).json(&body))
                .await?;

        // A login that answers without a token is a refusal in the shape of a
        // success. Cached, it would send an empty bearer with every read.
        let token = response.data.map(|data| data.token).filter(|token| !token.is_empty());
        let Some(token) = token else {
            return Err(crate::error::AppError::ExternalApi {
                service: SERVICE.to_string(),
                status: 401,
                message: "the login answered without a token".to_string(),
                retry_after: None,
            });
        };
        *guard = Some(CachedToken { key: self.api_key.clone(), token: token.clone() });
        Ok(token)
    }

    async fn get(&self, path: &str) -> AppResult<reqwest::RequestBuilder> {
        let token = self.token().await?;
        Ok(self.client.get(format!("{}{path}", self.base_url)).bearer_auth(token))
    }

    /// A read, logged in again once if the token has expired.
    ///
    /// TheTVDB documents a month's validity and the token lives for the life
    /// of the process, so past the month every read would answer 401, one per
    /// pending title per pass, with nothing naming the cause. A 401 drops the
    /// cached token and the read is made once more with a fresh one. A second
    /// 401 is the key's problem, and is reported as such.
    async fn read<T: serde::de::DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        match send_json(SERVICE, self.get(path).await?).await {
            Err(crate::error::AppError::ExternalApi { status: 401, .. }) => {
                self.token.lock().await.take();
                send_json(SERVICE, self.get(path).await?).await
            }
            other => other,
        }
    }

    /// A light read through the token, logging in if it has to. A cached token
    /// cannot tell a revoked key or a source that stopped answering from one
    /// that works, and a read can.
    pub async fn test_connection(&self) -> AppResult<bool> {
        match self.read::<serde_json::Value>("/genres").await {
            Ok(_) => Ok(true),
            Err(crate::error::AppError::ExternalApi { status, .. })
                if status == 401 || status == 403 =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn get_details(&self, tvdb_id: &str, media_type: &str) -> AppResult<TvdbDetails> {
        debug!("Fetching TheTVDB {media_type} {tvdb_id}");
        let collection = if media_type == "movie" { "movies" } else { "series" };
        let response: Envelope<RawRecord> =
            self.read(&format!("/{collection}/{tvdb_id}/extended")).await?;
        let Some(raw) = response.data else {
            return Ok(TvdbDetails::default());
        };

        Ok(TvdbDetails {
            genres: raw.genres.into_iter().filter_map(|entry| entry.name).collect(),
            original_language: raw.original_language.as_deref().and_then(language::from_iso_639_3),
            origin_countries: raw
                .original_country
                .as_deref()
                .and_then(language::country_from_alpha3)
                .into_iter()
                .collect(),
            certifications: ratings_by_country(&raw.content_ratings),
            status: raw.status.and_then(|entry| entry.name),
            overview: raw.overview,
        })
    }
}

/// Each country's rating, by its two-letter code, the first of each kept. A
/// rating for a country no two-letter code names, or a blank one, is dropped.
fn ratings_by_country(ratings: &[ContentRating]) -> BTreeMap<String, String> {
    let mut by_country = BTreeMap::new();
    for rating in ratings {
        let country = rating.country.as_deref().and_then(language::country_from_alpha3);
        let name = rating.name.as_deref().map(str::trim).filter(|name| !name.is_empty());
        if let (Some(country), Some(name)) = (country, name) {
            by_country.entry(country).or_insert_with(|| name.to_string());
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

    /// A blank rating, or one for a code no country has, rates nothing, and
    /// a country rated twice keeps its first.
    #[test]
    fn each_country_keeps_its_first_rating() {
        let rating = |name: &str, country: &str| ContentRating {
            name: Some(name.into()),
            country: Some(country.into()),
        };
        let ratings = [
            rating(" ", "usa"),
            rating("TV-14", "usa"),
            rating("-12", "fra"),
            rating("-16", "fra"),
            rating("12", "xxx"),
            ContentRating { name: None, country: Some("deu".into()) },
        ];
        assert_eq!(ratings_by_country(&ratings), by_country(&[("FR", "-12"), ("US", "TV-14")]));
    }

    /// A captured-shape TheTVDB v4 `/series/{id}/extended` response through the
    /// real types. The v4 API wraps everything in `data`, names country and
    /// language in three letters, and returns dozens of fields the client does
    /// not read. This pins down all three.
    #[test]
    fn a_real_shape_extended_record_deserialises_and_normalises() {
        let json = r#"{
          "status": "success",
          "data": {
            "id": 76885,
            "name": "Cowboy Bebop",
            "slug": "cowboy-bebop",
            "originalCountry": "jpn",
            "originalLanguage": "jpn",
            "score": 12345,
            "status": { "id": 2, "name": "Ended", "recordType": "series" },
            "overview": "A ragtag crew of bounty hunters.",
            "genres": [
              { "id": 1, "name": "Animation", "slug": "animation" },
              { "id": 2, "name": "Anime", "slug": "anime" }
            ],
            "contentRatings": [
              { "id": 1, "name": "TV-14", "country": "usa", "fullname": null },
              { "id": 2, "name": "-12", "country": "fra", "fullname": null }
            ]
          }
        }"#;

        let response: Envelope<RawRecord> = serde_json::from_str(json).unwrap();
        let raw = response.data.unwrap();

        assert_eq!(raw.genres.len(), 2);
        // Three-letter codes, which `language` shortens (tested there and end to
        // end).
        assert_eq!(raw.original_language.as_deref(), Some("jpn"));
        assert_eq!(raw.original_country.as_deref(), Some("jpn"));
        assert_eq!(raw.status.and_then(|s| s.name).as_deref(), Some("Ended"));
        assert_eq!(
            ratings_by_country(&raw.content_ratings),
            by_country(&[("FR", "-12"), ("US", "TV-14")])
        );
    }

    /// The login response, which is the only reason this client differs from
    /// the others: the token lives one level down under `data`.
    #[test]
    fn a_real_shape_login_response_yields_the_token() {
        let json = r#"{ "status": "success", "data": { "token": "eyJhbGciOi.stub.token" } }"#;
        let response: Envelope<LoginData> = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.unwrap().token, "eyJhbGciOi.stub.token");
    }
}
