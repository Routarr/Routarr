use serde::{Deserialize, Serialize};

use super::MetadataField;

/// Which titles a rule reads: films, series, or both.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleMediaType {
    Movie,
    Series,
    Both,
}

impl std::fmt::Display for RuleMediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleMediaType::Movie => write!(f, "movie"),
            RuleMediaType::Series => write!(f, "series"),
            RuleMediaType::Both => write!(f, "both"),
        }
    }
}

impl std::str::FromStr for RuleMediaType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "movie" => Ok(RuleMediaType::Movie),
            "series" => Ok(RuleMediaType::Series),
            "both" => Ok(RuleMediaType::Both),
            _ => Err(format!("Invalid media_type '{s}': expected movie, series or both")),
        }
    }
}

/// How the conditions of a rule combine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Every condition must match (default, and the safest).
    #[default]
    All,
    /// At least one condition must match, as in "genre Music OR keyword concert".
    Any,
}

impl std::fmt::Display for MatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchMode::All => write!(f, "all"),
            MatchMode::Any => write!(f, "any"),
        }
    }
}

impl std::str::FromStr for MatchMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(MatchMode::All),
            "any" => Ok(MatchMode::Any),
            _ => Err(format!("Invalid match_mode '{s}': expected all or any")),
        }
    }
}

/// One condition of a rule, written `{"type": "genre_contains", "value":
/// ["Animation"]}`. `GET /rules/conditions` lists each `type` with the shape
/// of its `value`.
// A new variant also needs a `CONDITIONS` entry and a `ConditionLabel` key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(tag = "type", content = "value", deny_unknown_fields)]
pub enum Condition {
    /// Genres must contain at least one of these values.
    // Each `_all` twin is its own variant: a sibling key would reshape every stored condition.
    #[serde(rename = "genre_contains")]
    GenreContains(Vec<String>),

    /// Genres must contain every one of these values.
    #[serde(rename = "genre_contains_all")]
    GenreContainsAll(Vec<String>),

    /// Genres must NOT contain any of these values.
    #[serde(rename = "genre_not_contains")]
    GenreNotContains(Vec<String>),

    /// Keywords must contain at least one of these values.
    #[serde(rename = "keyword_contains")]
    KeywordContains(Vec<String>),

    /// Keywords must contain every one of these values.
    #[serde(rename = "keyword_contains_all")]
    KeywordContainsAll(Vec<String>),

    /// Keywords must NOT contain any of these values.
    #[serde(rename = "keyword_not_contains")]
    KeywordNotContains(Vec<String>),

    /// Original language must be one of these (ISO 639-1).
    #[serde(rename = "original_language")]
    OriginalLanguage(Vec<String>),

    /// Original language must NOT be one of these (ISO 639-1).
    #[serde(rename = "original_language_not")]
    OriginalLanguageNot(Vec<String>),

    /// Whether any source knows the original language. A negative condition
    /// matches a title of unknown language, and this tells it apart.
    #[serde(rename = "original_language_known")]
    OriginalLanguageKnown(bool),

    /// Origin country must be one of these (ISO 3166-1).
    #[serde(rename = "origin_country")]
    OriginCountry(Vec<String>),

    /// Every one of these countries must be among the origins.
    #[serde(rename = "origin_country_all")]
    OriginCountryAll(Vec<String>),

    /// Year must be within this range.
    #[serde(rename = "year_range")]
    YearRange { min: Option<i64>, max: Option<i64> },

    /// One of the tags the user attached in the Arr.
    ///
    /// The only signal that carries the user's own intent, which no external
    /// metadata provider can supply.
    #[serde(rename = "tag_in")]
    TagIn(Vec<String>),

    /// Every one of these tags must be attached in the Arr.
    #[serde(rename = "tag_in_all")]
    TagInAll(Vec<String>),

    /// Sonarr's series type (`standard` / `anime` / `daily`).
    #[serde(rename = "series_type_is")]
    SeriesTypeIs(Vec<String>),

    /// The Arr's quality profile is one of these, by name.
    #[serde(rename = "quality_profile_in")]
    QualityProfileIn(Vec<String>),

    /// Runtime in minutes within this range: a film's, a series' episode's.
    #[serde(rename = "runtime_range")]
    RuntimeRange { min: Option<i64>, max: Option<i64> },

    /// Radarr's studio is one of these.
    #[serde(rename = "studio_in")]
    StudioIn(Vec<String>),

    /// Sonarr's network is one of these.
    #[serde(rename = "network_in")]
    NetworkIn(Vec<String>),

    /// Radarr's collection is one of these, by its title.
    #[serde(rename = "collection_in")]
    CollectionIn(Vec<String>),

    /// A score on one scale within this range, with at least `min_votes` votes where given.
    /// The scales are `imdb`, `tmdb` and `mal` out of 10, with votes, `metacritic`,
    /// `rotten_tomatoes` and `anilist` out of 100.
    #[serde(rename = "score_range")]
    ScoreRange {
        scale: String,
        min: Option<f64>,
        max: Option<f64>,
        #[serde(default)]
        min_votes: Option<i64>,
    },

    /// On-disk size, in GB of 1024³ bytes as the interface counts them,
    /// strictly above this value.
    #[serde(rename = "size_on_disk_over_gb")]
    SizeOnDiskOverGb(i64),

    /// Number of seasons, excluding specials, strictly above this value.
    #[serde(rename = "season_count_over")]
    SeasonCountOver(i64),

    /// Certification must be one of these.
    #[serde(rename = "certification_in")]
    CertificationIn(Vec<String>),

    /// Status must be one of these.
    #[serde(rename = "status_is")]
    StatusIs(Vec<String>),

    /// The current root folder is this folder, whatever its trailing
    /// separator and, on Windows, its case.
    #[serde(rename = "current_root_folder")]
    CurrentRootFolder(String),

    /// The current root folder is this folder or one inside it: `/data` holds
    /// `/data/movies`, and `/data/mov` holds neither.
    #[serde(rename = "current_root_folder_starts_with")]
    CurrentRootFolderStartsWith(String),

    /// Whether the media has files downloaded.
    #[serde(rename = "has_files")]
    HasFiles(bool),

    /// The title contains one of these, compared as every value is: case,
    /// accents and punctuation aside.
    #[serde(rename = "title_contains")]
    TitleContains(Vec<String>),

    /// The title's TMDB id is one of these.
    #[serde(rename = "tmdb_id_in")]
    TmdbIdIn(Vec<i64>),

    /// The title's TheTVDB id is one of these.
    #[serde(rename = "tvdb_id_in")]
    TvdbIdIn(Vec<i64>),

    /// The title's IMDb id is one of these, written `tt0096283`.
    #[serde(rename = "imdb_id_in")]
    ImdbIdIn(Vec<String>),

    /// Media added to the Arr instance within the last N days, N times 24
    /// hours, so a rule can target new additions without touching the
    /// existing library. An Arr clock less than a day ahead reads as just
    /// added. At least 1.
    #[serde(rename = "added_within_days")]
    AddedWithinDays(i64),

    /// Monitoring flag in Radarr/Sonarr.
    #[serde(rename = "monitored")]
    Monitored(bool),

    /// Whether any source, the Arr included, supplied a field a condition reads.
    #[serde(rename = "has_metadata")]
    HasMetadata(bool),
}

impl Condition {
    /// Stable machine-readable discriminator, used by the UI and by validation.
    pub fn kind(&self) -> &'static str {
        match self {
            Condition::GenreContains(_) => "genre_contains",
            Condition::GenreContainsAll(_) => "genre_contains_all",
            Condition::GenreNotContains(_) => "genre_not_contains",
            Condition::KeywordContains(_) => "keyword_contains",
            Condition::KeywordContainsAll(_) => "keyword_contains_all",
            Condition::KeywordNotContains(_) => "keyword_not_contains",
            Condition::OriginalLanguage(_) => "original_language",
            Condition::OriginalLanguageNot(_) => "original_language_not",
            Condition::OriginCountry(_) => "origin_country",
            Condition::OriginCountryAll(_) => "origin_country_all",
            Condition::YearRange { .. } => "year_range",
            Condition::TagIn(_) => "tag_in",
            Condition::TagInAll(_) => "tag_in_all",
            Condition::SeriesTypeIs(_) => "series_type_is",
            Condition::QualityProfileIn(_) => "quality_profile_in",
            Condition::RuntimeRange { .. } => "runtime_range",
            Condition::StudioIn(_) => "studio_in",
            Condition::NetworkIn(_) => "network_in",
            Condition::CollectionIn(_) => "collection_in",
            Condition::ScoreRange { .. } => "score_range",
            Condition::SizeOnDiskOverGb(_) => "size_on_disk_over_gb",
            Condition::SeasonCountOver(_) => "season_count_over",
            Condition::CertificationIn(_) => "certification_in",
            Condition::StatusIs(_) => "status_is",
            Condition::CurrentRootFolder(_) => "current_root_folder",
            Condition::CurrentRootFolderStartsWith(_) => "current_root_folder_starts_with",
            Condition::HasFiles(_) => "has_files",
            Condition::TitleContains(_) => "title_contains",
            Condition::TmdbIdIn(_) => "tmdb_id_in",
            Condition::TvdbIdIn(_) => "tvdb_id_in",
            Condition::ImdbIdIn(_) => "imdb_id_in",
            Condition::AddedWithinDays(_) => "added_within_days",
            Condition::Monitored(_) => "monitored",
            Condition::HasMetadata(_) => "has_metadata",
            Condition::OriginalLanguageKnown(_) => "original_language_known",
        }
    }

    /// True when the condition can never match because it carries no operand.
    pub fn is_empty(&self) -> bool {
        match self {
            Condition::GenreContains(v)
            | Condition::GenreContainsAll(v)
            | Condition::GenreNotContains(v)
            | Condition::KeywordContains(v)
            | Condition::KeywordContainsAll(v)
            | Condition::KeywordNotContains(v)
            | Condition::OriginalLanguage(v)
            | Condition::OriginalLanguageNot(v)
            | Condition::OriginCountry(v)
            | Condition::OriginCountryAll(v)
            | Condition::CertificationIn(v)
            | Condition::StatusIs(v)
            | Condition::TitleContains(v)
            | Condition::ImdbIdIn(v) => v.iter().all(|s| s.trim().is_empty()),
            Condition::TmdbIdIn(v) | Condition::TvdbIdIn(v) => v.is_empty(),
            Condition::CurrentRootFolder(s) | Condition::CurrentRootFolderStartsWith(s) => {
                s.trim().is_empty()
            }
            Condition::YearRange { min, max } | Condition::RuntimeRange { min, max } => {
                min.is_none() && max.is_none()
            }
            Condition::ScoreRange { min, max, min_votes, .. } => {
                min.is_none() && max.is_none() && min_votes.is_none()
            }
            Condition::TagIn(v)
            | Condition::TagInAll(v)
            | Condition::SeriesTypeIs(v)
            | Condition::QualityProfileIn(v)
            | Condition::StudioIn(v)
            | Condition::NetworkIn(v)
            | Condition::CollectionIn(v) => v.iter().all(|s| s.trim().is_empty()),
            // A count always holds a value, too low a one being another fault.
            Condition::SizeOnDiskOverGb(_)
            | Condition::SeasonCountOver(_)
            | Condition::HasFiles(_)
            | Condition::Monitored(_)
            | Condition::HasMetadata(_)
            | Condition::OriginalLanguageKnown(_)
            | Condition::AddedWithinDays(_) => false,
        }
    }

    /// The metadata field this condition reads, if any, so a warning can name
    /// the field no enabled source answers.
    pub fn metadata_field(&self) -> Option<MetadataField> {
        Some(match self {
            Condition::GenreContains(_)
            | Condition::GenreContainsAll(_)
            | Condition::GenreNotContains(_) => MetadataField::Genres,
            Condition::KeywordContains(_)
            | Condition::KeywordContainsAll(_)
            | Condition::KeywordNotContains(_) => MetadataField::Keywords,
            Condition::OriginalLanguage(_)
            | Condition::OriginalLanguageNot(_)
            | Condition::OriginalLanguageKnown(_) => MetadataField::OriginalLanguage,
            Condition::OriginCountry(_) | Condition::OriginCountryAll(_) => {
                MetadataField::OriginCountries
            }
            Condition::CertificationIn(_) => MetadataField::Certification,
            Condition::ScoreRange { .. } => MetadataField::Ratings,
            // No wildcard, so the compiler asks a new condition which field it reads.
            Condition::YearRange { .. }
            | Condition::TagIn(_)
            | Condition::TagInAll(_)
            | Condition::SeriesTypeIs(_)
            | Condition::QualityProfileIn(_)
            | Condition::RuntimeRange { .. }
            | Condition::StudioIn(_)
            | Condition::NetworkIn(_)
            | Condition::CollectionIn(_)
            | Condition::SizeOnDiskOverGb(_)
            | Condition::SeasonCountOver(_)
            | Condition::StatusIs(_)
            | Condition::CurrentRootFolder(_)
            | Condition::CurrentRootFolderStartsWith(_)
            | Condition::HasFiles(_)
            | Condition::TitleContains(_)
            | Condition::TmdbIdIn(_)
            | Condition::TvdbIdIn(_)
            | Condition::ImdbIdIn(_)
            | Condition::AddedWithinDays(_)
            | Condition::Monitored(_)
            | Condition::HasMetadata(_) => return None,
        })
    }
}

/// A routing rule that maps conditions to a target category.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Lower is tried first. Equal priorities go by name, then by id.
    #[schema(minimum = -1000000, maximum = 1000000)]
    pub priority: i64,
    pub enabled: bool,
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["movie", "series", "both"]))))]
    pub media_type: RuleMediaType,
    pub conditions: Vec<Condition>,
    /// Conditions that disqualify the rule even when `conditions` match.
    #[serde(default)]
    pub exclusions: Vec<Condition>,
    #[serde(default)]
    pub match_mode: MatchMode,
    pub target_category: String,
    pub instance_ids: Option<Vec<String>>,
    /// The template and version the rule was added from, as `children@1`. Null
    /// for a rule written by hand.
    #[serde(default)]
    pub template: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub updated_at: String,
}

impl Rule {
    pub fn covers_media_type(&self, media_type: &str) -> bool {
        match self.media_type {
            RuleMediaType::Both => true,
            RuleMediaType::Movie => media_type == "movie",
            RuleMediaType::Series => media_type == "series",
        }
    }

    /// No instance list, or an empty one, covers every instance.
    pub fn covers_instance(&self, instance_id: &str) -> bool {
        match &self.instance_ids {
            None => true,
            Some(ids) => ids.is_empty() || ids.iter().any(|id| id == instance_id),
        }
    }
}

/// A rule as written: what `POST /rules`, `PUT /rules/{id}`, `/rules/validate`
/// and `/rules/preview` take.
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRuleRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Lower is tried first. Equal priorities go by name, then by id. Left
    /// out, a new rule goes after every other, and an edited one keeps its
    /// place.
    // The bounds are `rule_engine::MIN_PRIORITY` and `MAX_PRIORITY`.
    #[serde(default)]
    #[schema(minimum = -1000000, maximum = 1000000)]
    pub priority: Option<i64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub media_type: String,
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub exclusions: Vec<Condition>,
    #[serde(default)]
    pub match_mode: MatchMode,
    pub target_category: String,
    #[serde(default)]
    pub instance_ids: Option<Vec<String>>,
}

/// Request for reordering rules.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderRulesRequest {
    /// Ordered list of rule IDs, from highest priority (index 0) to lowest.
    pub rule_ids: Vec<String>,
}

/// Portable rule bundle, produced by `GET /rules/export`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleBundle {
    /// The format, which this build writes as 2. A version 1 bundle names no
    /// instance a rule is limited to, and its rules without instance ids are
    /// imported switched off.
    pub version: u32,
    /// When it was written, in RFC 3339.
    #[serde(default, serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub exported_at: Option<String>,
    pub rules: Vec<BundledRule>,
    /// Categories referenced by the rules, so an import can recreate them.
    #[serde(default)]
    pub categories: Vec<String>,
}

/// A rule as a bundle carries it, its instance scope by name: an id means
/// nothing on another installation, and a scope dropped would let the rule
/// route every instance.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BundledRule {
    #[serde(flatten)]
    pub rule: CreateRuleRequest,
    /// The instances the rule is limited to, by name. Absent for every
    /// instance. A rule naming none this installation has is not imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_names: Option<Vec<String>>,
    /// The template and version the rule was added from, as `children@1`.
    /// Absent for a rule written by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

impl BundledRule {
    /// The template marker, when it has the shape one has: a template's id,
    /// `@` and a version.
    pub fn marker(&self) -> Option<&str> {
        let marker = self.template.as_deref()?;
        let (id, version) = marker.split_once('@')?;
        let id_ok = !id.is_empty()
            && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let version_ok = !version.is_empty() && version.chars().all(|c| c.is_ascii_digit());
        (id_ok && version_ok).then_some(marker)
    }
}

/// Request body for `POST /rules/import`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRulesRequest {
    pub bundle: RuleBundle,
    /// Delete every existing rule first instead of appending. Appended, the bundle's rules go
    /// after every rule in place, in the bundle's order, and one a rule in place already routes
    /// as is left out.
    #[serde(default)]
    pub replace: bool,
    /// Create any missing category referenced by the bundle.
    #[serde(default = "default_true")]
    pub create_missing_categories: bool,
}

/// How much a problem with a rule weighs.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks the write.
    Error,
    /// Advisory.
    Warning,
}

/// A problem with a rule: a stable `key` and its values, and the `message`
/// in the interface language, which is not part of the contract.
// `message` is filled in at the API boundary, so the validator stays free of wording.
#[derive(Debug, Clone, Serialize, PartialEq, utoipa::ToSchema)]
pub struct ValidationIssue {
    #[schema(value_type = String, extensions(("x-extensible-enum" = json!(["error", "warning"]))))]
    pub severity: Severity,
    pub field: String,
    pub key: String,
    pub params: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub message: String,
}

impl ValidationIssue {
    fn new(severity: Severity, field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self {
            severity,
            field: field.into(),
            key: key.into(),
            params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            message: String::new(),
        }
    }

    pub fn error(field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self::new(Severity::Error, field, key, params)
    }

    pub fn warning(field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self::new(Severity::Warning, field, key, params)
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

fn default_true() -> bool {
    true
}
