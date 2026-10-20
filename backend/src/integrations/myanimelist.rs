//! MyAnimeList's official API, keyed on a client ID each installation makes for itself, since
//! MyAnimeList's agreement lets nobody share one. It answers what Tenrai does, in one `genres`
//! list whose families only the genre's id tells apart.

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::send_json;
use crate::error::{AppError, AppResult};

const SERVICE: &str = "MyAnimeList";
pub const DEFAULT_BASE_URL: &str = "https://api.myanimelist.net/v2";
/// The header the client ID travels in: public data needs no user's sign-in.
const CLIENT_HEADER: &str = "X-MAL-CLIENT-ID";

/// A search refuses a query under three characters and over sixty-four.
const SHORTEST_QUERY: usize = 3;
const LONGEST_QUERY: usize = 64;

/// The genre ids that are genres, explicit ones included, as MyAnimeList groups them on its
/// genre page. Every other id is a theme or a demographic, a keyword as on Tenrai, and so is an
/// id this table does not know yet, as most new ids are themes.
const GENRE_IDS: [i64; 21] =
    [1, 2, 4, 5, 7, 8, 9, 10, 12, 14, 22, 24, 26, 28, 30, 36, 37, 41, 46, 47, 49];

#[derive(Clone)]
pub struct MyAnimeListClient {
    client: Client,
    client_id: String,
    base_url: String,
}

// The client ID stays out: a client printed in a log line or a failed assertion would carry it.
impl std::fmt::Debug for MyAnimeListClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MyAnimeListClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct MyAnimeListCandidate {
    pub id: i64,
    pub year: Option<i64>,
    pub titles: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MyAnimeListDetails {
    pub genres: Vec<String>,
    pub keywords: Vec<String>,
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    /// MyAnimeList's score, out of 10, with its votes.
    pub ratings: std::collections::BTreeMap<String, crate::models::Rating>,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    data: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    node: RawAnime,
}

#[derive(Debug, Deserialize)]
struct RawAnime {
    id: i64,
    title: Option<String>,
    alternative_titles: Option<AlternativeTitles>,
    /// `2017-10-23`, `2017-10` or `2017`.
    start_date: Option<String>,
    media_type: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    /// `g`, `pg`, `pg_13`, `r`, `r+` or `rx`.
    rating: Option<String>,
    status: Option<String>,
    synopsis: Option<String>,
    /// Left out while too few have scored the title.
    mean: Option<f64>,
    num_scoring_users: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AlternativeTitles {
    #[serde(default)]
    synonyms: Vec<String>,
    en: Option<String>,
    ja: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Genre {
    id: i64,
    name: String,
}

impl MyAnimeListClient {
    /// Where this client points, so a caller can tell the public API from a mirror or a proxy.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn new(client: Client, client_id: &str, base_url: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self { client, client_id: client_id.to_string(), base_url }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base_url)).header(CLIENT_HEADER, &self.client_id)
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        let probe = self.get("/anime/1").query(&[("fields", "id")]);
        send_json::<serde_json::Value>(SERVICE, probe).await.map(|_| true).map_err(refusal)
    }

    /// The entries matching `title`, a film's among films alone, since the search takes no type.
    pub async fn search(
        &self,
        title: &str,
        media_type: &str,
    ) -> AppResult<Vec<MyAnimeListCandidate>> {
        let Some(query) = query_of(title) else { return Ok(Vec::new()) };
        debug!("Searching MyAnimeList for {title}");
        // Without `nsfw`, the search hides every entry not yet reviewed, many ordinary ones.
        let request = self.get("/anime").query(&[
            ("q", query.as_str()),
            ("limit", "10"),
            ("nsfw", "true"),
            ("fields", "alternative_titles,start_date,media_type"),
        ]);
        let page: Page = send_json(SERVICE, request).await.map_err(refusal)?;
        Ok(page
            .data
            .into_iter()
            .map(|entry| entry.node)
            .filter(|anime| media_type != "movie" || anime.media_type.as_deref() == Some("movie"))
            .map(candidate)
            .collect())
    }

    pub async fn get_details(&self, id: i64) -> AppResult<MyAnimeListDetails> {
        debug!("Fetching MyAnimeList {id}");
        let fields = "genres,rating,status,synopsis,mean,num_scoring_users";
        let request = self.get(&format!("/anime/{id}")).query(&[("fields", fields)]);
        let raw: RawAnime = send_json(SERVICE, request).await.map_err(refusal)?;
        Ok(details(raw))
    }
}

/// The query a search may send for `title`: cut to the longest MyAnimeList takes, and none for
/// a title shorter than the shortest, which every search would refuse.
fn query_of(title: &str) -> Option<String> {
    let title = title.trim();
    if title.chars().count() < SHORTEST_QUERY {
        return None;
    }
    Some(title.chars().take(LONGEST_QUERY).collect::<String>().trim_end().to_string())
}

/// MyAnimeList's refusals read as the other sources' are: a client ID it does not know is a
/// 400, read as the key refused (401) so no title is written down as a miss, and a 403 with the
/// ID sent is its front blocking the address, read as a pace to slow (429).
fn refusal(error: AppError) -> AppError {
    match error {
        AppError::ExternalApi { service, status: 400, kind, message, retry_after }
            if message.to_ascii_lowercase().contains("client id") =>
        {
            AppError::ExternalApi { service, status: 401, kind, message, retry_after }
        }
        AppError::ExternalApi { service, status: 403, kind, message, retry_after } => {
            AppError::ExternalApi { service, status: 429, kind, message, retry_after }
        }
        other => other,
    }
}

fn candidate(raw: RawAnime) -> MyAnimeListCandidate {
    let year = raw.start_date.as_deref().and_then(|date| date.get(..4)?.parse().ok());
    let mut titles: Vec<String> = raw.title.into_iter().collect();
    if let Some(alternative) = raw.alternative_titles {
        titles.extend(alternative.en.into_iter().chain(alternative.ja).filter(|t| !t.is_empty()));
        titles.extend(alternative.synonyms);
    }
    MyAnimeListCandidate { id: raw.id, year, titles }
}

fn details(raw: RawAnime) -> MyAnimeListDetails {
    let ratings =
        crate::models::rating("mal", raw.mean, raw.num_scoring_users).into_iter().collect();
    let (genres, others): (Vec<Genre>, Vec<Genre>) =
        raw.genres.into_iter().partition(|genre| GENRE_IDS.binary_search(&genre.id).is_ok());
    // Keywords, since as genres the themes and demographics would collide with TMDB's list.
    let mut keywords: Vec<String> =
        others.into_iter().map(|genre| genre.name.to_lowercase()).collect();
    keywords.sort();
    keywords.dedup();
    MyAnimeListDetails {
        genres: genres.into_iter().map(|genre| genre.name).collect(),
        keywords,
        certification: raw.rating.as_deref().and_then(rating_code),
        status: raw.status,
        overview: raw.synopsis,
        ratings,
    }
}

/// `pg_13` -> `PG-13`: the code Tenrai gives for the same rating, which a rule names.
fn rating_code(rating: &str) -> Option<String> {
    let code = match rating {
        "g" => "G",
        "pg" => "PG",
        "pg_13" => "PG-13",
        "r" => "R",
        "r+" => "R+",
        "rx" => "Rx",
        _ => return None,
    };
    Some(code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured-shape `/anime/{id}` answer: one `genres` list mixing a genre, a theme, a
    /// demographic and an explicit genre, and a rating as the API writes it.
    #[test]
    fn a_real_shape_details_answer_sorts_genres_from_themes_by_id() {
        let json = r#"{
          "id": 523, "title": "Tonari no Totoro",
          "main_picture": { "medium": "https://cdn.myanimelist.net/images/anime/4/75923.jpg" },
          "synopsis": "Two sisters move to the country.",
          "mean": 8.24, "num_scoring_users": 400000, "status": "finished_airing",
          "genres": [
            { "id": 2, "name": "Adventure" }, { "id": 63, "name": "Iyashikei" },
            { "id": 15, "name": "Kids" }, { "id": 9, "name": "Ecchi" },
            { "id": 99, "name": "A Theme Added Later" }
          ],
          "rating": "pg_13"
        }"#;

        let found = details(serde_json::from_str(json).unwrap());

        assert_eq!(found.genres, ["Adventure", "Ecchi"]);
        assert_eq!(found.keywords, ["a theme added later", "iyashikei", "kids"]);
        assert_eq!(found.certification.as_deref(), Some("PG-13"));
        assert_eq!(found.status.as_deref(), Some("finished_airing"));
        let mal = found.ratings["mal"];
        assert_eq!((mal.value, mal.votes), (8.24, Some(400_000)));
    }

    /// The table is sorted, or `binary_search` misses a genre and files it as a keyword.
    #[test]
    fn the_genre_table_is_sorted_and_without_repeats() {
        assert!(GENRE_IDS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// A captured-shape `/anime?q=` page: entries under `node`, a film dated by `start_date`,
    /// an empty English title left out.
    #[test]
    fn a_real_shape_search_page_maps() {
        let json = r#"{
          "data": [
            { "node": { "id": 523, "title": "Tonari no Totoro",
                "alternative_titles": { "synonyms": ["My Neighbour Totoro"],
                  "en": "My Neighbor Totoro", "ja": "となりのトトロ" },
                "start_date": "1988-04-16", "media_type": "movie" } },
            { "node": { "id": 457, "title": "Mushishi",
                "alternative_titles": { "synonyms": [], "en": "", "ja": "蟲師" },
                "start_date": "2005-10", "media_type": "tv" } }
          ],
          "paging": { "next": "https://api.myanimelist.net/v2/anime?offset=10&q=x" }
        }"#;

        let page: Page = serde_json::from_str(json).unwrap();
        let found: Vec<MyAnimeListCandidate> =
            page.data.into_iter().map(|entry| candidate(entry.node)).collect();

        let summary: Vec<(i64, Option<i64>)> = found.iter().map(|c| (c.id, c.year)).collect();
        assert_eq!(summary, [(523, Some(1988)), (457, Some(2005))]);
        assert!(found[0].titles.contains(&"My Neighbor Totoro".to_string()));
        assert!(found[0].titles.contains(&"My Neighbour Totoro".to_string()));
        assert!(!found[1].titles.iter().any(String::is_empty), "{:?}", found[1].titles);
    }

    #[test]
    fn a_rating_reads_as_the_code_tenrai_gives() {
        for (written, code) in
            [("g", "G"), ("pg", "PG"), ("pg_13", "PG-13"), ("r", "R"), ("r+", "R+"), ("rx", "Rx")]
        {
            assert_eq!(rating_code(written).as_deref(), Some(code), "{written}");
        }
        assert_eq!(rating_code("unknown"), None);
    }

    /// Under three characters no search is sent, and a long title is cut to what the API takes,
    /// counted in characters, never bytes.
    #[test]
    fn a_query_holds_to_what_the_search_takes() {
        assert_eq!(query_of("K"), None);
        assert_eq!(query_of("  Ao "), None);
        assert_eq!(query_of("Akira").as_deref(), Some("Akira"));
        let long = "あ".repeat(70);
        assert_eq!(query_of(&long).map(|q| q.chars().count()), Some(LONGEST_QUERY));
    }

    #[test]
    fn a_client_id_refused_reads_as_a_refused_key_and_a_block_as_a_pace() {
        let refused = |status: u16, message: &str| AppError::ExternalApi {
            service: SERVICE.into(),
            status,
            kind: crate::error::Upstream::Http,
            message: message.into(),
            retry_after: None,
        };
        let status = |error: AppError| match error {
            AppError::ExternalApi { status, .. } => status,
            other => panic!("{other:?}"),
        };
        assert_eq!(status(refusal(refused(400, "Invalid client id"))), 401);
        assert_eq!(status(refusal(refused(400, "invalid q"))), 400, "a bad query is a miss");
        assert_eq!(status(refusal(refused(403, ""))), 429);
        assert_eq!(status(refusal(refused(404, "not_found"))), 404);
    }
}
