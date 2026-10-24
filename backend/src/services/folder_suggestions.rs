//! Categories suggested from the names of the root folders, read as whole words and never
//! guessed: `/movies/anime` says anime, `/movies/animals` says nothing. Nothing here writes, a
//! person maps what they accept through `PUT /root-folders/{id}/category`.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use crate::localization::{Localizer, languages};
use crate::services::rule_engine::normalise_value;
use crate::services::templates::{CATALOGUE, Template};

/// Words a folder of a template's family goes by beyond its category's name in each dictionary,
/// folded. A word that also names something else (`animation`, `live`) stays out: no suggestion
/// is better than a wrong one.
const ALIASES: &[(&str, &[&str])] = &[
    ("4k", &["4k", "uhd", "2160p", "ultra hd", "ultrahd"]),
    ("concerts", &["concert", "concerts", "konzert", "konzerte", "conciertos", "concerten"]),
    (
        "documentaries",
        &[
            "documentary",
            "documentaries",
            "docs",
            "docu",
            "docus",
            "documentaire",
            "documentaires",
            "doku",
            "dokus",
            "documental",
            "documentales",
        ],
    ),
    (
        "children",
        &[
            "kids", "kid", "children", "family", "jeunesse", "enfants", "famille", "kinder",
            "familie", "ninos", "infantil", "kinderen", "jeugd",
        ],
    ),
    ("anime", &["anime", "animes"]),
    ("foreign-films", &["foreign", "international", "world cinema", "vo", "vost", "vostfr"]),
];

/// Words a main library's folder goes by, beside the dictionaries' `Movies` and `Series`.
const GENERIC: &[&str] = &[
    "movies",
    "movie",
    "films",
    "film",
    "tv",
    "shows",
    "show",
    "series",
    "serie",
    "tvshows",
    "media",
    "library",
    "video",
    "videos",
    "filme",
    "serien",
    "peliculas",
    "pelicula",
    "emissions",
];

/// Why a folder was given its suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// Its name is a category the library holds.
    Existing,
    /// Its name holds this word of a template family.
    Named(String),
    /// Its name says only "movies" or "series": the main library, the default category's.
    Main,
    /// Its name says several categories, `choices` lists them.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The category to map, `None` when the name says several.
    pub category: Option<String>,
    /// Every category the name says, the suggested one alone when it says one.
    pub choices: Vec<String>,
    pub reason: Reason,
    /// The template whose family the name says, when it says one.
    pub template: Option<&'static str>,
}

/// One family's words, folded, and the names its category takes in every dictionary.
struct Family {
    template: &'static Template,
    words: Vec<Vec<String>>,
    names: BTreeSet<String>,
}

fn tokens(text: &str) -> Vec<String> {
    normalise_value(text).split(' ').filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

static FAMILIES: LazyLock<Vec<Family>> = LazyLock::new(|| {
    let dictionaries: Vec<Localizer> =
        languages().iter().map(|language| Localizer::new(language.code)).collect();
    CATALOGUE
        .iter()
        .filter(|template| !template.category.is_empty())
        .map(|template| {
            let names: BTreeSet<String> = dictionaries
                .iter()
                .map(|dictionary| dictionary.translate(template.category, &[]))
                .collect();
            let aliases =
                ALIASES.iter().find(|(id, _)| *id == template.id).map_or(&[][..], |(_, a)| a);
            let mut words: Vec<Vec<String>> = names
                .iter()
                .map(|name| tokens(name))
                .chain(aliases.iter().map(|a| tokens(a)))
                .collect();
            words.sort();
            words.dedup();
            Family { template, words, names }
        })
        .collect()
});

static GENERIC_WORDS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
    let dictionaries = languages().into_iter().map(|language| Localizer::new(language.code));
    let named = dictionaries.flat_map(|dictionary| {
        ["Movies", "Series"].map(|key| dictionary.translate(key, &[])).into_iter()
    });
    GENERIC
        .iter()
        .map(|word| word.to_string())
        .chain(named)
        .flat_map(|word| tokens(&word))
        .collect()
});

/// The last segment of a path, whichever separator the Arr writes.
fn leaf(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

/// Whether `words` appear in `said` side by side, in order.
fn says(said: &[String], words: &[String]) -> bool {
    !words.is_empty() && said.windows(words.len()).any(|window| window == words)
}

/// What the name of the folder at `path` suggests, given the categories the library holds and
/// the reader's language, or `None` when it says nothing a person could accept unread. A family
/// whose template routes no title of `media_type`, the folder's instance's, says nothing.
pub fn suggest(
    path: &str,
    known: &[String],
    localizer: &Localizer,
    default: &str,
    media_type: &str,
) -> Option<Suggestion> {
    let said = tokens(leaf(path));
    if said.is_empty() {
        return None;
    }
    // Named exactly as a category the person made: that one, whatever a family would say.
    if let Some(category) = known.iter().find(|category| tokens(category) == said) {
        let template = FAMILIES.iter().find(|f| f.names.contains(category)).map(|f| f.template.id);
        return Some(Suggestion {
            category: Some(category.clone()),
            choices: vec![category.clone()],
            reason: Reason::Existing,
            template,
        });
    }

    let mut found: Vec<(String, Reason, Option<&'static str>)> = Vec::new();
    for family in FAMILIES.iter().filter(|family| family.template.routes(media_type)) {
        let Some(word) = family.words.iter().find(|words| says(&said, words)) else { continue };
        // A category of this family the library already holds, in any language, before a new one.
        let category = known
            .iter()
            .find(|category| family.names.contains(*category))
            .cloned()
            .unwrap_or_else(|| localizer.translate(family.template.category, &[]));
        found.push((category, Reason::Named(word.join(" ")), Some(family.template.id)));
    }
    for category in known {
        let words = tokens(category);
        if says(&said, &words) && !found.iter().any(|(name, ..)| name == category) {
            found.push((category.clone(), Reason::Existing, None));
        }
    }

    match found.len() {
        0 if said.iter().all(|word| GENERIC_WORDS.contains(word)) => Some(Suggestion {
            category: Some(default.to_string()),
            choices: vec![default.to_string()],
            reason: Reason::Main,
            template: None,
        }),
        0 => None,
        1 => {
            let (category, reason, template) = found.remove(0);
            Some(Suggestion {
                category: Some(category.clone()),
                choices: vec![category],
                reason,
                template,
            })
        }
        _ => Some(Suggestion {
            category: None,
            choices: found.into_iter().map(|(category, ..)| category).collect(),
            reason: Reason::Ambiguous,
            template: None,
        }),
    }
}

/// Drops the dismissals of `instance_id` whose folder it no longer holds, so a path reported
/// again later is suggested afresh.
pub async fn forget_dismissals_without_folder<'e, E>(
    executor: E,
    instance_id: &str,
) -> sqlx::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "DELETE FROM folder_suggestion_dismissals
          WHERE instance_id = ?
            AND NOT EXISTS (SELECT 1 FROM root_folders rf
                             WHERE rf.instance_id = folder_suggestion_dismissals.instance_id
                               AND rf.path = folder_suggestion_dismissals.path COLLATE path)",
    )
    .bind(instance_id)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english(path: &str, known: &[&str]) -> Option<Suggestion> {
        let known: Vec<String> = known.iter().map(|k| k.to_string()).collect();
        suggest(path, &known, &Localizer::new("en"), "standard", "movie")
    }

    fn category(path: &str) -> Option<String> {
        english(path, &[]).and_then(|suggestion| suggestion.category)
    }

    #[test]
    fn the_last_segment_says_the_family_whatever_its_parents() {
        assert_eq!(category("/data/media/movies/anime"), Some("anime".into()));
        assert_eq!(category("/mnt/films/4K-UHD/"), Some("4k".into()));
        assert_eq!(category("D:\\Media\\Kids\\"), Some("kids".into()));
        assert_eq!(category("/media/Documentaires"), Some("documentaries".into()));
        assert_eq!(category("/tv/Animés"), Some("anime".into()));
    }

    #[test]
    fn a_word_inside_another_says_nothing() {
        for path in ["/movies/animals", "/srv/docker", "/movies/kidnapping", "/movies/vod"] {
            assert_eq!(english(path, &[]), None, "{path}");
        }
    }

    #[test]
    fn a_folder_named_only_for_its_media_is_the_main_library() {
        for path in ["/movies", "/data/media/tv", "/mnt/Films", "/media/TV Shows", "/Serien"] {
            let suggestion = english(path, &[]).expect(path);
            assert_eq!(suggestion.category.as_deref(), Some("standard"), "{path}");
            assert_eq!(suggestion.reason, Reason::Main, "{path}");
        }
    }

    #[test]
    fn a_name_saying_two_families_is_left_to_the_person() {
        let suggestion = english("/movies/kids-anime", &[]).unwrap();
        assert_eq!(suggestion.category, None);
        assert_eq!(suggestion.reason, Reason::Ambiguous);
        assert_eq!(suggestion.choices, ["kids", "anime"]);
    }

    #[test]
    fn a_category_the_library_holds_is_suggested_before_a_new_one() {
        let horror = english("/movies/horror", &["horror"]).unwrap();
        assert_eq!((horror.category.as_deref(), horror.reason), (Some("horror"), Reason::Existing));
        let kids = english("/movies/kids", &["enfants"]).unwrap();
        assert_eq!(kids.category.as_deref(), Some("enfants"));
        assert_eq!(kids.template, Some("children"));
        let within = english("/movies/horror-films", &["horror"]).unwrap();
        assert_eq!(within.category.as_deref(), Some("horror"));
    }

    #[test]
    fn a_new_category_takes_the_readers_language() {
        let french =
            suggest("/films/kids", &[], &Localizer::new("fr"), "standard", "movie").unwrap();
        assert_eq!(french.category.as_deref(), Some("enfants"));
        assert_eq!(french.reason, Reason::Named("kids".into()));
        assert_eq!(french.template, Some("children"));
    }

    #[test]
    fn a_name_saying_nothing_known_suggests_nothing() {
        assert_eq!(english("/movies/horror", &[]), None);
        assert_eq!(english("/", &[]), None);
    }
}
