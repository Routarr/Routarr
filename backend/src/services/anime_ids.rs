//! The anime identifier table: the TMDB, TheTVDB and IMDb ids of a work beside its AniList and
//! MyAnimeList ids, read before any search so a title is found by what it is rather than by its
//! name and year. Downloaded or imported at a person's request and never shipped: no such table
//! is licensed for redistribution.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};
use crate::jobs::{Attribution, Detail, JobKind};
use crate::services::metadata::{self, ANILIST, MYANIMELIST, TENRAI};
use crate::state::AppState;

/// Fribb's anime-lists, the table arm-server and Kometa's sources build on: every entry maps
/// AniList, MyAnimeList, TheTVDB with its season, TMDB and IMDb.
pub const DEFAULT_URL: &str =
    "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-mini.json";

/// The largest table read: the default one weighs about 6 MB.
pub const MAX_BYTES: usize = 64 << 20;

/// The sources a mapped id is written for, each in the namespace it reads.
const SOURCES: [&str; 3] = [ANILIST, TENRAI, MYANIMELIST];

/// One work of the table. Unknown fields are ignored: the table is not Routarr's.
#[derive(Deserialize)]
pub struct Entry {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    anilist_id: Option<i64>,
    mal_id: Option<i64>,
    tvdb_id: Option<i64>,
    #[serde(default, rename = "themoviedb_id")]
    tmdb: Option<Tmdb>,
    #[serde(default)]
    imdb_id: OneOrMany<String>,
    season: Option<Season>,
}

#[derive(Deserialize, Default)]
struct Tmdb {
    #[serde(default)]
    movie: OneOrMany<i64>,
    tv: Option<i64>,
}

#[derive(Deserialize)]
struct Season {
    tvdb: Option<i64>,
    tmdb: Option<i64>,
}

/// A field the table writes as a value or as a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

/// One id of an entry, as stored.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct Row {
    media_type: String,
    id_kind: String,
    id_value: String,
    season: Option<i64>,
    entry_kind: String,
    anilist_id: Option<i64>,
    mal_id: Option<i64>,
}

/// The rows of a table, and how many of its entries name an AniList or MyAnimeList id.
fn rows_of(entries: Vec<Entry>) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    let mut counted = 0;
    for entry in entries {
        if entry.anilist_id.is_none() && entry.mal_id.is_none() {
            continue;
        }
        counted += 1;
        let kind = entry.kind.unwrap_or_default().to_uppercase();
        let row = |media_type: &str, id_kind: &str, id_value: String, season: Option<i64>| Row {
            media_type: media_type.to_string(),
            id_kind: id_kind.to_string(),
            id_value,
            season,
            entry_kind: kind.clone(),
            anilist_id: entry.anilist_id,
            mal_id: entry.mal_id,
        };
        let tmdb = entry.tmdb.unwrap_or_default();
        let season = entry.season.as_ref();
        for id in tmdb.movie.into_vec() {
            rows.push(row("movie", "tmdb", id.to_string(), None));
        }
        // A series' IMDb id is its whole run, every season sharing it: read for a film alone.
        if kind == "MOVIE" {
            for id in entry.imdb_id.into_vec() {
                rows.push(row("movie", "imdb", id, None));
            }
        }
        if let Some(id) = entry.tvdb_id {
            rows.push(row("series", "tvdb", id.to_string(), season.and_then(|s| s.tvdb)));
        }
        if let Some(id) = tmdb.tv {
            rows.push(row("series", "tmdb", id.to_string(), season.and_then(|s| s.tmdb)));
        }
    }
    (rows, counted)
}

/// A work's ids in the two namespaces the anime sources read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ids {
    pub anilist: Option<i64>,
    pub mal: Option<i64>,
}

impl Ids {
    /// The id `source` reads, as it stores ids.
    pub fn of(&self, source: &str) -> Option<String> {
        match source {
            ANILIST => self.anilist,
            TENRAI | MYANIMELIST => self.mal,
            _ => None,
        }
        .map(|id| id.to_string())
    }
}

/// The table, indexed by the ids a title carries.
#[derive(Default)]
pub struct Table {
    by_id: HashMap<(String, String, String), Vec<Row>>,
    refused: HashSet<(String, String)>,
}

impl Table {
    fn from_rows(rows: Vec<Row>, refused: HashSet<(String, String)>) -> Self {
        let mut by_id: HashMap<(String, String, String), Vec<Row>> = HashMap::new();
        for row in rows {
            let key = (row.media_type.clone(), row.id_kind.clone(), row.id_value.clone());
            by_id.entry(key).or_default().push(row);
        }
        Self { by_id, refused }
    }

    /// The whole table, for a pass over the library.
    pub async fn load(pool: &SqlitePool) -> AppResult<Self> {
        let rows = sqlx::query_as(
            "SELECT media_type, id_kind, id_value, season, entry_kind, anilist_id, mal_id
               FROM anime_ids",
        )
        .fetch_all(pool)
        .await?;
        Ok(Self::from_rows(rows, refused(pool).await?))
    }

    /// The rows one title's ids name, for a title asked about alone.
    pub async fn for_title(
        pool: &SqlitePool,
        media_type: &str,
        tmdb_id: Option<i64>,
        tvdb_id: Option<i64>,
        imdb_id: Option<&str>,
    ) -> AppResult<Self> {
        let rows = sqlx::query_as(
            "SELECT media_type, id_kind, id_value, season, entry_kind, anilist_id, mal_id
               FROM anime_ids
              WHERE media_type = ?
                AND ((id_kind = 'tmdb' AND id_value = ?) OR (id_kind = 'tvdb' AND id_value = ?)
                     OR (id_kind = 'imdb' AND id_value = ?))",
        )
        .bind(media_type)
        .bind(tmdb_id.map(|id| id.to_string()))
        .bind(tvdb_id.map(|id| id.to_string()))
        .bind(imdb_id)
        .fetch_all(pool)
        .await?;
        Ok(Self::from_rows(rows, refused(pool).await?))
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The work a title is, by its ids in the order they are trusted: a film by its TMDB id,
    /// then its IMDb id, a series by its TheTVDB id, then its TMDB id. `None` when the table
    /// holds none of them, or names two works for one.
    pub fn find(
        &self,
        media_type: &str,
        tmdb_id: Option<i64>,
        tvdb_id: Option<i64>,
        imdb_id: Option<&str>,
    ) -> Option<Ids> {
        let tries: Vec<(&str, String)> = match media_type {
            "movie" => {
                [("tmdb", tmdb_id.map(|id| id.to_string())), ("imdb", imdb_id.map(str::to_owned))]
                    .into_iter()
                    .filter_map(|(kind, id)| Some((kind, id?)))
                    .collect()
            }
            "series" => [("tvdb", tvdb_id), ("tmdb", tmdb_id)]
                .into_iter()
                .filter_map(|(kind, id)| Some((kind, id?.to_string())))
                .collect(),
            _ => Vec::new(),
        };
        tries.into_iter().find_map(|(kind, id)| {
            let rows = self.by_id.get(&(media_type.to_string(), kind.to_string(), id))?;
            pick(media_type, rows)
        })
    }

    /// Whether `source` refused `external_id` although the table named it.
    pub fn refused(&self, source: &str, external_id: &str) -> bool {
        self.refused.contains(&(source.to_string(), external_id.to_string()))
    }
}

/// The one work `rows` name for a title: a series takes its first season, the season it is
/// filed under, and a kind matching the title's breaks a tie. Two works left is no answer.
fn pick(media_type: &str, rows: &[Row]) -> Option<Ids> {
    let seasoned: Vec<&Row> = match media_type {
        "series" => {
            let first: Vec<&Row> = rows.iter().filter(|row| row.season == Some(1)).collect();
            if first.is_empty() {
                rows.iter().filter(|row| row.season.is_none()).collect()
            } else {
                first
            }
        }
        _ => rows.iter().collect(),
    };
    let distinct = |rows: &[&Row]| {
        let mut ids: Vec<Ids> =
            rows.iter().map(|row| Ids { anilist: row.anilist_id, mal: row.mal_id }).collect();
        ids.sort_by_key(|ids| (ids.anilist, ids.mal));
        ids.dedup();
        ids
    };
    let all = distinct(&seasoned);
    if all.len() == 1 {
        return all.first().copied();
    }
    let kind = if media_type == "series" { "TV" } else { "MOVIE" };
    let alike: Vec<&Row> = seasoned.into_iter().filter(|row| row.entry_kind == kind).collect();
    let alike = distinct(&alike);
    (alike.len() == 1).then(|| alike[0])
}

async fn refused(pool: &SqlitePool) -> AppResult<HashSet<(String, String)>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT source, external_id FROM anime_ids_refused").fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

/// The table's state, for the Metadata sources screen.
#[derive(Debug, Serialize)]
pub struct AnimeIdsState {
    /// When it was loaded, `None` before a first load.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    pub loaded_at: Option<String>,
    pub entries: Option<i64>,
    /// The address it was downloaded from, or `file`.
    pub origin: Option<String>,
    /// Where a download asks.
    pub address: String,
}

pub async fn state(state: &AppState) -> AppResult<AnimeIdsState> {
    let row: Option<(String, i64, String)> =
        sqlx::query_as("SELECT loaded_at, entries, origin FROM anime_ids_state WHERE id = 1")
            .fetch_optional(&state.pool)
            .await?;
    let (loaded_at, entries, origin) = match row {
        Some((at, entries, origin)) => (Some(at), Some(entries), Some(origin)),
        None => (None, None, None),
    };
    Ok(AnimeIdsState { loaded_at, entries, origin, address: state.config.anime_ids_url.clone() })
}

/// Whether the weekly download is due: the table is a week old or was never loaded, and no load
/// was tried in the last day, so a download that keeps failing is tried once a day at most.
pub async fn weekly_due(pool: &SqlitePool) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM anime_ids_state
                             WHERE loaded_at > datetime('now', '-7 days'))
            AND NOT EXISTS (SELECT 1 FROM jobs
                             WHERE kind = 'anime_ids' AND started_at > datetime('now', '-1 day'))",
    )
    .fetch_one(pool)
    .await?)
}

/// What a load did.
#[derive(Debug, Serialize, Default)]
pub struct AnimeIdsLoaded {
    /// Entries naming an AniList or MyAnimeList id.
    pub entries: usize,
    /// Titles of the library now found by their identifiers whose id changed or was new.
    pub titles: usize,
}

/// Where a table comes from: the configured address, or a file a person handed in.
pub enum Origin {
    Address,
    File(Vec<Entry>),
}

/// The entries of a file a person handed in, or the refusal they read under its field.
pub async fn read_file(state: &AppState, body: &[u8]) -> AppResult<Vec<Entry>> {
    match serde_json::from_slice(body) {
        Ok(entries) => Ok(entries),
        Err(_) => {
            let message = state.localizer().await.translate("AnimeIdsUnreadable", &[]);
            Err(AppError::BadRequest(message))
        }
    }
}

/// Loads a table as a task, one at a time: downloaded from the configured address because
/// somebody asked, or read from a file already parsed.
pub async fn load(state: &AppState, by: &Attribution, origin: Origin) -> AppResult<AnimeIdsLoaded> {
    let Some(_lock) = state.jobs.try_lock("anime_ids") else {
        let message = state.localizer().await.translate("ErrorAnimeIdsInProgress", &[]);
        return Err(AppError::InProgress { reason: "anime_ids_running", message });
    };
    let address = state.config.anime_ids_url.clone();
    let detail = match &origin {
        Origin::Address => Detail::new("JobDetailDownloadingAnimeIds").with("address", &address),
        Origin::File(_) => Detail::new("JobDetailImportingAnimeIds"),
    };
    let mut job = state.jobs.start(JobKind::AnimeIds, by, None, detail).await?;
    let loaded = match origin {
        Origin::Address => {
            let request = state.http.get(&address).timeout(state.config.library_timeout);
            match crate::integrations::send_json_capped("Anime identifiers", request, MAX_BYTES)
                .await
            {
                Ok(entries) => store(state, entries, &address).await,
                Err(e) => Err(e),
            }
        }
        Origin::File(entries) => store(state, entries, "file").await,
    };
    match loaded {
        Ok(loaded) => {
            job.report(&loaded);
            let done = Detail::new("JobDetailAnimeIdsLoaded")
                .with("count", loaded.entries)
                .with("changed", loaded.titles);
            job.succeed(done).await;
            Ok(loaded)
        }
        Err(e) => {
            job.fail(&e).await;
            Err(e)
        }
    }
}

/// Replaces the stored table with `entries` in one transaction, then gives every title the ids
/// it names. A table naming no work is refused, the stored one left as it was.
async fn store(state: &AppState, entries: Vec<Entry>, origin: &str) -> AppResult<AnimeIdsLoaded> {
    let (rows, counted) = rows_of(entries);
    if rows.is_empty() {
        let message = state.localizer().await.translate("AnimeIdsEmpty", &[]);
        return Err(AppError::BadRequest(message));
    }
    let mut tx = crate::db::write_transaction(&state.pool).await?;
    sqlx::query("DELETE FROM anime_ids").execute(&mut *tx).await?;
    for row in &rows {
        sqlx::query(
            "INSERT INTO anime_ids
                    (media_type, id_kind, id_value, season, entry_kind, anilist_id, mal_id)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.media_type)
        .bind(&row.id_kind)
        .bind(&row.id_value)
        .bind(row.season)
        .bind(&row.entry_kind)
        .bind(row.anilist_id)
        .bind(row.mal_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO anime_ids_state (id, loaded_at, entries, origin)
         VALUES (1, datetime('now'), ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            loaded_at = excluded.loaded_at, entries = excluded.entries, origin = excluded.origin",
    )
    .bind(counted as i64)
    .bind(origin)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let titles = apply(&state.pool).await?;
    Ok(AnimeIdsLoaded { entries: counted, titles })
}

/// The columns of a library title the table is read by.
#[derive(sqlx::FromRow)]
struct Title {
    media_type: String,
    title: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    year: Option<i64>,
}

/// Writes for every title of the library the ids the table names, for each anime source, over
/// what a search or the other source found: the table is what a work is, a search a guess.
/// Answers how many identifiers changed.
pub async fn apply(pool: &SqlitePool) -> AppResult<usize> {
    let table = Table::load(pool).await?;
    if table.is_empty() {
        return Ok(0);
    }
    let titles: Vec<Title> = sqlx::query_as(
        "SELECT DISTINCT media_type, title, tmdb_id, tvdb_id, imdb_id, year FROM media",
    )
    .fetch_all(pool)
    .await?;
    let mut changed = 0;
    let mut tx = crate::db::write_transaction(pool).await?;
    for Title { media_type, title, tmdb_id, tvdb_id, imdb_id, year } in titles {
        let Some(ids) = table.find(&media_type, tmdb_id, tvdb_id, imdb_id.as_deref()) else {
            continue;
        };
        let key = metadata::local_key_of(tmdb_id, tvdb_id, imdb_id.as_deref(), &title, year);
        for source in SOURCES {
            let Some(external) = ids.of(source).filter(|id| !table.refused(source, id)) else {
                continue;
            };
            changed += sqlx::query(
                "INSERT INTO source_identifiers
                        (source, media_type, local_key, external_id, resolved_at, found_by)
                 VALUES (?, ?, ?, ?, datetime('now'), 'mapping')
                 ON CONFLICT(source, media_type, local_key) DO UPDATE SET
                    external_id = excluded.external_id, resolved_at = excluded.resolved_at,
                    found_by = 'mapping'
                  WHERE source_identifiers.external_id IS NOT excluded.external_id
                     OR source_identifiers.found_by <> 'mapping'",
            )
            .bind(source)
            .bind(&media_type)
            .bind(&key)
            .bind(&external)
            .execute(&mut *tx)
            .await?
            .rows_affected() as usize;
        }
    }
    tx.commit().await?;
    Ok(changed)
}

/// `source` refused an id the table named: it is never read from the table again, and the
/// titles holding it are searched at the next pass.
pub async fn refuse(pool: &SqlitePool, source: &str, external_id: &str) -> AppResult<()> {
    let mut tx = crate::db::write_transaction(pool).await?;
    // Set aside only what the table named: an id a search found is the search's to forget.
    let mapped = sqlx::query(
        "DELETE FROM source_identifiers
          WHERE source = ? AND external_id = ? AND found_by = 'mapping'",
    )
    .bind(source)
    .bind(external_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if mapped > 0 {
        sqlx::query("INSERT OR IGNORE INTO anime_ids_refused (source, external_id) VALUES (?, ?)")
            .bind(source)
            .bind(external_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two entries of Fribb's table as it reads: the first season of a series, its second, its
    /// special, and a film with two TMDB ids.
    const SAMPLE: &str = r#"[
      {"type":"TV","anidb_id":1,"anilist_id":290,"mal_id":290,"imdb_id":["tt0286390"],
       "themoviedb_id":{"tv":26209},"tvdb_id":72025,"season":{"tvdb":1,"tmdb":1}},
      {"type":"TV","anidb_id":4,"anilist_id":396,"mal_id":396,"imdb_id":["tt0286390"],
       "themoviedb_id":{"tv":26209},"tvdb_id":72025,"season":{"tvdb":2,"tmdb":2}},
      {"type":"SPECIAL","anidb_id":6,"anilist_id":1124,"mal_id":1124,"imdb_id":["tt0286390"],
       "themoviedb_id":{"tv":26209},"tvdb_id":72025,"season":{"tvdb":0,"tmdb":0}},
      {"type":"MOVIE","anidb_id":7,"anilist_id":164,"mal_id":164,"imdb_id":"tt0119698",
       "themoviedb_id":{"movie":[128,999]}},
      {"type":"OVA","anidb_id":9,"kitsu_id":5}
    ]"#;

    fn table() -> Table {
        let entries: Vec<Entry> = serde_json::from_str(SAMPLE).unwrap();
        let (rows, counted) = rows_of(entries);
        assert_eq!(counted, 4, "an entry naming neither id is no work");
        Table::from_rows(rows, HashSet::new())
    }

    const CREST: Ids = Ids { anilist: Some(290), mal: Some(290) };
    const MONONOKE: Ids = Ids { anilist: Some(164), mal: Some(164) };

    #[test]
    fn a_series_is_its_first_season_whichever_id_names_it() {
        let table = table();
        assert_eq!(table.find("series", None, Some(72025), None), Some(CREST));
        assert_eq!(table.find("series", Some(26209), None, None), Some(CREST));
    }

    #[test]
    fn a_film_is_found_by_any_of_its_tmdb_ids_or_its_imdb_id() {
        let table = table();
        assert_eq!(table.find("movie", Some(999), None, None), Some(MONONOKE));
        assert_eq!(table.find("movie", None, None, Some("tt0119698")), Some(MONONOKE));
    }

    /// TMDB numbers films and series apart, and a series' IMDb id names every season of it.
    #[test]
    fn an_id_is_read_for_its_own_kind_of_title_only() {
        let table = table();
        assert_eq!(table.find("series", Some(128), None, None), None);
        assert_eq!(table.find("movie", Some(26209), None, None), None);
        assert_eq!(table.find("movie", None, None, Some("tt0286390")), None);
    }

    #[test]
    fn two_works_for_one_id_are_no_answer() {
        let rows = vec![
            Row {
                media_type: "movie".into(),
                id_kind: "tmdb".into(),
                id_value: "5".into(),
                season: None,
                entry_kind: "MOVIE".into(),
                anilist_id: Some(1),
                mal_id: Some(1),
            },
            Row {
                media_type: "movie".into(),
                id_kind: "tmdb".into(),
                id_value: "5".into(),
                season: None,
                entry_kind: "MOVIE".into(),
                anilist_id: Some(2),
                mal_id: Some(2),
            },
        ];
        assert_eq!(Table::from_rows(rows, HashSet::new()).find("movie", Some(5), None, None), None);
    }

    #[test]
    fn each_source_reads_its_own_namespace() {
        let ids = Ids { anilist: Some(21), mal: Some(20) };
        assert_eq!(ids.of(ANILIST).as_deref(), Some("21"));
        assert_eq!(ids.of(TENRAI).as_deref(), Some("20"));
        assert_eq!(ids.of(MYANIMELIST).as_deref(), Some("20"));
        assert_eq!(ids.of(metadata::TMDB), None);
    }
}
