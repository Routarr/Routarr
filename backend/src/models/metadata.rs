//! Metadata as the engine consumes it, and as each source supplies it.
//!
//! Two shapes on purpose. `ProviderMetadata` is one source's answer, complete
//! or not — Radarr knows the genres but not the keywords, TMDb knows both.
//! `MediaMetadata` is what the rule engine sees: the answers of every enabled
//! source collapsed field by field, plus the record of which source won each
//! field, without which "genre does not contain Animation" becomes impossible
//! to explain when two sources disagree.

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
}

impl MetadataField {
    /// Every field a condition can read. Iterated rather than spelled out
    /// again, so a variant added above cannot be forgotten by the two places
    /// that decide whether an item is *known*.
    pub const ALL: [MetadataField; 5] = [
        Self::Genres,
        Self::Keywords,
        Self::OriginalLanguage,
        Self::OriginCountries,
        Self::Certification,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Genres => "genres",
            Self::Keywords => "keywords",
            Self::OriginalLanguage => "original_language",
            Self::OriginCountries => "origin_countries",
            Self::Certification => "certification",
        }
    }
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
    pub status: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
}

/// What every enabled source, taken in priority order, adds up to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaMetadata {
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub original_language: Option<String>,
    #[serde(default)]
    pub origin_countries: Vec<String>,
    pub certification: Option<String>,
    pub status: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    /// Field name -> the source that supplied it. Only populated fields appear.
    #[serde(default)]
    pub field_sources: BTreeMap<String, String>,
    /// Sources that supplied at least one field, highest priority first.
    #[serde(default)]
    pub sources: Vec<String>,
}

impl MediaMetadata {
    /// Collapse the sources, highest priority first.
    ///
    /// Per field, the first source that has a value keeps it and the ones below
    /// do not overwrite it — a lower-priority source only fills a gap. That is
    /// the whole point of the ordering: with Radarr above TMDb the genres come
    /// from the library, and TMDb still contributes the keywords Radarr has no
    /// notion of. Returns `None` when no source knew anything, so
    /// `has_metadata` keeps meaning "something is known about this item".
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
            take_value(
                &mut merged.poster_path,
                part.poster_path,
                source,
                "poster_path",
                &mut merged.field_sources,
            );

            if merged.field_sources.len() > before {
                merged.sources.push(source.to_string());
            }
        }

        // Known means *matchable*. A source that supplied only a synopsis, a
        // status or a poster is still listed — it did answer, and the panel
        // says so — but none of the three can be read by any condition, so an
        // item holding nothing else is exactly as blind to the engine as one
        // holding nothing at all. Counting them made the library list say
        // "metadata" about an item no rule could ever touch, and made the two
        // disagree the moment the pass stopped loading them.
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

        // Radarr has no notion of either, so TMDb still contributes them.
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
        // TMDb answers every field the Arr could have answered, so the Arr adds
        // nothing at all from this position — which is the honest reading of
        // "lower priority", not a bug.
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
