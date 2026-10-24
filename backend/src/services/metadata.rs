//! The metadata sources, their priority order, and where each one gets its ids.
//!
//! `arr` answers from the payload the sync already reads: never fetched, paced or stale, and
//! keyless, which makes the others optional. A fetched source issues one request per item and is
//! cached in `metadata_cache` under its own id namespace.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::integrations::anilist::AniListClient;
use crate::integrations::myanimelist::MyAnimeListClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tenrai::TenraiClient;
use crate::integrations::tmdb::TmdbClient;
use crate::integrations::tvdb::TvdbClient;
use crate::models::{Media, MetadataField, ProviderMetadata};
use crate::services::rate_limit::RateLimiter;
use crate::services::rule_engine::normalise_value;

pub const ARR: &str = "arr";
pub const TMDB: &str = "tmdb";
pub const ANILIST: &str = "anilist";
pub const TENRAI: &str = "tenrai";
pub const OMDB: &str = "omdb";
pub const TVDB: &str = "tvdb";
/// MyAnimeList's official API, beside Tenrai's reading of the same site.
pub const MYANIMELIST: &str = "myanimelist";

/// The most days TMDB's terms let one of its answers be kept: six months.
pub const TMDB_CACHE_DAYS: i64 = 180;

/// How many days an answer of `source` is kept, from the lifetime `configured`:
/// TMDB's terms hold its own to [`TMDB_CACHE_DAYS`].
pub fn cache_days(source: &str, configured: i64) -> i64 {
    let configured = configured.max(1);
    if source == TMDB { configured.min(TMDB_CACHE_DAYS) } else { configured }
}

/// The order when the setting is missing or unreadable: the free source alone. Every other
/// source is opt-in, and a TMDB key in the environment lists TMDB at startup
/// (`maintenance::converge_metadata_sources`).
pub const DEFAULT_ORDER: &[&str] = &[ARR];

/// How a source is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    /// Answered from the `media` row, with no lookup, cache or request: what `arr` is.
    Local,
    /// The identifier is a column of `media`, spliced into SQL, so it stays a literal of this file.
    Column(&'static str),
    /// The source knows none of our identifiers, so the item has to be found by
    /// title and year and the answer remembered in `source_identifiers`.
    Search,
}

/// What a source is and what it can answer.
#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    pub id: &'static str,
    /// Shown as-is: a proper noun in every language, never translated.
    pub display_name: &'static str,
    /// False for `arr`, whose data arrives with the library sync.
    pub fetched: bool,
    /// Whether the user must supply a credential before it can answer.
    pub needs_key: bool,
    /// The environment variable the credential is read from, which the interface names so the
    /// user knows what to set. `None` for a source that authenticates nothing.
    pub key_env: Option<&'static str>,
    pub addressing: Addressing,
    pub fields: &'static [MetadataField],
    /// The score scales it rates on, for which `Ratings` among `fields` stands.
    pub scales: &'static [&'static str],
    /// The media types it answers for: TheTVDB for series alone, since Radarr
    /// carries no TheTVDB id.
    pub media_types: &'static [&'static str],
    /// The site its data comes from, which the interface credits and links
    /// to wherever that data is shown, as the sources' terms ask.
    pub website: Option<&'static str>,
}

const EVERY_MEDIA_TYPE: &[&str] = &["movie", "series"];

/// Every source this build knows about, in no particular order: priority is
/// the user's, held in the `metadata_providers` setting.
pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: ARR,
        display_name: "Radarr / Sonarr",
        fetched: false,
        needs_key: false,
        key_env: None,
        addressing: Addressing::Local,
        // No keywords and no origin country: the Arr APIs do not carry them.
        fields: &[
            MetadataField::Genres,
            MetadataField::OriginalLanguage,
            MetadataField::Certification,
            MetadataField::Ratings,
        ],
        scales: &["imdb", "tmdb", "metacritic", "rotten_tomatoes"],
        media_types: EVERY_MEDIA_TYPE,
        website: None,
    },
    ProviderInfo {
        id: TMDB,
        display_name: "TMDB",
        fetched: true,
        needs_key: true,
        key_env: Some("TMDB_API_KEY"),
        addressing: Addressing::Column("tmdb_id"),
        fields: &[
            MetadataField::Genres,
            MetadataField::Keywords,
            MetadataField::OriginalLanguage,
            MetadataField::OriginCountries,
            MetadataField::Certification,
            MetadataField::Ratings,
        ],
        scales: &["tmdb"],
        media_types: EVERY_MEDIA_TYPE,
        website: Some("https://www.themoviedb.org"),
    },
    ProviderInfo {
        id: ANILIST,
        display_name: "AniList",
        fetched: true,
        // Its public GraphQL API authenticates nothing.
        needs_key: false,
        key_env: None,
        addressing: Addressing::Search,
        // No certification: AniList's adult flag is no rating a `certification_in` rule names.
        fields: &[
            MetadataField::Genres,
            MetadataField::Keywords,
            MetadataField::OriginCountries,
            MetadataField::Ratings,
        ],
        scales: &["anilist"],
        media_types: EVERY_MEDIA_TYPE,
        website: Some("https://anilist.co"),
    },
    ProviderInfo {
        id: TENRAI,
        display_name: "Tenrai (MyAnimeList)",
        fetched: true,
        needs_key: false,
        key_env: None,
        addressing: Addressing::Search,
        fields: &[
            MetadataField::Genres,
            MetadataField::Keywords,
            MetadataField::Certification,
            MetadataField::Ratings,
        ],
        scales: &["mal"],
        media_types: EVERY_MEDIA_TYPE,
        website: Some("https://myanimelist.net"),
    },
    ProviderInfo {
        id: MYANIMELIST,
        display_name: "MyAnimeList",
        fetched: true,
        // A client ID each installation makes for itself: MyAnimeList lets nobody share one.
        needs_key: true,
        key_env: Some("MAL_CLIENT_ID"),
        addressing: Addressing::Search,
        fields: &[
            MetadataField::Genres,
            MetadataField::Keywords,
            MetadataField::Certification,
            MetadataField::Ratings,
        ],
        scales: &["mal"],
        media_types: EVERY_MEDIA_TYPE,
        website: Some("https://myanimelist.net"),
    },
    ProviderInfo {
        id: OMDB,
        display_name: "OMDb",
        fetched: true,
        // A free key, but a key: OMDb rejects an unauthenticated request.
        needs_key: true,
        key_env: Some("OMDB_API_KEY"),
        addressing: Addressing::Column("imdb_id"),
        fields: &[
            MetadataField::Genres,
            MetadataField::OriginalLanguage,
            MetadataField::OriginCountries,
            MetadataField::Certification,
            MetadataField::Ratings,
        ],
        scales: &["imdb", "metacritic", "rotten_tomatoes"],
        media_types: EVERY_MEDIA_TYPE,
        website: Some("https://www.omdbapi.com"),
    },
    ProviderInfo {
        id: TVDB,
        display_name: "TheTVDB",
        fetched: true,
        needs_key: true,
        key_env: Some("TVDB_API_KEY"),
        addressing: Addressing::Column("tvdb_id"),
        fields: &[
            MetadataField::Genres,
            MetadataField::OriginalLanguage,
            MetadataField::OriginCountries,
            MetadataField::Certification,
        ],
        scales: &[],
        media_types: &["series"],
        website: Some("https://thetvdb.com"),
    },
];

/// Whether a source can answer at all in this configuration. A listed source missing its
/// credential contributes nothing, and the sources below it answer instead.
pub fn is_usable(
    provider: &ProviderInfo,
    keys: &std::collections::HashMap<&'static str, String>,
) -> bool {
    // Resolved keys: a key saved in the interface counts as much as one from the environment.
    if provider.needs_key { keys.contains_key(provider.id) } else { true }
}

pub fn info(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// Parse the `metadata_providers` setting into an order. An unknown id is dropped, not refused,
/// so a database written by a newer build still routes here. A duplicate keeps its first place.
pub fn parse_order(raw: &str) -> Vec<&'static ProviderInfo> {
    let mut seen: Vec<&'static ProviderInfo> = Vec::new();
    for id in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(provider) = info(id)
            && !seen.iter().any(|p| p.id == provider.id)
        {
            seen.push(provider);
        }
    }
    // Last if unlisted: a value written around the validator must not leave no source at all.
    if !seen.iter().any(|p| p.id == "arr")
        && let Some(arr) = info("arr")
    {
        seen.push(arr);
    }
    seen
}

/// Metadata sources as the user ordered them, highest priority first. An absent setting is the
/// shipped default, and any stored value, empty included, still yields `arr` (`parse_order`).
pub fn configured_order(setting: Option<&str>) -> Vec<&'static ProviderInfo> {
    match setting {
        Some(value) => parse_order(value),
        None => parse_order(&DEFAULT_ORDER.join(",")),
    }
}

/// The fields at least one of these sources can answer.
pub fn covered_fields(providers: &[&'static ProviderInfo]) -> Vec<MetadataField> {
    let mut fields: Vec<MetadataField> = Vec::new();
    for provider in providers {
        for field in provider.fields {
            if !fields.contains(field) {
                fields.push(*field);
            }
        }
    }
    fields
}

/// The score scales the enabled sources give between them.
pub fn covered_scales(providers: &[&'static ProviderInfo]) -> Vec<&'static str> {
    let mut scales: Vec<&'static str> = Vec::new();
    for scale in providers.iter().flat_map(|provider| provider.scales) {
        if !scales.contains(scale) {
            scales.push(scale);
        }
    }
    scales
}

/// The `arr` source, read from the row. A malformed `genres` payload yields no genres rather
/// than an error: one signal must not break an evaluation.
pub fn from_media(media: &Media) -> ProviderMetadata {
    ProviderMetadata {
        genres: media.genre_list(),
        keywords: Vec::new(),
        original_language: media.original_language.clone(),
        origin_countries: Vec::new(),
        certification: media.certification.clone(),
        // The instance's, which the row does not hold: set by `routing::resolve_metadata`.
        certification_scale: None,
        certifications: Default::default(),
        // `status` is a `media` column the engine reads, and no Arr payload carries `overview`.
        status: None,
        overview: None,
        ratings: media.rating_map(),
    }
}

/// The one pace every request to a source waits on, the enrichment's, `GET /route`'s and the
/// health probe's alike: each at the full rate, they would earn the 429s that stop a pass. Held
/// by the state rather than the process, so two states never slow each other.
#[derive(Debug, Clone, Default)]
pub struct Paces(Arc<std::sync::Mutex<HashMap<&'static str, RateLimiter>>>);

impl Paces {
    /// The limiter of `source`: at its published rate on its public endpoint,
    /// unlimited elsewhere, and held back alike when the source refuses.
    pub fn of(&self, source: &FetchingSource) -> RateLimiter {
        let mut paces = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pace = paces.entry(source.id()).or_insert_with(|| match source.rate() {
            Some((per_minute, burst)) => RateLimiter::new(per_minute, burst),
            None => RateLimiter::unlimited(),
        });
        pace.clone()
    }
}

/// A source asked over the network. The compiler finds a new variant's exhaustive matches, not
/// the arms that fall through: `resolve` for a `Search` source, `rate` for a paced public
/// endpoint, `AppState::metadata_sources`, and for a keyed source `provider_key_from` and
/// `provider_keys_from`.
#[derive(Debug, Clone)]
pub enum FetchingSource {
    Tmdb(TmdbClient),
    AniList(AniListClient),
    Tenrai(TenraiClient),
    MyAnimeList(MyAnimeListClient),
    Omdb(OmdbClient),
    Tvdb(TvdbClient),
}

impl FetchingSource {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Tmdb(_) => TMDB,
            Self::AniList(_) => ANILIST,
            Self::Tenrai(_) => TENRAI,
            Self::MyAnimeList(_) => MYANIMELIST,
            Self::Omdb(_) => OMDB,
            Self::Tvdb(_) => TVDB,
        }
    }

    pub fn addressing(&self) -> Addressing {
        info(self.id()).map(|provider| provider.addressing).unwrap_or(Addressing::Local)
    }

    /// The sustained ceiling, in requests per minute, and the burst an idle bucket holds.
    /// Concurrency bounds the requests open, this the requests made. These are the public
    /// endpoints' limits: a changed base URL is a mirror or a proxy with limits of its own.
    pub fn rate(&self) -> Option<(u32, u32)> {
        match self {
            // 30 a minute degraded, 90 otherwise: `paced_after` follows the one stated.
            Self::AniList(client)
                if client.base_url() == crate::integrations::anilist::DEFAULT_BASE_URL =>
            {
                Some((30, 2))
            }
            // Tenrai publishes no rate: three a second and sixty a minute, the minute binding.
            Self::Tenrai(client)
                if client.base_url() == crate::integrations::tenrai::DEFAULT_BASE_URL =>
            {
                Some((60, 3))
            }
            // MyAnimeList publishes no rate, and its front blocks an address that keeps up more
            // than about one a second.
            Self::MyAnimeList(client)
                if client.base_url() == crate::integrations::myanimelist::DEFAULT_BASE_URL =>
            {
                Some((60, 1))
            }
            // A daily quota rather than a rate, but a modest ceiling spares a hobby server.
            Self::Omdb(client)
                if client.base_url() == crate::integrations::omdb::DEFAULT_BASE_URL =>
            {
                Some((300, 10))
            }
            // TMDB's forty a second is above what concurrency allows, and TheTVDB publishes none.
            _ => None,
        }
    }

    /// The requests this source may be sent in a UTC day, counted across
    /// every caller: an OMDb key is given a daily quota, a thousand on a free
    /// one, and refuses every request past it until the day ends.
    pub fn daily_quota(&self) -> Option<crate::services::quota::DailyQuota> {
        match self {
            Self::Omdb(client) => {
                Some(crate::services::quota::DailyQuota::new(OMDB, client.daily_requests()))
            }
            _ => None,
        }
    }

    /// Pace `pace` after an answer of this source: to the wait a refusal named,
    /// and to the limit the source states beside a success.
    pub async fn paced_after<T>(
        &self,
        pace: &crate::services::rate_limit::RateLimiter,
        outcome: &AppResult<T>,
    ) {
        crate::services::rate_limit::after_answer(pace, outcome).await;
        if let (Self::AniList(client), Ok(_)) = (self, outcome) {
            pace.follow(client.stated_limit()).await;
        }
    }

    /// How many of this source's requests may be in flight at once. Tenrai and MyAnimeList limit
    /// each address, and more would only earn the refusal that stops the whole pass.
    pub fn concurrency(&self, configured: usize) -> usize {
        match self {
            Self::Tenrai(_) => configured.min(2),
            Self::MyAnimeList(_) => 1,
            _ => configured,
        }
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        match self {
            Self::Tmdb(client) => client.test_connection().await,
            Self::AniList(client) => client.test_connection().await,
            Self::Tenrai(client) => client.test_connection().await,
            Self::MyAnimeList(client) => client.test_connection().await,
            Self::Omdb(client) => client.test_connection().await,
            Self::Tvdb(client) => client.test_connection().await,
        }
    }

    /// [`FetchingSource::resolve`], a search the source refuses for this
    /// title read as one that found nothing ([`is_a_miss`]).
    pub async fn find(
        &self,
        title: &str,
        year: Option<i64>,
        media_type: &str,
    ) -> AppResult<Option<String>> {
        match self.resolve(title, year, media_type).await {
            Err(error) if is_a_miss(&error) => Ok(None),
            other => other,
        }
    }

    /// [`FetchingSource::fetch`], a title the source refuses ([`is_a_miss`]) answered `None`, which
    /// is stored as an empty answer.
    pub async fn fetched(
        &self,
        external_id: &str,
        media_type: &str,
    ) -> AppResult<Option<ProviderMetadata>> {
        match self.fetch(external_id, media_type).await {
            Ok(answer) => Ok(Some(answer)),
            Err(error) if is_a_miss(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// This source's identifier for an item it shares no id with. `None` when nothing matched,
    /// an answer that stops the next pass from searching again.
    pub async fn resolve(
        &self,
        title: &str,
        year: Option<i64>,
        media_type: &str,
    ) -> AppResult<Option<String>> {
        match self {
            Self::AniList(client) => {
                let candidates = client.search(title, media_type).await?;
                Ok(pick_candidate(
                    title,
                    year,
                    candidates.into_iter().map(|c| (c.id.to_string(), c.year, c.titles)),
                ))
            }
            Self::Tenrai(client) => {
                let candidates = client.search(title, media_type).await?;
                Ok(pick_candidate(
                    title,
                    year,
                    candidates.into_iter().map(|c| (c.id.to_string(), c.year, c.titles)),
                ))
            }
            Self::MyAnimeList(client) => {
                let candidates = client.search(title, media_type).await?;
                Ok(pick_candidate(
                    title,
                    year,
                    candidates.into_iter().map(|c| (c.id.to_string(), c.year, c.titles)),
                ))
            }
            // Every other source is addressed by an id the library already has.
            _ => Ok(None),
        }
    }

    /// What this source answers about one title, its status read into the
    /// words the Arrs use (`integrations::status`).
    pub async fn fetch(&self, external_id: &str, media_type: &str) -> AppResult<ProviderMetadata> {
        let mut answer = match self {
            Self::Tmdb(client) => {
                let details = client.get_details(numeric(external_id, "TMDB")?, media_type).await?;
                ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    certifications: details.certifications,
                    status: details.status,
                    overview: details.overview,
                    ratings: details.ratings,
                    ..Default::default()
                }
            }
            Self::AniList(client) => {
                let details = client.get_details(numeric(external_id, "AniList")?).await?;
                ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    origin_countries: details.origin_countries,
                    status: details.status,
                    overview: details.overview,
                    ratings: details.ratings,
                    ..Default::default()
                }
            }
            Self::Tenrai(client) => {
                let details = client.get_details(numeric(external_id, "Tenrai")?).await?;
                ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    // MyAnimeList rates in a system of its own.
                    certification_scale: details
                        .certification
                        .as_ref()
                        .map(|_| crate::integrations::certification::MAL.to_string()),
                    certification: details.certification,
                    status: details.status,
                    overview: details.overview,
                    ratings: details.ratings,
                    ..Default::default()
                }
            }
            Self::MyAnimeList(client) => {
                let details = client.get_details(numeric(external_id, "MyAnimeList")?).await?;
                ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    certification_scale: details
                        .certification
                        .as_ref()
                        .map(|_| crate::integrations::certification::MAL.to_string()),
                    certification: details.certification,
                    status: details.status,
                    overview: details.overview,
                    ratings: details.ratings,
                    ..Default::default()
                }
            }
            Self::Omdb(client) => {
                let details = client.get_details(external_id).await?;
                ProviderMetadata {
                    genres: details.genres,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    // OMDb gives the MPA's rating, for the United States.
                    certification_scale: details.certification.as_ref().map(|_| "US".to_string()),
                    certification: details.certification,
                    overview: details.overview,
                    ratings: details.ratings,
                    ..Default::default()
                }
            }
            Self::Tvdb(client) => {
                let details = client.get_details(external_id).await?;
                ProviderMetadata {
                    genres: details.genres,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    certifications: details.certifications,
                    status: details.status,
                    overview: details.overview,
                    ..Default::default()
                }
            }
        };
        answer.status = answer
            .status
            .and_then(|raw| crate::integrations::status::read(&raw, media_type))
            .map(str::to_string);
        Ok(answer)
    }
}

/// Whether a source's refusal is about one title rather than the source: a 404
/// says it does not have the title, a 400, 410 or 422 that it will never answer
/// that id or that query. Remembered as an answer, or every pass asks again.
pub fn is_a_miss(error: &AppError) -> bool {
    matches!(error, AppError::ExternalApi { status: 400 | 404 | 410 | 422, .. })
}

fn numeric(external_id: &str, service: &str) -> AppResult<i64> {
    external_id.parse().map_err(|_| {
        crate::error::AppError::BadRequest(format!("'{external_id}' is not a {service} identifier"))
    })
}

/// Accept a candidate only on an exact title match and a year that agrees. A search is where a
/// source can attach the wrong work, whose genres route a film into the wrong folder: better to
/// resolve nothing and let the source below answer.
fn pick_candidate<I>(title: &str, year: Option<i64>, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = (String, Option<i64>, Vec<String>)>,
{
    let wanted = normalise_value(title);
    if wanted.is_empty() {
        return None;
    }

    for (id, candidate_year, titles) in candidates {
        let title_matches = titles.iter().any(|t| normalise_value(t) == wanted);
        // A year drifts by one between databases: a December film is a January one elsewhere.
        let year_matches = match (year, candidate_year) {
            (Some(wanted), Some(found)) => (wanted - found).abs() <= 1,
            // An unknown library year is not the candidate's fault: the exact title carries it.
            (None, _) => true,
            // A candidate not knowing its year, when ours is known, is too little to go on.
            (Some(_), None) => false,
        };

        if title_matches && year_matches {
            return Some(id);
        }
    }

    None
}

/// The library's own identity for an item, namespaced by the id it comes from. Shared by every
/// instance holding the item, so a film in two Radarrs is searched for once.
pub fn local_key(media: &Media) -> String {
    local_key_of(media.tmdb_id, media.tvdb_id, media.imdb_id.as_deref(), &media.title, media.year)
}

/// The same, from loose columns: the enrichment pass reads six columns rather
/// than whole `Media` rows, and the two must agree or nothing would ever match.
pub fn local_key_of(
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<&str>,
    title: &str,
    year: Option<i64>,
) -> String {
    if let Some(id) = tmdb_id {
        return format!("tmdb:{id}");
    }
    if let Some(id) = tvdb_id {
        return format!("tvdb:{id}");
    }
    match imdb_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => format!("imdb:{id}"),
        None => format!("title:{}|{}", normalise_value(title), year.unwrap_or(0)),
    }
}

/// Resolutions already made, including the ones that found nothing.
pub type Identifiers = HashMap<(String, String, String), Option<String>>;

pub async fn load_identifiers(connection: &mut sqlx::SqliteConnection) -> AppResult<Identifiers> {
    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT source, media_type, local_key, external_id FROM source_identifiers")
            .fetch_all(connection)
            .await?;

    Ok(rows
        .into_iter()
        .map(|(source, kind, key, external)| ((source, kind, key), external))
        .collect())
}

/// Whether the cached answer of `source` for `external_id` was asked for
/// again. An answer that cannot be read is taken as current: asking a source
/// on a database error is a request spent for nothing.
pub async fn is_stale(
    pool: &SqlitePool,
    source: &str,
    external_id: &str,
    media_type: &str,
) -> bool {
    let stale: Option<bool> = sqlx::query_scalar(
        "SELECT stale FROM metadata_cache WHERE source = ? AND external_id = ? AND media_type = ?",
    )
    .bind(source)
    .bind(external_id)
    .bind(media_type)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    stale.unwrap_or(false)
}

/// The resolutions made for these items, which is all one media page reads.
pub async fn load_identifiers_of(
    connection: &mut sqlx::SqliteConnection,
    media: &[Media],
) -> AppResult<Identifiers> {
    let mut keys: HashMap<&str, Vec<String>> = HashMap::new();
    for item in media {
        keys.entry(item.media_type.as_str()).or_default().push(local_key(item));
    }
    let mut identifiers = Identifiers::new();
    for (kind, keys) in keys {
        for chunk in keys.chunks(crate::services::routing::BIND_CHUNK) {
            let sql = format!(
                "SELECT source, media_type, local_key, external_id FROM source_identifiers
                  WHERE media_type = ? AND local_key IN ({})",
                crate::db::placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, (String, String, String, Option<String>)>(
                AssertSqlSafe(sql.as_str()),
            )
            .bind(kind);
            for key in chunk {
                query = query.bind(key);
            }
            identifiers.extend(
                query
                    .fetch_all(&mut *connection)
                    .await?
                    .into_iter()
                    .map(|(source, kind, key, external)| ((source, kind, key), external)),
            );
        }
    }
    Ok(identifiers)
}

/// How long a search that found nothing for `key` holds: thirty days and up to ten more, the
/// same for a key each time. A work is often listed late, but every miss searched each pass would
/// hold a paced enrichment for hours, and one shared lifetime would bring them all due at once.
fn miss_lifetime(key: &str) -> chrono::Duration {
    use sha2::Digest;
    let spread = sha2::Sha256::digest(key.as_bytes())[0] % 11;
    chrono::Duration::days(30 + i64::from(spread))
}

/// The keys already resolved for one source, so a pass only searches for what
/// it has never searched for, or found nothing for long enough ago.
pub async fn resolved_keys(pool: &SqlitePool, source: &str) -> AppResult<HashSet<String>> {
    let rows: Vec<(String, String, bool, String)> = sqlx::query_as(
        "SELECT media_type, local_key, external_id IS NOT NULL, resolved_at
           FROM source_identifiers WHERE source = ?",
    )
    .bind(source)
    .fetch_all(pool)
    .await?;

    let now = chrono::Utc::now();
    Ok(rows
        .into_iter()
        .map(|(kind, key, found, at)| (resolution_key(&kind, &key), found, at))
        .filter(|(key, found, at)| {
            *found
                || crate::services::routing::parse_timestamp(at)
                    .is_some_and(|at| at + miss_lifetime(key) > now)
        })
        .map(|(key, _, _)| key)
        .collect())
}

/// The source naming a work by the same MyAnimeList id as `source`: what one found by search,
/// the other takes without searching, since a search is what MyAnimeList limits most.
pub fn sharing_ids_with(source: &str) -> Option<&'static str> {
    match source {
        TENRAI => Some(MYANIMELIST),
        MYANIMELIST => Some(TENRAI),
        _ => None,
    }
}

/// The identifiers `source` found by search, by [`resolution_key`].
pub async fn found_identifiers(
    pool: &SqlitePool,
    source: &str,
) -> AppResult<HashMap<String, String>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT media_type, local_key, external_id FROM source_identifiers
          WHERE source = ? AND external_id IS NOT NULL",
    )
    .bind(source)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(kind, key, id)| (resolution_key(&kind, &key), id)).collect())
}

/// The values of `anime_search`: AniList and MyAnimeList searched for the
/// titles that may be anime, the default, or for every title.
pub const ANIME_SEARCH: [&str; 2] = ["animated", "all"];

/// Whether a source found by search may be searched for a title its Arr files under `genres` and
/// `series_type`. AniList and MyAnimeList list anime alone, so under the default `scope` only a
/// title that may be anime is: Animation or Anime, an anime series, or no genre yet.
pub fn may_search(scope: &str, genres: &[String], series_type: Option<&str>) -> bool {
    scope == ANIME_SEARCH[1]
        || series_type.is_some_and(|kind| kind.eq_ignore_ascii_case("anime"))
        || genres.is_empty()
        || genres
            .iter()
            .any(|genre| matches!(normalise_value(genre).as_str(), "animation" | "anime"))
}

/// `(media_type, local_key)` as one string. No title holds the separator, so a series and a film
/// with the same key never collide.
pub fn resolution_key(media_type: &str, local_key: &str) -> String {
    format!("{media_type}\u{1f}{local_key}")
}

/// How an identifier was found, as `source_identifiers.found_by` holds it.
pub const BY_SEARCH: &str = "search";
pub const BY_SHARING: &str = "shared";
pub const BY_MAPPING: &str = "mapping";

pub async fn remember_identifier(
    pool: &SqlitePool,
    source: &str,
    media_type: &str,
    local_key: &str,
    external_id: Option<&str>,
    found_by: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO source_identifiers
                (source, media_type, local_key, external_id, resolved_at, found_by)
         VALUES (?, ?, ?, ?, datetime('now'), ?)
         ON CONFLICT(source, media_type, local_key) DO UPDATE SET
            external_id = excluded.external_id,
            resolved_at = excluded.resolved_at,
            found_by = excluded.found_by",
    )
    .bind(source)
    .bind(media_type)
    .bind(local_key)
    .bind(external_id)
    .bind(found_by)
    .execute(pool)
    .await?;

    Ok(())
}

/// This item's id in a given source's namespace.
pub fn external_id(
    provider: &ProviderInfo,
    media: &Media,
    identifiers: &Identifiers,
) -> Option<String> {
    if !provider.media_types.contains(&media.media_type.as_str()) {
        return None;
    }
    match provider.addressing {
        Addressing::Local => None,
        Addressing::Column("tmdb_id") => media.tmdb_id.map(|id| id.to_string()),
        Addressing::Column("tvdb_id") => media.tvdb_id.map(|id| id.to_string()),
        Addressing::Column("imdb_id") => media.imdb_id.clone().filter(|id| !id.trim().is_empty()),
        Addressing::Column(other) => {
            tracing::warn!("No accessor for the media column '{other}'");
            None
        }
        Addressing::Search => identifiers
            .get(&(provider.id.to_string(), media.media_type.clone(), local_key(media)))
            .cloned()
            .flatten(),
    }
}

/// One row of `metadata_cache`, as the per-item path reads it. A named `FromRow`, since a tuple
/// binds by position and a column inserted in the list would fail at run time.
#[derive(Debug, sqlx::FromRow)]
pub struct CacheRow {
    pub genres: String,
    pub ratings: String,
    pub keywords: String,
    pub original_language: Option<String>,
    pub origin_countries: String,
    pub certification: Option<String>,
    pub certification_scale: Option<String>,
    pub certifications: String,
    pub status: Option<String>,
    pub overview: Option<String>,
}

/// The columns a rule can be written against. No condition reads the status or the synopsis,
/// and the overviews would weigh on `load_cache` for nothing, so only the per-item path reads
/// them (`CACHE_COLUMNS`), for the explanation panel.
pub const EVALUATED_COLUMNS: &str = "source, external_id, media_type, genres, keywords,
     original_language, origin_countries, certification, certification_scale, certifications,
     ratings";

/// What one row answers with. The three key columns are not among them: the
/// only reader addresses a row by them and never reads them back.
pub const CACHE_COLUMNS: &str = "genres, keywords, original_language, origin_countries,
     certification, certification_scale, certifications, status, overview, ratings";

impl CacheRow {
    /// Malformed JSON yields an empty list rather than an error: one bad cache
    /// row must not be able to break a whole simulation.
    pub fn into_answer(self) -> ProviderMetadata {
        ProviderMetadata {
            genres: serde_json::from_str(&self.genres).unwrap_or_default(),
            keywords: serde_json::from_str(&self.keywords).unwrap_or_default(),
            original_language: self.original_language,
            origin_countries: serde_json::from_str(&self.origin_countries).unwrap_or_default(),
            certification: self.certification,
            certification_scale: self.certification_scale,
            certifications: serde_json::from_str(&self.certifications).unwrap_or_default(),
            status: self.status,
            overview: self.overview,
            ratings: serde_json::from_str(&self.ratings).unwrap_or_default(),
        }
    }
}

/// The rating of the first of `regions` that `certifications` rates, and that
/// region. A blank rating rates nothing.
pub fn rating_for(
    certifications: &BTreeMap<String, String>,
    regions: &[String],
) -> Option<(String, String)> {
    regions.iter().find_map(|region| {
        let region = region.to_ascii_uppercase();
        let rating = certifications.get(&region)?.trim();
        (!rating.is_empty()).then(|| (rating.to_string(), region))
    })
}

/// `metadata_cache` as a table expression whose `certification` and `certification_scale` hold
/// what [`rating_for`] picks for `regions`, unless the row's own column holds a rating. The
/// regions are spliced, never bound, so it reads in any query: only two ASCII letters are kept.
pub fn rated_cache(regions: &[String]) -> String {
    let codes: Vec<String> = regions
        .iter()
        .map(|region| region.to_ascii_uppercase())
        .filter(|code| code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()))
        .collect();
    let picked = |what: &str| {
        if codes.is_empty() {
            return "NULL".to_string();
        }
        let listed: Vec<String> = codes.iter().map(|code| format!("'{code}'")).collect();
        let ranked: String = codes
            .iter()
            .enumerate()
            .map(|(rank, code)| format!(" WHEN '{code}' THEN {rank}"))
            .collect();
        format!(
            "(SELECT {what} FROM json_each(CASE WHEN json_valid(certifications)
                                              THEN certifications ELSE '{{}}' END) k
               WHERE k.key IN ({}) AND TRIM(k.value) != ''
               ORDER BY CASE k.key{ranked} END LIMIT 1)",
            listed.join(", ")
        )
    };
    format!(
        "(SELECT source, external_id, media_type, genres, keywords, original_language,
                 origin_countries,
                 COALESCE(certification, {value}) AS certification,
                 CASE WHEN certification IS NOT NULL THEN certification_scale
                      ELSE {region} END AS certification_scale,
                 ratings
            FROM metadata_cache)",
        value = picked("TRIM(k.value)"),
        region = picked("k.key"),
    )
}

/// Every cached answer, keyed by `(source, external id, media type)`. Loaded whole, once per
/// simulation: a query per item and source is what `tests/scale.rs` refuses.
pub async fn load_cache(
    connection: &mut sqlx::SqliteConnection,
) -> AppResult<std::collections::HashMap<(String, String, String), ProviderMetadata>> {
    /// The evaluated columns alone, named for the reason `CacheRow` is.
    #[derive(Debug, sqlx::FromRow)]
    struct EvaluatedRow {
        source: String,
        external_id: String,
        media_type: String,
        genres: String,
        keywords: String,
        original_language: Option<String>,
        origin_countries: String,
        certification: Option<String>,
        certification_scale: Option<String>,
        certifications: String,
        ratings: String,
    }

    let rows: Vec<EvaluatedRow> =
        sqlx::query_as(AssertSqlSafe(format!("SELECT {EVALUATED_COLUMNS} FROM metadata_cache")))
            .fetch_all(connection)
            .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let answer = CacheRow {
                genres: row.genres,
                keywords: row.keywords,
                original_language: row.original_language,
                origin_countries: row.origin_countries,
                certification: row.certification,
                certification_scale: row.certification_scale,
                certifications: row.certifications,
                ratings: row.ratings,
                // No condition reads them, and the panel gets them from the per-item path.
                status: None,
                overview: None,
            }
            .into_answer();
            ((row.source, row.external_id, row.media_type), answer)
        })
        .collect())
}

/// The cached answers for the items named, keyed as [`load_cache`] keys the library's. Every
/// column, `CACHE_COLUMNS`, since the one-item readers show the status and the synopsis.
pub async fn load_cache_of(
    connection: &mut sqlx::SqliteConnection,
    media: &[Media],
    providers: &[&'static ProviderInfo],
    identifiers: &Identifiers,
) -> AppResult<HashMap<(String, String, String), ProviderMetadata>> {
    #[derive(sqlx::FromRow)]
    struct KeyedRow {
        external_id: String,
        #[sqlx(flatten)]
        answer: CacheRow,
    }

    let mut cache = HashMap::new();
    for provider in providers.iter().filter(|provider| provider.id != ARR) {
        let mut ids: HashMap<&str, HashSet<String>> = HashMap::new();
        for item in media {
            if let Some(external_id) = external_id(provider, item, identifiers) {
                ids.entry(item.media_type.as_str()).or_default().insert(external_id);
            }
        }
        for (kind, ids) in ids {
            let ids: Vec<String> = ids.into_iter().collect();
            for chunk in ids.chunks(crate::services::routing::BIND_CHUNK) {
                let sql = format!(
                    "SELECT external_id, {CACHE_COLUMNS} FROM metadata_cache
                      WHERE source = ? AND media_type = ? AND external_id IN ({})",
                    crate::db::placeholders(chunk.len())
                );
                let mut query = sqlx::query_as::<_, KeyedRow>(AssertSqlSafe(sql.as_str()))
                    .bind(provider.id)
                    .bind(kind);
                for id in chunk {
                    query = query.bind(id);
                }
                for row in query.fetch_all(&mut *connection).await? {
                    let key = (provider.id.to_string(), row.external_id, kind.to_string());
                    cache.insert(key, row.answer.into_answer());
                }
            }
        }
    }
    Ok(cache)
}

#[cfg(test)]
mod tests {

    /// A score range is warned of by the scales a source names, so a source answering ratings
    /// on no scale, or on one no rule can name, leaves a rule unanswered without a word.
    #[test]
    fn a_source_names_its_scales_exactly_when_it_answers_ratings() {
        for provider in super::PROVIDERS {
            let rates = provider.fields.contains(&super::MetadataField::Ratings);
            assert_eq!(rates, !provider.scales.is_empty(), "{}", provider.id);
            for scale in provider.scales {
                assert!(crate::models::scale(scale).is_some(), "{}: {scale}", provider.id);
            }
        }
    }

    #[test]
    fn the_first_region_rated_gives_the_rating() {
        let rated = super::BTreeMap::from([
            ("DE".to_string(), " ".to_string()),
            ("FR".to_string(), "12".to_string()),
            ("US".to_string(), "PG-13".to_string()),
        ]);
        let pick = |regions: &[&str]| {
            let regions: Vec<String> = regions.iter().map(|region| region.to_string()).collect();
            super::rating_for(&rated, &regions)
        };
        assert_eq!(pick(&["us", "FR"]), Some(("PG-13".into(), "US".into())));
        assert_eq!(pick(&["DE", "FR"]), Some(("12".into(), "FR".into())));
        assert_eq!(pick(&["DE", "GB"]), None);
    }

    #[tokio::test]
    async fn every_path_to_a_source_spends_one_pace() {
        use crate::integrations::anilist::AniListClient;
        let paces = Paces::default();
        let source = FetchingSource::AniList(AniListClient::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1",
        ));
        assert_eq!(source.rate(), None, "the fixture's source is paced");
        paces.of(&source).penalise(std::time::Duration::from_millis(300)).await;
        let started = std::time::Instant::now();
        paces.of(&source).acquire().await;
        assert!(started.elapsed() >= std::time::Duration::from_millis(250), "two budgets");
    }

    use super::*;

    #[test]
    fn the_order_is_read_from_the_setting() {
        let order = parse_order("tmdb, arr");
        assert_eq!(order.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["tmdb", "arr"]);
    }

    #[test]
    fn a_duplicate_keeps_only_its_highest_position() {
        let order = parse_order("tmdb,arr,tmdb");
        assert_eq!(order.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["tmdb", "arr"]);
    }

    /// Ahead of the listed sources, the Arr would answer in their place.
    #[test]
    fn a_list_without_the_arr_reads_it_last() {
        let order = parse_order("tmdb, omdb");
        assert_eq!(order.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["tmdb", "omdb", "arr"]);
    }

    /// A source may spell a title without its accents, or with them where the
    /// library has none, and the rules already read both as one value.
    #[test]
    fn a_title_matches_its_candidate_whatever_its_accents() {
        let candidates = || {
            vec![(
                "tenrai-1".to_string(),
                Some(2001),
                vec!["Le Fabuleux Destin d'Amelie Poulain".to_string()],
            )]
        };
        assert_eq!(
            pick_candidate("Le Fabuleux Destin d'Amélie Poulain", Some(2001), candidates()),
            Some("tenrai-1".to_string())
        );
        assert_eq!(
            local_key_of(None, None, None, "Amélie", Some(2001)),
            local_key_of(None, None, None, "Amelie", Some(2001))
        );
    }

    #[tokio::test]
    async fn a_miss_is_remembered_a_month_or_so_and_a_found_id_for_good() {
        let pool = crate::db::test_pool().await;
        sqlx::query(
            "INSERT INTO source_identifiers (source, media_type, local_key, external_id, resolved_at)
             VALUES ('anilist', 'movie', 'found-long-ago', '523', datetime('now', '-400 days')),
                    ('anilist', 'movie', 'missed-29-days-ago', NULL, datetime('now', '-29 days')),
                    ('anilist', 'movie', 'missed-41-days-ago', NULL, datetime('now', '-41 days')),
                    ('tenrai', 'movie', 'another-source', '1', datetime('now'))",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut known: Vec<String> =
            resolved_keys(&pool, "anilist").await.unwrap().into_iter().collect();
        known.sort();

        assert_eq!(
            known,
            [
                resolution_key("movie", "found-long-ago"),
                resolution_key("movie", "missed-29-days-ago")
            ]
        );
    }

    #[test]
    fn a_tmdb_answer_is_kept_six_months_at_most() {
        assert_eq!(cache_days(TMDB, 3_650), 180);
        assert_eq!(cache_days(TMDB, 30), 30);
        assert_eq!(cache_days(OMDB, 3_650), 3_650);
        assert_eq!(cache_days(OMDB, 0), 1);
    }

    #[test]
    fn misses_written_at_one_instant_come_due_over_ten_days() {
        let days: HashSet<i64> = (0..200)
            .map(|n| miss_lifetime(&resolution_key("movie", &format!("title:{n}"))).num_days())
            .collect();
        assert_eq!(days, (30..=40).collect());
    }

    #[test]
    fn only_a_title_that_may_be_anime_is_searched_by_default() {
        let genres = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert!(may_search("animated", &genres(&["ANIMATION"]), None));
        assert!(may_search("animated", &genres(&["Drama", "anime"]), None));
        assert!(may_search("animated", &[], None));
        assert!(may_search("animated", &genres(&["Drama"]), Some("Anime")));
        assert!(!may_search("animated", &genres(&["Drama"]), Some("standard")));
        assert!(may_search("all", &genres(&["Drama"]), None));
    }

    #[test]
    fn a_candidate_is_taken_on_its_title_and_a_year_within_one() {
        let totoro = |id: &str, year: Option<i64>| {
            (id.to_string(), year, vec!["Tonari no Totoro".to_string(), "Totoro".to_string()])
        };
        for (library_year, candidate_year, taken) in [
            (Some(1988), Some(1988), true),
            (Some(1988), Some(1989), true),
            (Some(1988), Some(1987), true),
            (Some(1988), Some(1990), false),
            (Some(1988), Some(1986), false),
            (Some(1988), None, false),
            (None, Some(1988), true),
            (None, None, true),
        ] {
            let picked = pick_candidate("Totoro", library_year, [totoro("a-1", candidate_year)]);
            assert_eq!(picked.is_some(), taken, "{library_year:?} against {candidate_year:?}");
        }

        assert_eq!(pick_candidate("!?", None, [("a-1".into(), None, vec!["!?".into()])]), None);
        let several = [
            ("other".to_string(), Some(1988), vec!["Kiki's Delivery Service".to_string()]),
            totoro("too-old", Some(1970)),
            totoro("first", Some(1988)),
            totoro("second", Some(1988)),
        ];
        assert_eq!(pick_candidate("Totoro", Some(1988), several), Some("first".into()));
    }

    #[test]
    fn adding_tmdb_covers_every_field() {
        let fields = covered_fields(&[info(ARR).unwrap(), info(TMDB).unwrap()]);
        assert_eq!(fields.len(), MetadataField::ALL.len());
    }
}
