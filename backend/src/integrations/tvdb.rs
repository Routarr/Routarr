//! TheTVDB v4.
//!
//! The source Sonarr itself is built on, and the only one in the set that
//! authenticates with a **login** rather than a query parameter: `POST /login`
//! returns a bearer token valid for about a month. The token is fetched on first
//! use and kept in the client, so a full enrichment pass costs one login, not
//! one per item.
//!
//! Its key is free; a *user-supported* key additionally needs the subscriber PIN
//! its owner was given, which is why the PIN is a separate optional setting
//! rather than being folded into the key.

use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

use super::{language, send_json};
use crate::error::AppResult;

const SERVICE: &str = "TheTVDB";
pub const DEFAULT_BASE_URL: &str = "https://api4.thetvdb.com/v4";

#[derive(Debug, Clone)]
pub struct TvdbClient {
    client: Client,
    api_key: String,
    /// Only user-supported keys need one.
    pin: Option<String>,
    base_url: String,
    /// Owned by `AppState`, so the token outlives the client and one login
    /// serves every pass until it expires.
    token: Arc<Mutex<Option<String>>>,
    /// Certification regions, most preferred first.
    regions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TvdbDetails {
    pub genres: Vec<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
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
        regions: &[String],
        token: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            client,
            api_key: api_key.to_string(),
            pin: pin.map(str::to_string),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            regions: if regions.is_empty() {
                vec!["US".to_string()]
            } else {
                regions.iter().map(|r| r.trim().to_uppercase()).collect()
            },
        }
    }

    /// The bearer token, logging in the first time it is needed.
    ///
    /// The lock is held across the login on purpose: two concurrent enrichment
    /// futures would otherwise both log in, and TheTVDB counts that.
    async fn token(&self) -> AppResult<String> {
        let mut guard = self.token.lock().await;
        if let Some(token) = guard.as_ref() {
            return Ok(token.clone());
        }

        let mut body = serde_json::json!({ "apikey": self.api_key });
        if let Some(pin) = &self.pin {
            body["pin"] = serde_json::Value::String(pin.clone());
        }

        let response: Envelope<LoginData> =
            send_json(SERVICE, self.client.post(format!("{}/login", self.base_url)).json(&body))
                .await?;

        // A login that answers without a token is a refusal in the shape of a
        // success; cached, it would send an empty bearer with every read.
        let token = response.data.map(|data| data.token).filter(|token| !token.is_empty());
        let Some(token) = token else {
            return Err(crate::error::AppError::ExternalApi {
                service: SERVICE.to_string(),
                status: 401,
                message: "the login answered without a token".to_string(),
                retry_after: None,
            });
        };
        *guard = Some(token.clone());
        Ok(token)
    }

    async fn get(&self, path: &str) -> AppResult<reqwest::RequestBuilder> {
        let token = self.token().await?;
        Ok(self.client.get(format!("{}{path}", self.base_url)).bearer_auth(token))
    }

    /// A read, logged in again once if the token has expired.
    ///
    /// TheTVDB documents a month's validity and the token lives for the life
    /// of the process, so past the month every read answered 401 — one per
    /// pending title per pass, with nothing naming the cause. A 401 drops the
    /// cached token and the read is made once more with a fresh one; a second
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

    /// Logging in *is* the probe: it is the only call that can tell a bad key
    /// from an unreachable host.
    pub async fn test_connection(&self) -> AppResult<bool> {
        match self.token().await {
            Ok(token) => Ok(!token.is_empty()),
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
            certification: pick_rating(&raw.content_ratings, &self.regions),
            status: raw.status.and_then(|entry| entry.name),
            overview: raw.overview,
        })
    }
}

/// The rating for the first configured region that has one.
fn pick_rating(ratings: &[ContentRating], regions: &[String]) -> Option<String> {
    for region in regions {
        let found = ratings.iter().find(|rating| {
            rating
                .country
                .as_deref()
                .and_then(language::country_from_alpha3)
                .is_some_and(|country| country == *region)
        });
        if let Some(name) = found.and_then(|rating| rating.name.clone()) {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratings() -> Vec<ContentRating> {
        vec![
            ContentRating { name: Some("TV-14".into()), country: Some("usa".into()) },
            ContentRating { name: Some("-12".into()), country: Some("fra".into()) },
        ]
    }

    #[test]
    fn the_rating_follows_the_configured_region_order() {
        assert_eq!(pick_rating(&ratings(), &["FR".into(), "US".into()]).as_deref(), Some("-12"));
        assert_eq!(pick_rating(&ratings(), &["US".into()]).as_deref(), Some("TV-14"));
    }

    /// A captured-shape TheTVDB v4 `/series/{id}/extended` response through the
    /// real types. The v4 API wraps everything in `data`, names country and
    /// language in three letters, and returns dozens of fields the client does
    /// not read — all three are what this pins down.
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
        // Three-letter codes in, the two-letter forms a rule is written against
        // out.
        assert_eq!(
            super::language::from_iso_639_3(raw.original_language.as_deref().unwrap()).as_deref(),
            Some("ja")
        );
        assert_eq!(
            super::language::country_from_alpha3(raw.original_country.as_deref().unwrap())
                .as_deref(),
            Some("JP")
        );
        assert_eq!(raw.status.and_then(|s| s.name).as_deref(), Some("Ended"));
        assert_eq!(pick_rating(&raw.content_ratings, &["FR".into()]).as_deref(), Some("-12"));
    }

    /// The login response, which is the only reason this client differs from
    /// the others: the token lives one level down under `data`.
    #[test]
    fn a_real_shape_login_response_yields_the_token() {
        let json = r#"{ "status": "success", "data": { "token": "eyJhbGciOi.stub.token" } }"#;
        let response: Envelope<LoginData> = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.unwrap().token, "eyJhbGciOi.stub.token");
    }

    #[test]
    fn a_region_nobody_rated_is_no_certification() {
        assert_eq!(pick_rating(&ratings(), &["DE".into()]), None);
    }
}
