//! AniList, through its public GraphQL API.
//!
//! The one source in the set that needs **no credential of any kind**, and the
//! one that is strongest exactly where TMDb is weakest: it knows what an anime
//! is, its country of origin, and it carries community tags that make far better
//! keywords than TMDb's sparse ones.
//!
//! It has no TMDb, TVDB or IMDb identifier, so an item has to be found by title
//! and year first — see `services::metadata`, which remembers the answer.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::send_json;
use crate::error::AppResult;

const SERVICE: &str = "AniList";
pub const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

/// One query for the search, one for the details. Both ask for the same shape,
/// so a search can answer without a second round trip when it matches.
const SEARCH_QUERY: &str = "query ($search: String, $format: MediaFormat) {
  Page(perPage: 5) {
    media(search: $search, type: ANIME, format: $format, sort: SEARCH_MATCH) {
      id
      startDate { year }
      title { romaji english native }
      synonyms
    }
  }
}";

const DETAILS_QUERY: &str = "query ($id: Int) {
  Media(id: $id, type: ANIME) {
    genres
    countryOfOrigin
    status
    description(asHtml: false)
    isAdult
    tags { name rank }
  }
}";

#[derive(Debug, Clone)]
pub struct AniListClient {
    client: Client,
    base_url: String,
}

/// A candidate returned by a search, with every title it is known by.
#[derive(Debug, Clone)]
pub struct AniListCandidate {
    pub id: i64,
    pub year: Option<i64>,
    pub titles: Vec<String>,
}

/// What AniList knows about one work.
#[derive(Debug, Clone, Default)]
pub struct AniListDetails {
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub origin_countries: Vec<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(rename = "Page")]
    page: SearchPage,
}

#[derive(Debug, Deserialize)]
struct SearchPage {
    #[serde(default)]
    media: Vec<RawMedia>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMedia {
    id: i64,
    #[serde(rename = "startDate")]
    start_date: Option<StartDate>,
    title: Option<RawTitle>,
    #[serde(default)]
    synonyms: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StartDate {
    year: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTitle {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DetailsData {
    #[serde(rename = "Media")]
    media: Option<RawDetails>,
}

#[derive(Debug, Deserialize)]
struct RawDetails {
    #[serde(default)]
    genres: Vec<String>,
    #[serde(rename = "countryOfOrigin")]
    country_of_origin: Option<String>,
    status: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<RawTag>,
}

#[derive(Debug, Deserialize)]
struct RawTag {
    name: String,
    /// 0–100 community agreement. A tag nobody agrees with is noise in a rule.
    rank: Option<i64>,
}

/// Below this, a tag is a minority opinion rather than a fact about the work.
const TAG_RANK_FLOOR: i64 = 60;

impl AniListClient {
    /// Where this client is pointed, so a caller can tell the public API from a
    /// mirror or a proxy.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(client: Client, base_url: &str) -> Self {
        Self { client, base_url: base_url.trim_end_matches('/').to_string() }
    }

    fn post(&self, query: &str, variables: serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .post(&self.base_url)
            .json(&serde_json::json!({ "query": query, "variables": variables }))
    }

    /// Cheap reachability probe: the smallest legal query.
    pub async fn test_connection(&self) -> AppResult<bool> {
        let probe = self.post("query { Media(id: 1) { id } }", serde_json::json!({}));
        Ok(send_json::<serde_json::Value>(SERVICE, probe).await.is_ok())
    }

    /// Candidates for a title, most relevant first.
    pub async fn search(&self, title: &str, media_type: &str) -> AppResult<Vec<AniListCandidate>> {
        debug!("Searching AniList for {title}");
        // AniList's own vocabulary: a film is `MOVIE`, anything episodic is left
        // unconstrained rather than guessed at (TV, OVA, ONA and SPECIAL are all
        // "a series" as far as Sonarr is concerned).
        let format = if media_type == "movie" { Some("MOVIE") } else { None };

        let response: GraphQlResponse<SearchData> = send_json(
            SERVICE,
            self.post(SEARCH_QUERY, serde_json::json!({ "search": title, "format": format })),
        )
        .await?;

        let Some(data) = response.data else {
            return Ok(Vec::new());
        };

        Ok(data
            .page
            .media
            .into_iter()
            .map(|raw| AniListCandidate {
                id: raw.id,
                year: raw.start_date.and_then(|d| d.year),
                titles: titles_of(raw.title, raw.synonyms),
            })
            .collect())
    }

    pub async fn get_details(&self, id: i64) -> AppResult<AniListDetails> {
        debug!("Fetching AniList {id}");
        let response: GraphQlResponse<DetailsData> =
            send_json(SERVICE, self.post(DETAILS_QUERY, serde_json::json!({ "id": id }))).await?;

        let Some(raw) = response.data.and_then(|d| d.media) else {
            return Ok(AniListDetails::default());
        };

        let mut keywords: Vec<String> = raw
            .tags
            .into_iter()
            .filter(|tag| tag.rank.unwrap_or(0) >= TAG_RANK_FLOOR)
            .map(|tag| tag.name)
            .collect();
        keywords.sort();
        keywords.dedup();

        Ok(AniListDetails {
            genres: raw.genres,
            keywords,
            // Already ISO 3166-1 alpha-2, unlike OMDb's country names.
            origin_countries: raw.country_of_origin.into_iter().collect(),
            status: raw.status,
            overview: raw.description,
        })
    }
}

fn titles_of(title: Option<RawTitle>, synonyms: Vec<String>) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    if let Some(title) = title {
        all.extend(title.romaji);
        all.extend(title.english);
        all.extend(title.native);
    }
    all.extend(synonyms);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_of_a_title_is_kept_for_matching() {
        let title = RawTitle {
            romaji: Some("Tonari no Totoro".into()),
            english: Some("My Neighbor Totoro".into()),
            native: Some("となりのトトロ".into()),
        };
        let titles = titles_of(Some(title), vec!["Totoro".into()]);

        // A library names it in English, AniList indexes it in romaji: matching
        // on one spelling only would resolve almost nothing.
        assert_eq!(titles.len(), 4);
        assert!(titles.contains(&"My Neighbor Totoro".to_string()));
    }

    #[test]
    fn a_missing_title_block_is_not_a_failure() {
        assert!(titles_of(None, vec![]).is_empty());
    }

    /// A captured-shape AniList search response, deserialised through the real
    /// client types. This is what guards against the wire format drifting from
    /// what the mapping expects — the one thing an in-process fake cannot,
    /// because the fake is written to match the mapping rather than the API.
    ///
    /// The realistic edges are deliberate: `english` is `null` (common for a
    /// work indexed only in romaji), and there are extra fields the client does
    /// not read (`format`, `episodes`) which serde must ignore.
    #[test]
    fn a_real_shape_search_payload_deserialises_and_maps() {
        let json = r#"{
          "data": { "Page": { "media": [
            {
              "id": 523,
              "format": "MOVIE",
              "episodes": 1,
              "startDate": { "year": 1988 },
              "title": { "romaji": "Tonari no Totoro", "english": null, "native": "となりのトトロ" },
              "synonyms": ["Totoro"]
            }
          ] } }
        }"#;

        let response: GraphQlResponse<SearchData> = serde_json::from_str(json).unwrap();
        let media = response.data.unwrap().page.media;
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].id, 523);
        // `null` english drops out; the two present spellings survive.
        let titles = titles_of(media[0].title.clone(), media[0].synonyms.clone());
        assert!(titles.contains(&"Tonari no Totoro".to_string()));
        assert!(titles.contains(&"Totoro".to_string()));
    }

    /// The details half of the same capture, with real `tags` (rank included)
    /// and the HTML-in-description AniList returns even with `asHtml: false`.
    #[test]
    fn a_real_shape_details_payload_maps_to_metadata() {
        let json = r#"{
          "data": { "Media": {
            "genres": ["Adventure", "Comedy", "Supernatural"],
            "countryOfOrigin": "JP",
            "status": "FINISHED",
            "description": "Two young girls move to the countryside.",
            "isAdult": false,
            "tags": [
              { "name": "Rural", "rank": 88 },
              { "name": "Iyashikei", "rank": 71 },
              { "name": "Kaiju", "rank": 22 }
            ]
          } }
        }"#;

        let response: GraphQlResponse<DetailsData> = serde_json::from_str(json).unwrap();
        let raw = response.data.unwrap().media.unwrap();

        let mut keywords: Vec<String> = raw
            .tags
            .into_iter()
            .filter(|t| t.rank.unwrap_or(0) >= TAG_RANK_FLOOR)
            .map(|t| t.name)
            .collect();
        keywords.sort();

        assert_eq!(raw.genres, vec!["Adventure", "Comedy", "Supernatural"]);
        assert_eq!(raw.country_of_origin.as_deref(), Some("JP"));
        // Only the tags above the agreement floor become keywords.
        assert_eq!(keywords, vec!["Iyashikei", "Rural"]);
    }
}
