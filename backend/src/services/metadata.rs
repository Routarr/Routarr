//! The metadata sources, their priority order, and where each one gets its ids.
//!
//! An enum rather than a `dyn` trait, like `integrations::adapter::ArrAdapter`:
//! a provider is added in code anyway, and the enum keeps the whole set
//! checkable at compile time.
//!
//! Two kinds of source:
//!
//! * `arr` costs nothing: Radarr and Sonarr return genres, original language
//!   and certification in the payload the sync already reads, so it is never
//!   fetched, rate-limited or stale, and needs no key. It is what makes the
//!   others optional.
//! * a *fetched* source issues one request per item and is cached in
//!   `metadata_cache` under its own id namespace.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};

use crate::error::AppResult;
use crate::integrations::anilist::AniListClient;
use crate::integrations::jikan::JikanClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tmdb::TmdbClient;
use crate::integrations::tvdb::TvdbClient;
use crate::models::{Media, MetadataField, ProviderMetadata};
use crate::services::rule_engine::normalise_value;

pub const ARR: &str = "arr";
pub const TMDB: &str = "tmdb";
pub const ANILIST: &str = "anilist";
pub const JIKAN: &str = "jikan";
pub const OMDB: &str = "omdb";
pub const TVDB: &str = "tvdb";

/// Order applied when the setting is missing or unreadable: the free source
/// alone. Every other source is opt-in: each is extra requests, and a keyed
/// one listed without its key answers nothing and makes the diagnostics warn.
/// A TMDb key in the environment lists TMDb at startup
/// (`maintenance::converge_metadata_sources`).
pub const DEFAULT_ORDER: &[&str] = &[ARR];

/// How a source is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    /// Answered from the `media` row. Nothing to look up, nothing to cache, no
    /// request: this is what `arr` is.
    Local,
    /// The identifier is a column of `media`. It is interpolated into SQL, so it
    /// must stay a literal from this file and never anything a user can
    /// influence.
    Column(&'static str),
    /// The source knows none of our identifiers, so the item has to be found by
    /// title and year and the answer remembered in `source_identifiers`.
    Search,
}

/// What a source is and what it can answer.
#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    pub id: &'static str,
    /// Shown as-is in the interface. A proper noun in every language, like
    /// "Radarr" and "Sonarr" everywhere else in Routarr, so it is never
    /// translated.
    pub display_name: &'static str,
    /// False for `arr`, whose data arrives with the library sync.
    pub fetched: bool,
    /// Whether the user must supply a credential before it can answer.
    pub needs_key: bool,
    /// The environment variable that credential is read from.
    ///
    /// Named here so the interface can say *which* variable to set rather than
    /// "an API key is missing", a message the user cannot act on. `None` for a
    /// source that authenticates nothing.
    pub key_env: Option<&'static str>,
    pub addressing: Addressing,
    pub fields: &'static [MetadataField],
}

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
        // No keywords and no origin country: the Arr APIs simply do not carry
        // them. Those two fields are the entire reason to add a second source.
        fields: &[
            MetadataField::Genres,
            MetadataField::OriginalLanguage,
            MetadataField::Certification,
        ],
    },
    ProviderInfo {
        id: TMDB,
        display_name: "TMDb",
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
        ],
    },
    ProviderInfo {
        id: ANILIST,
        display_name: "AniList",
        fetched: true,
        // Its public GraphQL API authenticates nothing.
        needs_key: false,
        key_env: None,
        addressing: Addressing::Search,
        // No certification: AniList has only an adult flag, which is not a
        // rating a `certification_in` rule could name.
        fields: &[MetadataField::Genres, MetadataField::Keywords, MetadataField::OriginCountries],
    },
    ProviderInfo {
        id: JIKAN,
        display_name: "Jikan (MyAnimeList)",
        fetched: true,
        needs_key: false,
        key_env: None,
        addressing: Addressing::Search,
        fields: &[MetadataField::Genres, MetadataField::Keywords, MetadataField::Certification],
    },
    ProviderInfo {
        id: OMDB,
        display_name: "OMDb",
        fetched: true,
        // A free key, but a key: OMDb rejects an unauthenticated request rather
        // than degrading.
        needs_key: true,
        key_env: Some("OMDB_API_KEY"),
        addressing: Addressing::Column("imdb_id"),
        fields: &[
            MetadataField::Genres,
            MetadataField::OriginalLanguage,
            MetadataField::OriginCountries,
            MetadataField::Certification,
        ],
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
    },
];

/// Whether a source can answer at all in this configuration.
///
/// A source listed by the user but missing its credential is not an error: it
/// simply contributes nothing, and the sources below it answer instead. That is
/// the whole promise of the ordered list.
pub fn is_usable(
    provider: &ProviderInfo,
    keys: &std::collections::HashMap<&'static str, String>,
) -> bool {
    // Resolved keys rather than the config: a credential saved in the interface
    // counts exactly as much as one handed in by the environment, and only the
    // caller knows which of the two answered.
    if provider.needs_key { keys.contains_key(provider.id) } else { true }
}

pub fn info(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// Parse the `metadata_providers` setting into an order.
///
/// An unknown id is dropped rather than refused: a database written by a newer
/// build that knows a source this one lacks must still route on this one,
/// minus that source.
/// A duplicate keeps only its first, highest, position.
pub fn parse_order(raw: &str) -> Vec<&'static ProviderInfo> {
    let mut seen: Vec<&'static ProviderInfo> = Vec::new();
    for id in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(provider) = info(id)
            && !seen.iter().any(|p| p.id == provider.id)
        {
            seen.push(provider);
        }
    }
    // `arr` is always read, last if nowhere else: the validator refuses a list
    // without it, and a stored value written around the validator must not
    // leave a library with no source at all.
    if !seen.iter().any(|p| p.id == "arr")
        && let Some(arr) = info("arr")
    {
        seen.push(arr);
    }
    seen
}

/// Metadata sources as the user ordered them, highest priority first, from the
/// `metadata_providers` setting.
///
/// An absent setting is the shipped default. Any stored value, the empty
/// string included, still yields `arr` (see `parse_order`), so turning every
/// fetched source off leaves the library routed on what the Arr reports.
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

/// The `arr` source: what Radarr or Sonarr already told us about this item.
///
/// Reads the row, never the network. A malformed `genres` payload yields no
/// genres rather than an error, like `tag_labels`: one signal must not be able
/// to break an evaluation.
pub fn from_media(media: &Media) -> ProviderMetadata {
    ProviderMetadata {
        genres: media.genre_list(),
        keywords: Vec::new(),
        original_language: media.original_language.clone(),
        origin_countries: Vec::new(),
        certification: media.certification.clone(),
        // `status`, `overview` and the poster stay empty: the first is already a
        // column of `media` that the engine reads directly, and the other two
        // are not in the Arr payloads.
        status: None,
        overview: None,
        poster_path: None,
    }
}

/// A source that has to be asked over the network.
///
/// A new source adds a variant here, and the compiler finds every exhaustive
/// match on it. It does not find the arms that fall through to a default when
/// the source is missing from them: `resolve` for a `Search` source (the source
/// resolves nothing and is never fetched), `rate` for a paced public endpoint
/// (it goes unpaced), `AppState::metadata_sources` (no client is ever built),
/// and for a keyed source `provider_key_from` and `provider_keys_from` (its
/// environment key is never read, and the source never counts as usable).
#[derive(Debug, Clone)]
pub enum FetchingSource {
    Tmdb(TmdbClient),
    AniList(AniListClient),
    Jikan(JikanClient),
    Omdb(OmdbClient),
    Tvdb(TvdbClient),
}

impl FetchingSource {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Tmdb(_) => TMDB,
            Self::AniList(_) => ANILIST,
            Self::Jikan(_) => JIKAN,
            Self::Omdb(_) => OMDB,
            Self::Tvdb(_) => TVDB,
        }
    }

    pub fn addressing(&self) -> Addressing {
        info(self.id()).map(|provider| provider.addressing).unwrap_or(Addressing::Local)
    }

    /// The sustained ceiling, in requests per minute, and the burst an idle
    /// bucket holds.
    ///
    /// Concurrency bounds how many requests are *open*. This bounds how many are
    /// *made*. Four in flight against a fast source is forty a second, which is
    /// how a pass earns a 429 while never exceeding its concurrency cap.
    /// These are the *published* limits of the *public* endpoints, so they only
    /// apply while a source is pointed at one. A base URL the operator changed
    /// is a mirror, a caching proxy or a local instance. It has its own limits,
    /// usually none, and pacing it to a third party's ceiling would be a delay
    /// bought for nothing.
    pub fn rate(&self) -> Option<(u32, u32)> {
        match self {
            // AniList documents 90 a minute. Nothing authenticates, so the
            // limit is per address and shared with anything else on the host.
            Self::AniList(client)
                if client.base_url() == crate::integrations::anilist::DEFAULT_BASE_URL =>
            {
                Some((90, 5))
            }
            // Jikan documents three a second *and* sixty a minute. The minute is
            // the binding one, and it is unofficial infrastructure that deserves
            // the politeness.
            Self::Jikan(client)
                if client.base_url() == crate::integrations::jikan::DEFAULT_BASE_URL =>
            {
                Some((60, 3))
            }
            // OMDb's free tier is a daily quota rather than a rate, but it is
            // one person's hobby server: a modest ceiling costs nothing.
            Self::Omdb(client)
                if client.base_url() == crate::integrations::omdb::DEFAULT_BASE_URL =>
            {
                Some((300, 10))
            }
            // TMDb and TheTVDB publish no rate limit, so concurrency is the only
            // bound worth applying to them. A new source on a paced public
            // endpoint lands here too until it has its own arm, and goes unpaced.
            _ => None,
        }
    }

    /// The limiter for this source, ready to be shared across a pass.
    pub fn limiter(&self) -> crate::services::rate_limit::RateLimiter {
        match self.rate() {
            Some((per_minute, burst)) => {
                crate::services::rate_limit::RateLimiter::new(per_minute, burst)
            }
            None => crate::services::rate_limit::RateLimiter::unlimited(),
        }
    }

    /// How many of this source's requests may be in flight at once.
    ///
    /// Jikan is an unofficial service with a documented handful of requests per
    /// second. The shared setting is a ceiling, not a target, and exceeding it
    /// here would only earn the 429 that stops the whole pass.
    pub fn concurrency(&self, configured: usize) -> usize {
        match self {
            Self::Jikan(_) => configured.min(2),
            _ => configured,
        }
    }

    pub async fn test_connection(&self) -> AppResult<bool> {
        match self {
            Self::Tmdb(client) => client.test_connection().await,
            Self::AniList(client) => client.test_connection().await,
            Self::Jikan(client) => client.test_connection().await,
            Self::Omdb(client) => client.test_connection().await,
            Self::Tvdb(client) => client.test_connection().await,
        }
    }

    /// Find this source's identifier for an item it has no shared id with.
    ///
    /// Returns `None` when nothing matched. That is an answer in its own right,
    /// and the one that stops the next pass from searching again for the same
    /// item.
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
            Self::Jikan(client) => {
                let candidates = client.search(title, media_type).await?;
                Ok(pick_candidate(
                    title,
                    year,
                    candidates.into_iter().map(|c| (c.id.to_string(), c.year, c.titles)),
                ))
            }
            // Every other source is addressed by an id the library already has.
            // A new `Search` source without its own arm lands here as well, and
            // silently resolves nothing, so it is never fetched either.
            _ => Ok(None),
        }
    }

    pub async fn fetch(&self, external_id: &str, media_type: &str) -> AppResult<ProviderMetadata> {
        match self {
            Self::Tmdb(client) => {
                let details = client.get_details(numeric(external_id, "TMDb")?, media_type).await?;
                Ok(ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    certification: details.certification,
                    status: details.status,
                    overview: details.overview,
                    poster_path: details.poster_path,
                })
            }
            Self::AniList(client) => {
                let details = client.get_details(numeric(external_id, "AniList")?).await?;
                Ok(ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    origin_countries: details.origin_countries,
                    status: details.status,
                    overview: details.overview,
                    ..Default::default()
                })
            }
            Self::Jikan(client) => {
                let details = client.get_details(numeric(external_id, "Jikan")?).await?;
                Ok(ProviderMetadata {
                    genres: details.genres,
                    keywords: details.keywords,
                    certification: details.certification,
                    status: details.status,
                    overview: details.overview,
                    ..Default::default()
                })
            }
            Self::Omdb(client) => {
                let details = client.get_details(external_id).await?;
                Ok(ProviderMetadata {
                    genres: details.genres,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    certification: details.certification,
                    overview: details.overview,
                    ..Default::default()
                })
            }
            Self::Tvdb(client) => {
                let details = client.get_details(external_id, media_type).await?;
                Ok(ProviderMetadata {
                    genres: details.genres,
                    original_language: details.original_language,
                    origin_countries: details.origin_countries,
                    certification: details.certification,
                    status: details.status,
                    overview: details.overview,
                    ..Default::default()
                })
            }
        }
    }
}

fn numeric(external_id: &str, service: &str) -> AppResult<i64> {
    external_id.parse().map_err(|_| {
        crate::error::AppError::BadRequest(format!("'{external_id}' is not a {service} identifier"))
    })
}

// ------------------------------------------------------------ resolution

/// Accept a candidate only on an exact title match and a year that agrees.
///
/// Deliberately strict. A search is the one place where a source can attach the
/// *wrong* work to a media item, and a wrong genre routes a film into the wrong
/// folder, which is the failure this whole application exists to avoid. Better
/// to resolve nothing and let the source below answer.
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
        // A release year drifts by one between databases (a December film is a
        // January release elsewhere), so one year of tolerance and no more.
        let year_matches = match (year, candidate_year) {
            (Some(wanted), Some(found)) => (wanted - found).abs() <= 1,
            // The library not knowing the year is not the candidate's fault, so
            // an exact title match alone has to carry it.
            (None, _) => true,
            // The candidate not knowing its own year, when we know ours, is too
            // little to go on.
            (Some(_), None) => false,
        };

        if title_matches && year_matches {
            return Some(id);
        }
    }

    None
}

/// The library's own identity for an item, namespaced by the id it comes from.
///
/// Shared by every instance holding the same item, so a film present in two
/// Radarrs is searched for once, not twice.
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

pub async fn load_identifiers(pool: &SqlitePool) -> AppResult<Identifiers> {
    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT source, media_type, local_key, external_id FROM source_identifiers")
            .fetch_all(pool)
            .await?;

    Ok(rows
        .into_iter()
        .map(|(source, kind, key, external)| ((source, kind, key), external))
        .collect())
}

/// The resolutions made for one item, which is all one media page reads.
pub async fn load_identifiers_of(pool: &SqlitePool, media: &Media) -> AppResult<Identifiers> {
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT source, media_type, local_key, external_id FROM source_identifiers
          WHERE media_type = ? AND local_key = ?",
    )
    .bind(&media.media_type)
    .bind(local_key(media))
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(source, kind, key, external)| ((source, kind, key), external))
        .collect())
}

/// How long a search that found nothing holds before the source is asked again.
///
/// A work is often listed after the library holds it: a film indexed before its
/// release is not on AniList or MyAnimeList yet. Not every pass either: a
/// library's worth of misses, searched again each week against sources paced
/// to about a request a second, would hold the enrichment for hours.
const MISS_LIFETIME: &str = "-30 days";

/// The keys already resolved for one source, so a pass only searches for what
/// it has never searched for, or found nothing for long enough ago.
pub async fn resolved_keys(pool: &SqlitePool, source: &str) -> AppResult<HashSet<String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT media_type, local_key FROM source_identifiers
          WHERE source = ? AND (external_id IS NOT NULL OR resolved_at > datetime('now', ?))",
    )
    .bind(source)
    .bind(MISS_LIFETIME)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(kind, key)| resolution_key(&kind, &key)).collect())
}

/// `(media_type, local_key)` as one string, for set membership.
///
/// The separator is a character no title can contain, so a series and a film
/// with the same key can never collide.
pub fn resolution_key(media_type: &str, local_key: &str) -> String {
    format!("{media_type}\u{1f}{local_key}")
}

pub async fn remember_identifier(
    pool: &SqlitePool,
    source: &str,
    media_type: &str,
    local_key: &str,
    external_id: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO source_identifiers (source, media_type, local_key, external_id, resolved_at)
         VALUES (?, ?, ?, ?, datetime('now'))
         ON CONFLICT(source, media_type, local_key) DO UPDATE SET
            external_id = excluded.external_id,
            resolved_at = excluded.resolved_at",
    )
    .bind(source)
    .bind(media_type)
    .bind(local_key)
    .bind(external_id)
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

/// One row of `metadata_cache`, as the per-item path reads it.
///
/// A named `FromRow` rather than a tuple: sqlx is used without compile-time
/// macros here, so a tuple would bind by position and a column inserted in the
/// middle of the list would fail at run time, on whichever path happened to run
/// it. `FromRow` maps by name.
#[derive(Debug, sqlx::FromRow)]
pub struct CacheRow {
    pub genres: String,
    pub keywords: String,
    pub original_language: Option<String>,
    pub origin_countries: String,
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// The columns a rule can actually be written against.
///
/// `MetadataField` names five: genres, keywords, original language, origin
/// countries, certification. The status, the synopsis and the poster are
/// matchable by no condition, and the routing pass never opens them. A TMDb
/// overview runs several times longer than an AniList one, so reading them in
/// `load_cache` would load, for nothing, a share of the cache that grows with
/// every source an operator adds.
///
/// The per-item path keeps `CACHE_COLUMNS`: the explanation panel shows all
/// three, and one row is not worth a second query to trim.
pub const EVALUATED_COLUMNS: &str = "source, external_id, media_type, genres, keywords,
     original_language, origin_countries, certification";

/// What one row answers with. The three key columns are not among them: the
/// only reader addresses a row by them and never reads them back.
pub const CACHE_COLUMNS: &str = "genres, keywords, original_language, origin_countries,
     certification, status, overview, poster_path";

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
            status: self.status,
            overview: self.overview,
            poster_path: self.poster_path,
        }
    }
}

/// Every cached answer, keyed by `(source, external id, media type)`.
///
/// Loaded whole, once per simulation: the alternative is one query per item and
/// per source, which the query count in `tests/scale.rs` refuses.
pub async fn load_cache(
    pool: &SqlitePool,
) -> AppResult<std::collections::HashMap<(String, String, String), ProviderMetadata>> {
    /// The evaluated columns and nothing else. A named `FromRow` for the same
    /// reason `CacheRow` is one: sqlx has no compile-time macros here, so a
    /// tuple would bind by position.
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
    }

    let rows: Vec<EvaluatedRow> =
        sqlx::query_as(AssertSqlSafe(format!("SELECT {EVALUATED_COLUMNS} FROM metadata_cache")))
            .fetch_all(pool)
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
                // Not loaded, because no condition can read them. The per-item
                // path is where the panel gets them.
                status: None,
                overview: None,
                poster_path: None,
            }
            .into_answer();
            ((row.source, row.external_id, row.media_type), answer)
        })
        .collect())
}

#[cfg(test)]
mod tests {
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

    /// A source may spell a title without its accents, or with them where the
    /// library has none, and the rules already read both as one value. A search
    /// that compared them strictly would leave the work unresolved.
    #[test]
    fn a_title_matches_its_candidate_whatever_its_accents() {
        let candidates = || {
            vec![(
                "jikan-1".to_string(),
                Some(2001),
                vec!["Le Fabuleux Destin d'Amelie Poulain".to_string()],
            )]
        };
        assert_eq!(
            pick_candidate("Le Fabuleux Destin d'Amélie Poulain", Some(2001), candidates()),
            Some("jikan-1".to_string())
        );
        assert_eq!(
            local_key_of(None, None, None, "Amélie", Some(2001)),
            local_key_of(None, None, None, "Amelie", Some(2001))
        );
    }

    #[test]
    fn adding_tmdb_covers_every_field() {
        let fields = covered_fields(&[info(ARR).unwrap(), info(TMDB).unwrap()]);
        assert_eq!(fields.len(), 5);
    }
}
