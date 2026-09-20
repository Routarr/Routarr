//! The condition vocabulary the rule builder is driven by.
//!
//! Its own module rather than another section of `rules.rs`: this is not a
//! handler over the `rules` table, it is a static description of what a
//! condition can be — its value shape, the metadata field it reads, and the
//! media types it can ever match on. Kept there, `rules.rs` is four unrelated
//! things in one file: CRUD, preview, the import/export bundle, and this.

use super::Json;
use axum::extract::State;

use crate::models::MetadataField;
use crate::services::metadata;
use crate::state::AppState;

/// One entry of the vocabulary.
struct Spec {
    kind: &'static str,
    value_type: &'static str,
    /// The metadata field it reads, if any.
    field: Option<MetadataField>,
    media_types: &'static [&'static str],
    /// The `GET /media/facets` axis its values are drawn from, so the builder
    /// offers what the library holds instead of asking for an exact spelling.
    /// Empty where the values are not a closed set — a keyword, a title
    /// fragment, an external id.
    suggestions: &'static str,
    /// How the values of this condition combine, and the kind that asks the
    /// same question the other way. Only where the media side is a *list*: an
    /// item has one certification and one language, so "all of them" is a
    /// question those cannot be asked.
    ///
    /// The pair is what lets the builder offer one condition with a choice
    /// rather than two that read alike. `counterpart` empty means the condition
    /// has no quantifier at all.
    quantifier: &'static str,
    counterpart: &'static str,
}

/// Machine-readable catalogue of condition types, so the rule builder does not
/// have to hard-code the list that the backend already owns.
pub async fn condition_catalog(State(state): State<AppState>) -> Json<serde_json::Value> {
    let localizer = state.localizer().await;

    // Which media types a condition can ever match on.
    //
    // Not a preference: Radarr's payload carries no `tvdbId`, no `seriesType`
    // and no season list, so `upsert_media` leaves those columns null for every
    // film and the three conditions below can only ever fail there. Offering
    // them on a movie rule is offering a condition that cannot match.
    const ANY: &[&str] = &["movie", "series"];
    const SERIES_ONLY: &[&str] = &["series"];

    // The label comes from the dictionary, keyed as `ConditionLabel` + the
    // PascalCase form of the kind.
    const CONDITIONS: &[Spec] = &[
        Spec {
            kind: "genre_contains",
            value_type: "string_list",
            field: Some(MetadataField::Genres),
            media_types: ANY,
            suggestions: "genres",
            quantifier: "any",
            counterpart: "genre_contains_all",
        },
        Spec {
            kind: "genre_contains_all",
            value_type: "string_list",
            field: Some(MetadataField::Genres),
            media_types: ANY,
            suggestions: "genres",
            quantifier: "all",
            counterpart: "genre_contains",
        },
        Spec {
            kind: "genre_not_contains",
            value_type: "string_list",
            field: Some(MetadataField::Genres),
            media_types: ANY,
            suggestions: "genres",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "keyword_contains",
            value_type: "string_list",
            field: Some(MetadataField::Keywords),
            media_types: ANY,
            suggestions: "",
            quantifier: "any",
            counterpart: "keyword_contains_all",
        },
        Spec {
            kind: "keyword_contains_all",
            value_type: "string_list",
            field: Some(MetadataField::Keywords),
            media_types: ANY,
            suggestions: "",
            quantifier: "all",
            counterpart: "keyword_contains",
        },
        Spec {
            kind: "keyword_not_contains",
            value_type: "string_list",
            field: Some(MetadataField::Keywords),
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "original_language",
            value_type: "string_list",
            field: Some(MetadataField::OriginalLanguage),
            media_types: ANY,
            suggestions: "original_languages",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "original_language_not",
            value_type: "string_list",
            field: Some(MetadataField::OriginalLanguage),
            media_types: ANY,
            suggestions: "original_languages",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "origin_country",
            value_type: "string_list",
            field: Some(MetadataField::OriginCountries),
            media_types: ANY,
            suggestions: "origin_countries",
            quantifier: "any",
            counterpart: "origin_country_all",
        },
        Spec {
            kind: "origin_country_all",
            value_type: "string_list",
            field: Some(MetadataField::OriginCountries),
            media_types: ANY,
            suggestions: "origin_countries",
            quantifier: "all",
            counterpart: "origin_country",
        },
        Spec {
            kind: "certification_in",
            value_type: "string_list",
            field: Some(MetadataField::Certification),
            media_types: ANY,
            suggestions: "certifications",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "year_range",
            value_type: "year_range",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "status_is",
            value_type: "string_list",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "current_root_folder",
            value_type: "string",
            field: None,
            media_types: ANY,
            suggestions: "root_folders",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "current_root_folder_starts_with",
            value_type: "string",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "has_files",
            value_type: "boolean",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "monitored",
            value_type: "boolean",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "has_metadata",
            value_type: "boolean",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "title_contains",
            value_type: "string_list",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "tmdb_id_in",
            value_type: "number_list",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "tvdb_id_in",
            value_type: "number_list",
            field: None,
            media_types: SERIES_ONLY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "imdb_id_in",
            value_type: "string_list",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "added_within_days",
            value_type: "number",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        // Signals the Arr already reports; none of them reads a source.
        Spec {
            kind: "tag_in",
            value_type: "string_list",
            field: None,
            media_types: ANY,
            suggestions: "tags",
            quantifier: "any",
            counterpart: "tag_in_all",
        },
        Spec {
            kind: "tag_in_all",
            value_type: "string_list",
            field: None,
            media_types: ANY,
            suggestions: "tags",
            quantifier: "all",
            counterpart: "tag_in",
        },
        Spec {
            kind: "series_type_is",
            value_type: "string_list",
            field: None,
            media_types: SERIES_ONLY,
            suggestions: "series_types",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "size_on_disk_over_gb",
            value_type: "number",
            field: None,
            media_types: ANY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
        Spec {
            kind: "season_count_over",
            value_type: "number",
            field: None,
            media_types: SERIES_ONLY,
            suggestions: "",
            quantifier: "",
            counterpart: "",
        },
    ];

    // Whether a condition can match depends on which sources are enabled, so
    // the catalogue answers it rather than the interface marking it
    // "(needs TMDb)" once and for all.
    let covered = metadata::covered_fields(&state.metadata_providers().await);

    let conditions: Vec<serde_json::Value> = CONDITIONS
        .iter()
        .map(|spec| {
            let key = format!("ConditionLabel{}", pascal_case(spec.kind));
            serde_json::json!({
                "type": spec.kind,
                "label": localizer.translate(&key, &[]),
                "label_key": key,
                "value_type": spec.value_type,
                "needs_metadata": spec.field.is_some(),
                "metadata_field": spec.field.map(|f| f.as_str()),
                "available": spec.field.is_none_or(|f| covered.contains(&f)),
                "media_types": spec.media_types,
                "suggestions": spec.suggestions,
                "quantifier": spec.quantifier,
                "counterpart": spec.counterpart,
            })
        })
        .collect();

    Json(serde_json::json!({ "match_modes": ["all", "any"], "conditions": conditions }))
}

/// `genre_contains` -> `GenreContains`.
fn pascal_case(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- helpers

#[cfg(test)]
mod tests {
    use crate::models::Condition;

    /// The catalogue, read out of this file's own source.
    ///
    /// `CONDITIONS` is a `const` inside a handler, so nothing outside can see
    /// it. Reading the text is what lets a test compare it against the enum it
    /// is supposed to describe.
    fn catalogue() -> Vec<(String, String, Option<String>)> {
        let source = include_str!("conditions.rs");
        let mut found = Vec::new();
        for spec in source.split("Spec {").skip(1) {
            let Some(body) = spec.split("\n        }").next() else { continue };
            let Some(kind) = between(body, "kind: \"", "\"") else { continue };
            let Some(value_type) = between(body, "value_type: \"", "\"") else { continue };
            let field = between(body, "field: Some(MetadataField::", ")");
            found.push((kind, value_type, field));
        }
        found
    }

    fn between(haystack: &str, open: &str, close: &str) -> Option<String> {
        let start = haystack.find(open)? + open.len();
        let rest = &haystack[start..];
        Some(rest[..rest.find(close)?].to_string())
    }

    /// A value of the shape the catalogue declares, so the kind can be
    /// deserialised into the real `Condition` it names.
    fn sample(value_type: &str) -> serde_json::Value {
        match value_type {
            "string_list" => serde_json::json!(["x"]),
            "number_list" => serde_json::json!([1]),
            "string" => serde_json::json!("x"),
            "number" => serde_json::json!(1),
            "boolean" => serde_json::json!(true),
            "year_range" => serde_json::json!({ "min": 2000, "max": 2010 }),
            other => panic!("no sample for value_type {other}"),
        }
    }

    /// Every condition the engine understands is offered by the builder.
    ///
    /// The catalogue is a `const` in this file and the enum is in another, so
    /// the compiler joins them nowhere: a variant added to `Condition` and
    /// forgotten here evaluates correctly and is offered by nothing, which
    /// looks exactly like a feature that was never built.
    #[test]
    fn every_condition_the_engine_knows_is_in_the_catalogue() {
        let wire = wire_names();
        assert!(wire.len() >= 25, "the enum parser read {} variants", wire.len());

        let offered: Vec<String> = catalogue().into_iter().map(|(kind, _, _)| kind).collect();
        let missing: Vec<&String> = wire.iter().filter(|name| !offered.contains(name)).collect();
        assert!(missing.is_empty(), "conditions the rule builder cannot offer: {missing:?}");
        assert_eq!(
            offered.len(),
            wire.len(),
            "the catalogue holds an entry naming no condition, or two naming one"
        );
    }

    /// The wire name of every `Condition` variant, from its serde rename.
    fn wire_names() -> Vec<String> {
        let source = include_str!("../models/rule.rs");
        let enumeration = source
            .split("pub enum Condition {")
            .nth(1)
            .expect("the Condition enum")
            .split("\n}")
            .next()
            .expect("its body");
        enumeration
            .split("#[serde(rename = \"")
            .skip(1)
            .filter_map(|tail| tail.split('"').next().map(str::to_string))
            .collect()
    }

    /// Every axis a `Spec` names is an axis `/media/facets` answers with.
    ///
    /// `suggestions` is what selects a picker over the library's own values
    /// instead of a free-text field, and the interface reads it by indexing the
    /// facets payload — through a cast, because the key arrives at run time.
    /// The cast erases the check and the `?? []` behind it turns a miss into an
    /// empty list, so an axis renamed on one side leaves the rule builder
    /// silently offering nothing, and the operator types values by hand. A rule
    /// written against a value the library does not hold matches nothing and
    /// reads on screen exactly like a rule that correctly matches nothing —
    /// which is the thing facets exist to prevent.
    ///
    /// `check-api-types.py` cannot see this: it compares field *names*, and
    /// `suggestions` carries a *value*.
    #[test]
    fn every_axis_a_spec_names_is_one_the_facets_payload_answers() {
        let types = include_str!("../../../frontend/src/api/types.ts");
        let payload = types
            .split_once("export interface LibraryFacets {")
            .expect("the LibraryFacets interface")
            .1
            .split_once("\n}")
            .expect("its closing brace")
            .0;
        let axes: Vec<&str> =
            payload.lines().filter_map(|line| line.trim().strip_suffix(": Facet[];")).collect();
        assert!(axes.len() >= 5, "only {} axes parsed out of types.ts: {axes:?}", axes.len());

        let source = include_str!("conditions.rs");
        let table = source.split("Spec {").skip(1);
        let mut missing = Vec::new();
        let mut checked = 0;
        for spec in table {
            let Some(body) = spec.split("\n        }").next() else { continue };
            let Some(kind) = between(body, "kind: \"", "\"") else { continue };
            let Some(axis) = between(body, "suggestions: \"", "\"") else { continue };
            if axis.is_empty() {
                continue;
            }
            checked += 1;
            if !axes.contains(&axis.as_str()) {
                missing.push(format!(
                    "{kind} draws from '{axis}', which /media/facets does not answer"
                ));
            }
        }
        assert!(checked >= 5, "only {checked} spec(s) name an axis at all");
        assert!(missing.is_empty(), "{missing:?}");
    }

    /// Every entry names a condition the engine can actually be given.
    ///
    /// The builder emits `{"type": kind, "value": …}` straight from this table,
    /// so a misspelt `kind` or a `value_type` that does not match the variant's
    /// shape produces a rule the backend refuses — with the interface offering
    /// it as if it worked.
    #[test]
    fn every_catalogue_entry_deserialises_into_a_condition() {
        let entries = catalogue();
        assert!(entries.len() >= 25, "the catalogue parser read {} entries", entries.len());

        for (kind, value_type, _) in &entries {
            let body = serde_json::json!({ "type": kind, "value": sample(value_type) });
            serde_json::from_value::<Condition>(body.clone()).unwrap_or_else(|e| {
                panic!("catalogue entry {kind} is not a condition: {e} ({body})")
            });
        }
    }

    /// The catalogue and the engine agree on which metadata field a condition
    /// reads.
    ///
    /// It is stated twice — `Spec.field` here, `Condition::metadata_field`
    /// there — and the two answer different readers: the builder warns that no
    /// enabled source can supply the field, the engine decides whether a
    /// condition could ever match. Disagreeing, the interface offers a
    /// condition it says is fine and the evaluation reports it unanswerable.
    #[test]
    fn the_catalogue_names_the_field_the_engine_reads() {
        for (kind, value_type, declared) in catalogue() {
            let body = serde_json::json!({ "type": kind, "value": sample(&value_type) });
            let condition: Condition = serde_json::from_value(body).expect("a condition");
            let engine = condition.metadata_field().map(|f| format!("{f:?}"));
            assert_eq!(
                engine, declared,
                "{kind}: the engine reads {engine:?}, the catalogue declares {declared:?}"
            );
        }
    }
}
