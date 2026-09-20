use serde::{Deserialize, Serialize};

use super::MetadataField;

/// Rule media type scope.
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Every condition must match (default, and the safest).
    #[default]
    All,
    /// At least one condition must match — "musique OR keyword concert".
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
        match s.to_lowercase().as_str() {
            "all" | "and" => Ok(MatchMode::All),
            "any" | "or" => Ok(MatchMode::Any),
            _ => Err(format!("Invalid match_mode '{s}': expected all or any")),
        }
    }
}

/// A single condition that can be evaluated against media metadata.
///
/// Serialized as `{"type": "genre_contains", "value": [...]}`. Adding a variant
/// means updating `evaluate_single_condition` and the rule builder UI too.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum Condition {
    /// Genres must contain at least one of these values.
    ///
    /// The `_all` counterpart of each pair below is the same question with the
    /// other quantifier, and a separate variant rather than a field beside the
    /// values: the enum is serialised `{"type": …, "value": …}`, so a sibling
    /// key would change the shape of every condition already on disk.
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

    /// On-disk size, in gigabytes, strictly above this value.
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

    /// Current root folder path matches exactly.
    #[serde(rename = "current_root_folder")]
    CurrentRootFolder(String),

    /// Current root folder path starts with this prefix.
    #[serde(rename = "current_root_folder_starts_with")]
    CurrentRootFolderStartsWith(String),

    /// Whether the media has files downloaded.
    #[serde(rename = "has_files")]
    HasFiles(bool),

    /// Title contains one of these substrings (case-insensitive).
    #[serde(rename = "title_contains")]
    TitleContains(Vec<String>),

    /// Exception by external identifier — the "override by identifier" case.
    #[serde(rename = "tmdb_id_in")]
    TmdbIdIn(Vec<i64>),

    #[serde(rename = "tvdb_id_in")]
    TvdbIdIn(Vec<i64>),

    #[serde(rename = "imdb_id_in")]
    ImdbIdIn(Vec<String>),

    /// Media added to the Arr instance within the last N days — lets a rule
    /// target new additions without touching the existing library.
    #[serde(rename = "added_within_days")]
    AddedWithinDays(i64),

    /// Monitoring flag in Radarr/Sonarr.
    #[serde(rename = "monitored")]
    Monitored(bool),

    /// Whether external metadata could be resolved at all.
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
            Condition::YearRange { min, max } => min.is_none() && max.is_none(),
            Condition::TagIn(v) | Condition::TagInAll(v) | Condition::SeriesTypeIs(v) => {
                v.iter().all(|s| s.trim().is_empty())
            }
            // A threshold of zero is meaningless rather than empty: every media
            // with any size at all is "over 0 GB".
            Condition::SizeOnDiskOverGb(v) | Condition::SeasonCountOver(v) => *v <= 0,
            Condition::HasFiles(_)
            | Condition::Monitored(_)
            | Condition::HasMetadata(_)
            | Condition::AddedWithinDays(_) => false,
        }
    }

    /// The metadata field this condition reads, if any.
    ///
    /// Naming the field rather than answering "does this need TMDb?" is what
    /// lets Routarr tell the user *which* source is missing: with Radarr alone
    /// enabled, a genre rule is fine and a keyword rule cannot match, and a
    /// single "does this need TMDb?" answer would warn about both alike.
    pub fn metadata_field(&self) -> Option<MetadataField> {
        Some(match self {
            Condition::GenreContains(_)
            | Condition::GenreContainsAll(_)
            | Condition::GenreNotContains(_) => MetadataField::Genres,
            Condition::KeywordContains(_)
            | Condition::KeywordContainsAll(_)
            | Condition::KeywordNotContains(_) => MetadataField::Keywords,
            Condition::OriginalLanguage(_) | Condition::OriginalLanguageNot(_) => {
                MetadataField::OriginalLanguage
            }
            Condition::OriginCountry(_) | Condition::OriginCountryAll(_) => {
                MetadataField::OriginCountries
            }
            Condition::CertificationIn(_) => MetadataField::Certification,
            // Named rather than left to a wildcard: these read the media row,
            // and a condition added without a decision about which metadata
            // field it needs would fall through to `None` — silently exempt
            // from the warning that tells the reader no enabled source can
            // answer it. Spelled out, the compiler asks.
            Condition::YearRange { .. }
            | Condition::TagIn(_)
            | Condition::TagInAll(_)
            | Condition::SeriesTypeIs(_)
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub priority: i64,
    pub enabled: bool,
    pub media_type: String,
    pub conditions: Vec<Condition>,
    /// Conditions that disqualify the rule even when `conditions` match.
    #[serde(default)]
    pub exclusions: Vec<Condition>,
    #[serde(default)]
    pub match_mode: MatchMode,
    pub target_category: String,
    pub instance_ids: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

impl Rule {
    /// Whether this rule may apply to the given media type.
    pub fn covers_media_type(&self, media_type: &str) -> bool {
        self.media_type == "both" || self.media_type == media_type
    }

    /// Whether this rule is scoped to the given instance.
    pub fn covers_instance(&self, instance_id: &str) -> bool {
        match &self.instance_ids {
            None => true,
            Some(ids) => ids.is_empty() || ids.iter().any(|id| id == instance_id),
        }
    }
}

/// Request body for creating/updating a rule.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateRuleRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
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
#[derive(Debug, Deserialize)]
pub struct ReorderRulesRequest {
    /// Ordered list of rule IDs, from highest priority (index 0) to lowest.
    pub rule_ids: Vec<String>,
}

/// Portable rule bundle, produced by `GET /rules/export`.
#[derive(Debug, Serialize, Deserialize)]
pub struct RuleBundle {
    pub version: u32,
    #[serde(default)]
    pub exported_at: Option<String>,
    pub rules: Vec<CreateRuleRequest>,
    /// Categories referenced by the rules, so an import can recreate them.
    #[serde(default)]
    pub categories: Vec<String>,
}

/// Request body for `POST /rules/import`.
#[derive(Debug, Deserialize)]
pub struct ImportRulesRequest {
    pub bundle: RuleBundle,
    /// Delete every existing rule first instead of appending.
    #[serde(default)]
    pub replace: bool,
    /// Create any missing category referenced by the bundle.
    #[serde(default = "default_true")]
    pub create_missing_categories: bool,
}

/// A problem found while validating a rule before it is stored or enabled.
///
/// Carries a translation key rather than prose so the validator stays free of
/// wording; `message` is filled in at the API boundary.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationIssue {
    /// `error` blocks the write, `warning` is advisory.
    pub severity: String,
    pub field: String,
    pub key: String,
    pub params: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub message: String,
}

impl ValidationIssue {
    fn new(severity: &str, field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self {
            severity: severity.into(),
            field: field.into(),
            key: key.into(),
            params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            message: String::new(),
        }
    }

    pub fn error(field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self::new("error", field, key, params)
    }

    pub fn warning(field: &str, key: &str, params: &[(&str, String)]) -> Self {
        Self::new("warning", field, key, params)
    }

    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }
}

fn default_priority() -> i64 {
    100
}
fn default_true() -> bool {
    true
}
