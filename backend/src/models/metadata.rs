//! Metadata as one source answers it (`ProviderMetadata`) and as the rule engine reads it
//! (`MediaMetadata`): every source merged field by field, with the source that won each field.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The fields a source can supply, named so a rule can be told which of them
/// nothing currently provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataField {
    Genres,
    Keywords,
    OriginalLanguage,
    OriginCountries,
    Certification,
    Ratings,
}

impl MetadataField {
    /// Every field a condition can read, which `merge` and `api::media::metadata_predicate`
    /// iterate, so a new variant belongs here too.
    pub const ALL: [MetadataField; 6] = [
        Self::Genres,
        Self::Keywords,
        Self::OriginalLanguage,
        Self::OriginCountries,
        Self::Certification,
        Self::Ratings,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Genres => "genres",
            Self::Keywords => "keywords",
            Self::OriginalLanguage => "original_language",
            Self::OriginCountries => "origin_countries",
            Self::Certification => "certification",
            Self::Ratings => "ratings",
        }
    }
}

/// A score on one scale, and how many voted for it where the scale counts votes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Rating {
    pub value: f64,
    pub votes: Option<i64>,
}

/// A scale a score is given on. An IMDb 7 and a MyAnimeList 7 are not one thing, so a rule
/// names its scale and no two are merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    pub id: &'static str,
    pub name: &'static str,
    /// The highest score: 10 or 100.
    pub max: i64,
    /// Whether its answers say how many voted.
    pub votes: bool,
}

pub const SCALES: &[Scale] = &[
    Scale { id: "imdb", name: "IMDb", max: 10, votes: true },
    Scale { id: "tmdb", name: "TMDB", max: 10, votes: true },
    Scale { id: "metacritic", name: "Metacritic", max: 100, votes: false },
    Scale { id: "rotten_tomatoes", name: "Rotten Tomatoes", max: 100, votes: false },
    Scale { id: "mal", name: "MyAnimeList", max: 10, votes: true },
    Scale { id: "anilist", name: "AniList", max: 100, votes: false },
];

pub fn scale(id: &str) -> Option<&'static Scale> {
    SCALES.iter().find(|scale| scale.id == id)
}

/// A score as a source writes it, kept when it is one: a source answers 0 for a title nobody
/// rated, and a scale it does not count votes on gets none.
pub fn rating(scale_id: &str, value: Option<f64>, votes: Option<i64>) -> Option<(String, Rating)> {
    let scale = scale(scale_id)?;
    let value =
        value.filter(|value| value.is_finite() && *value > 0.0 && *value <= scale.max as f64)?;
    let votes = if scale.votes { votes.filter(|votes| *votes > 0) } else { None };
    Some((scale_id.to_string(), Rating { value, votes }))
}

/// One source's answer about one media item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderMetadata {
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub original_language: Option<String>,
    #[serde(default)]
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
    /// The system `certification` belongs to: a country code, or `MAL`.
    #[serde(default)]
    pub certification_scale: Option<String>,
    /// Every country's rating, keyed by upper-case country code. The certification regions pick
    /// one when the answer is read, so a change of regions needs no new request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub certifications: BTreeMap<String, String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    /// Each scale's score, keyed by scale id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ratings: BTreeMap<String, Rating>,
}

/// What every enabled source, taken in priority order, adds up to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MediaMetadata {
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub original_language: Option<String>,
    #[serde(default)]
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
    /// The system the certification belongs to: an ISO 3166-1 country code,
    /// or `MAL` for MyAnimeList's. Absent when the source did not say.
    #[serde(default)]
    pub certification_scale: Option<String>,
    /// Where the title stands, in the words Radarr and Sonarr use: `tba`,
    /// `announced`, `inCinemas` or `released` for a film, `upcoming`,
    /// `continuing` or `ended` for a series.
    pub status: Option<String>,
    /// The synopsis of the first source in the order that gives one.
    pub overview: Option<String>,
    /// Each scale's score, keyed by `imdb`, `tmdb`, `metacritic`, `rotten_tomatoes`, `mal` or
    /// `anilist`, from the first source in the order that gives that scale.
    #[serde(default)]
    pub ratings: BTreeMap<String, Rating>,
    /// Field name -> the source that supplied it. Only populated fields appear.
    #[serde(default)]
    pub field_sources: BTreeMap<String, String>,
    /// Sources that supplied at least one field, highest priority first.
    #[serde(default)]
    pub sources: Vec<String>,
}

impl MediaMetadata {
    /// Collapse the sources, highest priority first: per field, the first source with a value
    /// keeps it. `None` when no source knew a field a condition reads, so `has_metadata` means
    /// something matchable is known.
    pub fn merge<'a, I>(parts: I) -> Option<Self>
    where
        I: IntoIterator<Item = (&'a str, ProviderMetadata)>,
    {
        let mut merged = Self::default();

        for (source, part) in parts {
            let before = merged.field_sources.len();

            take_list(&mut merged.genres, part.genres, source, "genres", &mut merged.field_sources);
            take_list(
                &mut merged.keywords,
                part.keywords,
                source,
                "keywords",
                &mut merged.field_sources,
            );
            take_list(
                &mut merged.origin_countries,
                part.origin_countries,
                source,
                "origin_countries",
                &mut merged.field_sources,
            );
            take_value(
                &mut merged.original_language,
                part.original_language,
                source,
                "original_language",
                &mut merged.field_sources,
            );
            // Before `certification` is taken: the scale goes with the certification it qualifies.
            if merged.certification.is_none() && part.certification.is_some() {
                merged.certification_scale = part.certification_scale;
            }
            take_value(
                &mut merged.certification,
                part.certification,
                source,
                "certification",
                &mut merged.field_sources,
            );
            take_value(
                &mut merged.status,
                part.status,
                source,
                "status",
                &mut merged.field_sources,
            );
            take_value(
                &mut merged.overview,
                part.overview,
                source,
                "overview",
                &mut merged.field_sources,
            );
            // Scale by scale, each named in the sources as `ratings.<scale>`.
            for (scale, rating) in part.ratings {
                if merged.ratings.contains_key(&scale) {
                    continue;
                }
                merged.field_sources.insert(format!("ratings.{scale}"), source.to_string());
                merged
                    .field_sources
                    .entry("ratings".to_string())
                    .or_insert_with(|| source.to_string());
                merged.ratings.insert(scale, rating);
            }

            // A source may come twice, the Arr's English being offered last.
            if merged.field_sources.len() > before && !merged.sources.iter().any(|s| s == source) {
                merged.sources.push(source.to_string());
            }
        }

        // A synopsis or a status alone is no metadata: no condition reads either.
        let matchable = MetadataField::ALL
            .iter()
            .any(|field| merged.field_sources.contains_key(field.as_str()));
        if matchable { Some(merged) } else { None }
    }

    /// The source a given field came from, for the explanation.
    pub fn source_of(&self, field: &str) -> Option<&str> {
        self.field_sources.get(field).map(String::as_str)
    }
}

fn filled(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| !v.trim().is_empty())
}

fn take_list(
    slot: &mut Vec<String>,
    incoming: Vec<String>,
    source: &str,
    field: &str,
    sources: &mut BTreeMap<String, String>,
) {
    if !slot.is_empty() {
        return;
    }
    let cleaned: Vec<String> =
        incoming.into_iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
    if cleaned.is_empty() {
        return;
    }
    *slot = cleaned;
    sources.insert(field.to_string(), source.to_string());
}

fn take_value(
    slot: &mut Option<String>,
    incoming: Option<String>,
    source: &str,
    field: &str,
    sources: &mut BTreeMap<String, String>,
) {
    if filled(slot) {
        return;
    }
    let Some(value) = incoming.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
        return;
    };
    *slot = Some(value);
    sources.insert(field.to_string(), source.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_scale_comes_from_the_first_source_that_gives_it() {
        let scored = |pairs: &[(&str, f64, Option<i64>)]| ProviderMetadata {
            ratings: pairs
                .iter()
                .filter_map(|(scale, value, votes)| rating(scale, Some(*value), *votes))
                .collect(),
            ..Default::default()
        };
        let merged = MediaMetadata::merge([
            ("arr", scored(&[("imdb", 8.1, Some(393_000))])),
            ("tmdb", scored(&[("tmdb", 7.9, Some(1200))])),
            ("omdb", scored(&[("imdb", 8.0, Some(400_000)), ("metacritic", 86.0, None)])),
        ])
        .expect("scores are metadata a rule reads");

        assert_eq!(merged.ratings["imdb"], Rating { value: 8.1, votes: Some(393_000) });
        assert_eq!(merged.source_of("ratings.imdb"), Some("arr"));
        assert_eq!(merged.source_of("ratings.metacritic"), Some("omdb"));
        assert_eq!(merged.source_of("ratings"), Some("arr"));
        assert_eq!(merged.sources, ["arr", "tmdb", "omdb"]);
    }

    #[test]
    fn a_score_off_its_scale_or_of_nobody_is_none() {
        assert_eq!(rating("imdb", Some(0.0), Some(0)), None, "a title nobody rated");
        assert_eq!(rating("imdb", Some(11.0), Some(3)), None);
        assert_eq!(rating("trakt", Some(8.0), Some(3)), None);
        assert_eq!(rating("imdb", None, Some(3)), None);
        let critics = rating("metacritic", Some(86.0), Some(40)).unwrap().1;
        assert_eq!(critics.votes, None, "a scale counting no votes keeps none");
        assert_eq!(rating("tmdb", Some(7.0), Some(0)).unwrap().1.votes, None);
    }

    fn arr() -> ProviderMetadata {
        ProviderMetadata {
            genres: vec!["Animation".into()],
            original_language: Some("ja".into()),
            certification: Some("U".into()),
            ..Default::default()
        }
    }

    fn tmdb() -> ProviderMetadata {
        ProviderMetadata {
            genres: vec!["Fantasy".into()],
            keywords: vec!["anime".into()],
            original_language: Some("jp".into()),
            origin_countries: vec!["JP".into()],
            certification: Some("PG".into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_highest_source_wins_each_field_it_can_answer() {
        let merged = MediaMetadata::merge([("arr", arr()), ("tmdb", tmdb())]).unwrap();

        assert_eq!(merged.genres, vec!["Animation"]);
        assert_eq!(merged.original_language.as_deref(), Some("ja"));
        assert_eq!(merged.certification.as_deref(), Some("U"));
        assert_eq!(merged.source_of("genres"), Some("arr"));
    }

    #[test]
    fn a_lower_source_fills_the_gaps_instead_of_being_ignored() {
        let merged = MediaMetadata::merge([("arr", arr()), ("tmdb", tmdb())]).unwrap();

        assert_eq!(merged.keywords, vec!["anime"]);
        assert_eq!(merged.origin_countries, vec!["JP"]);
        assert_eq!(merged.source_of("keywords"), Some("tmdb"));
        assert_eq!(merged.sources, vec!["arr", "tmdb"]);
    }

    #[test]
    fn reordering_the_sources_reverses_who_wins() {
        let merged = MediaMetadata::merge([("tmdb", tmdb()), ("arr", arr())]).unwrap();

        assert_eq!(merged.genres, vec!["Fantasy"]);
        assert_eq!(merged.source_of("genres"), Some("tmdb"));
        // TMDB answers every field the Arr could, so the Arr adds nothing from below it.
        assert_eq!(merged.sources, vec!["tmdb"]);
    }

    #[test]
    fn a_source_that_answered_nothing_is_not_listed_as_contributing() {
        let merged =
            MediaMetadata::merge([("arr", ProviderMetadata::default()), ("tmdb", tmdb())]).unwrap();

        assert_eq!(merged.sources, vec!["tmdb"]);
    }

    #[test]
    fn nothing_known_is_absent_metadata_not_empty_metadata() {
        assert!(MediaMetadata::merge([("arr", ProviderMetadata::default())]).is_none());
        assert!(MediaMetadata::merge([]).is_none());
    }

    #[test]
    fn blank_values_do_not_claim_a_field_from_the_source_below() {
        let blank = ProviderMetadata {
            genres: vec!["   ".into()],
            certification: Some("  ".into()),
            ..Default::default()
        };
        let merged = MediaMetadata::merge([("arr", blank), ("tmdb", tmdb())]).unwrap();

        assert_eq!(merged.genres, vec!["Fantasy"]);
        assert_eq!(merged.certification.as_deref(), Some("PG"));
        assert_eq!(merged.sources, vec!["tmdb"]);
    }
}
