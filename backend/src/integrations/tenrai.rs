//! Tenrai, an unofficial and credential-free MyAnimeList API. Its themes and demographics
//! ("Shounen", "Iyashikei") are what anime libraries are organised by. It limits requests per
//! address, so a 429 backs the whole pass off rather than retrying.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::send_json;
use crate::error::{AppError, AppResult};

const SERVICE: &str = "Tenrai";
pub const DEFAULT_BASE_URL: &str = "https://api.tenrai.org/v1";

#[derive(Debug, Clone)]
pub struct TenraiClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct TenraiCandidate {
    pub id: i64,
    pub year: Option<i64>,
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TenraiDetails {
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    /// MyAnimeList's score, out of 10, with its votes.
    pub ratings: std::collections::BTreeMap<String, crate::models::Rating>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct RawAnime {
    mal_id: i64,
    /// The broadcast season's year, null for a film: read through [`RawAnime::released`].
    year: Option<i64>,
    aired: Option<Aired>,
    title: Option<String>,
    title_english: Option<String>,
    title_japanese: Option<String>,
    #[serde(default)]
    titles: Vec<RawTitle>,
    #[serde(default)]
    genres: Vec<Named>,
    /// Empty on Tenrai, which lists Ecchi, Hentai and Erotica under `genres`, and read for a
    /// Jikan mirror that fills it.
    #[serde(default)]
    explicit_genres: Vec<Named>,
    #[serde(default)]
    themes: Vec<Named>,
    #[serde(default)]
    demographics: Vec<Named>,
    /// "R - 17+ (violence & profanity)" and friends.
    rating: Option<String>,
    status: Option<String>,
    synopsis: Option<String>,
    score: Option<f64>,
    scored_by: Option<i64>,
}

impl RawAnime {
    fn ratings(&self) -> std::collections::BTreeMap<String, crate::models::Rating> {
        crate::models::rating("mal", self.score, self.scored_by).into_iter().collect()
    }
}

/// When an entry first aired or came out, a film included.
#[derive(Debug, Deserialize)]
struct Aired {
    prop: Option<AiredProp>,
}

#[derive(Debug, Deserialize)]
struct AiredProp {
    from: Option<AiredDate>,
}

#[derive(Debug, Deserialize)]
struct AiredDate {
    year: Option<i64>,
}

impl RawAnime {
    /// The year the work came out, which the library's own year is checked against.
    fn released(&self) -> Option<i64> {
        let aired = self.aired.as_ref().and_then(|aired| aired.prop.as_ref());
        aired.and_then(|prop| prop.from.as_ref()).and_then(|from| from.year).or(self.year)
    }
}

#[derive(Debug, Deserialize)]
struct RawTitle {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

impl TenraiClient {
    /// Where this client points, so a caller can tell the public API from a mirror or a proxy.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(client: Client, base_url: &str) -> Self {
        Self { client, base_url: base_url.trim_end_matches('/').to_string() }
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        let probe = self.client.get(format!("{}/anime/1", self.base_url));
        send_json::<serde_json::Value>(SERVICE, probe).await.map(|_| true).map_err(refusal)
    }

    pub async fn search(&self, title: &str, media_type: &str) -> AppResult<Vec<TenraiCandidate>> {
        debug!("Searching Tenrai for {title}");
        let mut request = self
            .client
            .get(format!("{}/anime", self.base_url))
            .query(&[("q", title), ("limit", "5")]);
        if media_type == "movie" {
            request = request.query(&[("type", "movie")]);
        }

        let response: Envelope<Vec<RawAnime>> =
            send_json(SERVICE, request).await.map_err(refusal)?;
        Ok(response.data.into_iter().map(candidate).collect())
    }

    pub async fn get_details(&self, id: i64) -> AppResult<TenraiDetails> {
        debug!("Fetching Tenrai {id}");
        let request = self.client.get(format!("{}/anime/{id}/full", self.base_url));
        let response: Envelope<RawAnime> = send_json(SERVICE, request).await.map_err(refusal)?;
        Ok(details(response.data))
    }
}

/// A 403 is Tenrai's anti-abuse system blocking the address, which "normally clears within 24
/// hours": read as a pace (429) that closes the day ([`crate::integrations::is_quota_spent`]), as
/// asking again before then only renews the block.
fn refusal(error: AppError) -> AppError {
    match error {
        AppError::ExternalApi { service, status: 403, message, retry_after, .. } => {
            AppError::ExternalApi {
                service,
                status: 429,
                kind: crate::error::Upstream::QuotaSpent,
                message,
                retry_after,
            }
        }
        other => other,
    }
}

fn details(raw: RawAnime) -> TenraiDetails {
    let ratings = raw.ratings();
    // Keywords, since as genres the themes and demographics would collide with TMDB's list.
    let mut keywords: Vec<String> = raw
        .themes
        .into_iter()
        .chain(raw.demographics)
        .map(|entry| entry.name.to_lowercase())
        .collect();
    keywords.sort();
    keywords.dedup();

    TenraiDetails {
        genres: raw.genres.into_iter().chain(raw.explicit_genres).map(|entry| entry.name).collect(),
        keywords,
        certification: raw.rating.map(|rating| shorten_rating(&rating)),
        status: raw.status,
        overview: raw.synopsis,
        ratings,
    }
}

/// `"R - 17+ (violence & profanity)"` -> `"R"`: a `certification_in` rule names a code, as the
/// other sources give theirs. The code ends at the first ` - `, which a code never holds.
fn shorten_rating(rating: &str) -> String {
    rating.split(" - ").next().unwrap_or(rating).trim().to_string()
}

fn candidate(raw: RawAnime) -> TenraiCandidate {
    TenraiCandidate { id: raw.mal_id, year: raw.released(), titles: titles_of(&raw) }
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

    /// A captured-shape `/anime/{id}/full` response: `year` is often null (the date lives in
    /// `aired`), and the deprecated `title_*` fields still come beside `titles[]`.
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
            "score": 8.24,
            "scored_by": 400000,
            "synopsis": "Two sisters move to the country.",
            "genres": [{ "mal_id": 2, "name": "Adventure" }],
            "themes": [{ "mal_id": 63, "name": "Iyashikei" }],
            "demographics": [{ "mal_id": 15, "name": "Kids" }]
          }
        }"#;

        let response: Envelope<RawAnime> = serde_json::from_str(json).unwrap();
        let raw = response.data;

        assert_eq!(raw.mal_id, 523);
        // A null year must not break the row.
        assert_eq!(raw.year, None);
        assert!(titles_of(&raw).contains(&"My Neighbor Totoro".to_string()));
        assert_eq!(raw.genres.len(), 1);
        assert_eq!(shorten_rating(raw.rating.as_deref().unwrap()), "G");
        assert_eq!(raw.themes[0].name, "Iyashikei");
        assert_eq!(raw.demographics[0].name, "Kids");
        let mal = raw.ratings()["mal"];
        assert_eq!((mal.value, mal.votes), (8.24, Some(400_000)));
    }

    /// An explicit genre is a genre, as Jikan files it apart and Tenrai and the official API
    /// among the others, so an adult category routes on any of them.
    #[test]
    fn an_explicit_genre_is_a_genre_beside_the_others() {
        let json = r#"{ "data": { "mal_id": 1, "genres": [{ "name": "Comedy" }],
            "explicit_genres": [{ "name": "Ecchi" }], "themes": [{ "name": "School" }],
            "demographics": [] } }"#;

        let found = details(serde_json::from_str::<Envelope<RawAnime>>(json).unwrap().data);

        assert_eq!(found.genres, ["Comedy", "Ecchi"]);
        assert_eq!(found.keywords, ["school"]);
    }

    /// A captured-shape `/anime?q=` page: the pagination beside the entries, a film dated by
    /// `aired` with a null `year`, and a series dated by both.
    #[test]
    fn a_real_shape_search_page_maps() {
        let json = r#"{
          "pagination": { "last_visible_page": 1, "has_next_page": false, "current_page": 1,
            "items": { "count": 2, "total": 2, "per_page": 5 } },
          "data": [
            { "mal_id": 523, "url": "https://myanimelist.net/anime/523/Tonari_no_Totoro",
              "images": { "jpg": { "image_url": "https://cdn.myanimelist.net/images/anime/4/75923.jpg",
                "small_image_url": null, "large_image_url": null }, "webp": { "image_url": null } },
              "trailer": { "youtube_id": null, "url": null, "embed_url": null },
              "approved": true,
              "titles": [{ "type": "Default", "title": "Tonari no Totoro" },
                { "type": "English", "title": "My Neighbor Totoro" }],
              "title": "Tonari no Totoro", "title_english": "My Neighbor Totoro",
              "title_japanese": "となりのトトロ", "title_synonyms": ["My Neighbour Totoro"],
              "type": "Movie", "source": "Original", "episodes": 1, "status": "Finished Airing",
              "airing": false,
              "aired": { "from": "1988-04-16T00:00:00+00:00", "to": null,
                "prop": { "from": { "day": 16, "month": 4, "year": 1988 },
                  "to": { "day": null, "month": null, "year": null } },
                "string": "Apr 16, 1988" },
              "duration": "1 hr 26 min", "rating": "G - All Ages", "score": 8.24,
              "scored_by": 400000, "rank": 300, "popularity": 200, "members": 700000,
              "favorites": 9000, "synopsis": "Two sisters move to the country.",
              "background": null, "season": null, "year": null, "broadcast": { "day": null,
                "time": null, "timezone": null, "string": null },
              "producers": [], "licensors": [], "studios": [{ "mal_id": 21, "type": "anime",
                "name": "Studio Ghibli", "url": "https://myanimelist.net/anime/producer/21" }],
              "genres": [{ "mal_id": 2, "type": "anime", "name": "Adventure", "url": "" }],
              "explicit_genres": [], "themes": [], "demographics": [] },
            { "mal_id": 457, "url": "https://myanimelist.net/anime/457/Mushishi",
              "titles": [{ "type": "Default", "title": "Mushishi" }],
              "title": "Mushishi", "title_english": "Mushi-Shi", "title_japanese": "蟲師",
              "type": "TV", "episodes": 26, "status": "Finished Airing",
              "aired": { "from": "2005-10-23T00:00:00+00:00", "to": "2006-06-19T00:00:00+00:00",
                "prop": { "from": { "day": 23, "month": 10, "year": 2005 },
                  "to": { "day": 19, "month": 6, "year": 2006 } } },
              "rating": "PG-13 - Teens 13 or older", "season": "fall", "year": 2005,
              "genres": [], "themes": [{ "mal_id": 63, "name": "Iyashikei" }], "demographics": [] }
          ]
        }"#;

        let page: Envelope<Vec<RawAnime>> = serde_json::from_str(json).unwrap();
        let found: Vec<TenraiCandidate> = page.data.into_iter().map(candidate).collect();

        let summary: Vec<(i64, Option<i64>)> = found.iter().map(|c| (c.id, c.year)).collect();
        assert_eq!(summary, [(523, Some(1988)), (457, Some(2005))]);
        assert!(found[0].titles.contains(&"My Neighbor Totoro".to_string()));
        assert!(found[1].titles.contains(&"Mushi-Shi".to_string()));
    }

    #[test]
    fn a_rating_keeps_only_the_code_a_rule_names() {
        for (written, code) in [
            ("G - All Ages", "G"),
            ("PG - Children", "PG"),
            ("PG-13 - Teens 13 or older", "PG-13"),
            ("R - 17+ (violence & profanity)", "R"),
            ("R+ - Mild Nudity", "R+"),
            ("Rx - Hentai", "Rx"),
        ] {
            assert_eq!(shorten_rating(written), code, "{written}");
        }
    }

    #[test]
    fn a_block_of_the_address_reads_as_a_pace_closed_for_the_day() {
        let refused = |status: u16| AppError::ExternalApi {
            service: SERVICE.into(),
            status,
            kind: crate::error::Upstream::Http,
            message: String::new(),
            retry_after: None,
        };
        let status = |error: &AppError| match error {
            AppError::ExternalApi { status, .. } => *status,
            other => panic!("{other:?}"),
        };
        let blocked = refusal(refused(403));
        assert_eq!(status(&blocked), 429);
        assert!(crate::integrations::is_quota_spent(&blocked), "{blocked:?}");
        assert_eq!(status(&refusal(refused(404))), 404);
        assert_eq!(status(&refusal(refused(429))), 429);
        assert!(!crate::integrations::is_quota_spent(&refusal(refused(429))), "a pace is no day");
    }
}
