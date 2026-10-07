//! Servarr language names to ISO 639-1 codes.
//!
//! TMDb reports `original_language` as a code (`ja`), the Arrs as a word
//! (`Japanese`). One field, two vocabularies: untranslated, a rule matching
//! through TMDb silently stops matching the day the Arr answers first, the
//! worst failure a routing rule has.
//!
//! The tables (`tables.rs`) hold every ISO 639-1 language and every ISO 3166-1
//! country. Radarr names 57 languages and Sonarr 46 of them, OMDb writes
//! English names of its own, and TheTVDB three-letter codes. A name no table
//! knows keeps its own spelling, lowercased, so a rule still has something to
//! compare against instead of the field going empty.

mod tables;

use tables::{ALPHA3, ISO_639_3};
pub use tables::{COUNTRIES, LANGUAGES};

/// ISO 639-1 code for a language *name*, or the name itself, lowercased, when it
/// is not one we know.
///
/// `None` for "Unknown", the value the Arr uses when it has no idea, and for
/// OMDb's "N/A" and "None": an absent field is the truth there, and it lets
/// the source below answer.
pub fn normalise(name: &str) -> Option<String> {
    // "Portuguese (Brazil)" and "Spanish (Latino)" are regional spellings of a
    // language TMDb reports without the region.
    let base = name.split('(').next().unwrap_or(name).trim().to_lowercase();
    if matches!(base.as_str(), "" | "unknown" | "n/a" | "none") {
        return None;
    }

    LANGUAGES
        .iter()
        .find(|(_, spellings)| spellings.contains(&base.as_str()))
        .map(|(code, _)| (*code).to_string())
        // A language the table does not know keeps its own name: dropping it
        // would lose a value a rule could still be written against.
        .or(Some(base))
}

/// ISO 639-1 from TMDb's `original_language`.
///
/// TMDb writes ISO 639-1 but for two codes: `cn`, its Cantonese, which ISO
/// files under `zh`, and `xx`, no language, which would claim the field and
/// keep a lower source from filling it.
pub fn from_tmdb(code: &str) -> Option<String> {
    match code.trim().to_lowercase().as_str() {
        "" | "xx" => None,
        "cn" => Some("zh".to_string()),
        other => Some(other.to_string()),
    }
}

/// ISO 639-1 from a three-letter code.
///
/// TheTVDB answers `jpn`, Routarr's rules are written against `ja`. A code
/// that names no ISO 639-1 language answers `None`.
pub fn from_iso_639_3(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase();
    if code.len() == 2 {
        return Some(code);
    }
    ISO_639_3
        .binary_search_by(|(three, _)| (*three).cmp(code.as_str()))
        .ok()
        .map(|index| ISO_639_3[index].1.to_string())
}

/// ISO 3166-1 alpha-2 from a country *name*.
///
/// OMDb answers "United States, Japan" where TMDb answers `["US", "JP"]`, and
/// `origin_country` conditions are written against the code. A country we do not
/// know yields nothing rather than a wrong code: a source below can still
/// answer, and a wrong country silently misroutes.
fn country_code(name: &str) -> Option<String> {
    let name = name.trim().to_lowercase();
    COUNTRIES
        .iter()
        .find(|(_, spellings)| spellings.contains(&name.as_str()))
        .map(|(code, _)| (*code).to_string())
}

/// ISO 3166-1 alpha-2 from an alpha-3 code.
///
/// TheTVDB answers `usa` and `jpn`, where a rule reads `US` and `JP`. Every
/// country ISO 3166-1 lists, from the Debian iso-codes data: a country left
/// out answers nothing, and a rule on it never matches.
pub fn country_from_alpha3(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase();
    if let Some((_, tmdb)) = RETIRED_ALIASES.iter().find(|(alias, _)| *alias == code) {
        return Some((*tmdb).to_string());
    }
    if code.len() == 2 {
        return Some(code.to_uppercase());
    }
    ALPHA3
        .binary_search_by(|(alpha3, _)| (*alpha3).cmp(code.as_str()))
        .ok()
        .map(|index| ALPHA3[index].1.to_string())
}

/// The countries that no longer exist, by the code TMDb gives them, each named
/// by the dictionary key `CountryRetired<code>`: a browser names `SU` after
/// Russia and `AN` after Curaçao, the countries that took their codes.
pub const RETIRED: &[&str] = &["SU", "XG", "XC", "YU", "CS", "AN"];

/// The other codes a source writes for those countries, ISO 3166-3's
/// alpha-3 and East Germany's alpha-2 among them, and the code TMDb gives.
const RETIRED_ALIASES: &[(&str, &str)] = &[
    ("sun", "SU"),
    ("ddr", "XG"),
    ("dd", "XG"),
    ("csk", "XC"),
    ("yug", "YU"),
    ("scg", "CS"),
    ("ant", "AN"),
];

/// Split a comma-separated list of country names into ISO codes.
pub fn country_codes(list: &str) -> Vec<String> {
    list.split(',').filter_map(country_code).collect()
}

/// The first language of a comma-separated list of names, as a code.
///
/// OMDb's `Language` is "Japanese, English", the original one first.
pub fn first_language(list: &str) -> Option<String> {
    list.split(',').find_map(normalise)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A country that no longer exists keeps the code TMDb files its films
    /// under, whichever source names it and however: East Germany is not
    /// today's Germany, and a rule on `DE` does not take its films.
    #[test]
    fn a_retired_country_reads_as_the_code_tmdb_gives_it() {
        assert_eq!(country_codes("East Germany"), ["XG"]);
        assert_eq!(country_codes("West Germany"), ["DE"]);
        assert_eq!(country_codes("Czechoslovakia, Yugoslavia"), ["XC", "YU"]);
        assert_eq!(country_codes("Serbia and Montenegro"), ["CS"]);
        assert_eq!(country_codes("Netherlands Antilles"), ["AN"]);
        for (alpha3, code) in [
            ("ddr", "XG"),
            ("csk", "XC"),
            ("yug", "YU"),
            ("scg", "CS"),
            ("ant", "AN"),
            ("sun", "SU"),
        ] {
            assert_eq!(country_from_alpha3(alpha3).as_deref(), Some(code), "{alpha3}");
        }
        assert_eq!(country_from_alpha3("DD").as_deref(), Some("XG"), "the ISO code of the GDR");
    }

    /// OMDb writes "N/A" where it has no language, and "None" for a film with
    /// no dialogue: neither is a language a rule could name.
    #[test]
    fn omdb_saying_it_has_no_language_gives_none() {
        for absent in ["N/A", "None", "n/a"] {
            assert_eq!(first_language(absent), None, "{absent}");
        }
        assert_eq!(first_language("None, English").as_deref(), Some("en"));
    }

    /// OMDb names the spoken Chinese, and Filipino, where TMDb gives the
    /// language's code: `zh`, as TMDb's own Cantonese reads, and `tl`.
    #[test]
    fn omdb_spoken_chinese_and_filipino_read_as_their_codes() {
        for (spoken, code) in [("Mandarin", "zh"), ("Cantonese", "zh"), ("Filipino", "tl")] {
            assert_eq!(normalise(spoken).as_deref(), Some(code), "{spoken}");
        }
        // TheTVDB's codes for both.
        assert_eq!(from_iso_639_3("yue").as_deref(), Some("zh"));
        assert_eq!(from_iso_639_3("zhtw").as_deref(), Some("zh"));
    }

    /// Every ISO country reads from its English name, as OMDb writes it, so a
    /// country of a list is never dropped and left to claim the field with the
    /// others alone.
    #[test]
    fn every_iso_country_reads_from_its_english_name() {
        for (_, alpha2) in ALPHA3 {
            let (_, spellings) = COUNTRIES.iter().find(|(code, _)| code == alpha2).expect(alpha2);
            assert_eq!(country_codes(spellings[0]), [*alpha2], "{}", spellings[0]);
        }
        assert_eq!(country_codes("Egypt, France"), ["EG", "FR"]);
        assert_eq!(country_codes("UK, USA, North Korea"), ["GB", "US", "KP"]);
        assert_eq!(country_codes("Republic of North Macedonia"), ["MK"]);
        assert_eq!(country_codes("Federal Republic of Yugoslavia"), ["YU"]);
        assert_eq!(country_codes("Côte d'Ivoire, Ivory Coast, Iran"), ["CI", "CI", "IR"]);
    }

    /// Every ISO 639-1 language reads from its English name and from each of
    /// its three-letter codes, the bibliographic one and the terminological.
    /// Bokmål's `nob` reads as the `no` TMDb and the Arrs give.
    #[test]
    fn every_iso_639_1_language_reads_from_its_name_and_its_three_letters() {
        for (code, spellings) in LANGUAGES {
            assert_eq!(normalise(spellings[0]).as_deref(), Some(*code), "{}", spellings[0]);
            let read = ISO_639_3.iter().any(|(_, two)| two == code);
            assert!(read || *code == "nb", "{code} has no three letters");
        }
        assert_eq!(from_iso_639_3("nob").as_deref(), Some("no"));
        assert_eq!(normalise("Malay").as_deref(), Some("ms"));
        assert_eq!(normalise("Swahili").as_deref(), Some("sw"));
        assert_eq!(normalise("Punjabi").as_deref(), Some("pa"));
        for three in ["msa", "may"] {
            assert_eq!(from_iso_639_3(three).as_deref(), Some("ms"), "{three}");
        }
        assert!(ISO_639_3.windows(2).all(|pair| pair[0].0 < pair[1].0), "the table is unsorted");
    }

    /// Radarr names 57 languages, Sonarr 46 of them: each reads as the code a
    /// rule is written against, Georgian as `ka`.
    #[test]
    fn every_language_radarr_names_reads_as_its_code() {
        let radarr = [
            ("English", "en"),
            ("French", "fr"),
            ("Spanish", "es"),
            ("German", "de"),
            ("Italian", "it"),
            ("Danish", "da"),
            ("Dutch", "nl"),
            ("Japanese", "ja"),
            ("Icelandic", "is"),
            ("Chinese", "zh"),
            ("Russian", "ru"),
            ("Polish", "pl"),
            ("Vietnamese", "vi"),
            ("Swedish", "sv"),
            ("Norwegian", "no"),
            ("Finnish", "fi"),
            ("Turkish", "tr"),
            ("Portuguese", "pt"),
            ("Flemish", "nl"),
            ("Greek", "el"),
            ("Korean", "ko"),
            ("Hungarian", "hu"),
            ("Hebrew", "he"),
            ("Lithuanian", "lt"),
            ("Czech", "cs"),
            ("Hindi", "hi"),
            ("Romanian", "ro"),
            ("Thai", "th"),
            ("Bulgarian", "bg"),
            ("Portuguese (Brazil)", "pt"),
            ("Arabic", "ar"),
            ("Ukrainian", "uk"),
            ("Persian", "fa"),
            ("Bengali", "bn"),
            ("Slovak", "sk"),
            ("Latvian", "lv"),
            ("Spanish (Latino)", "es"),
            ("Catalan", "ca"),
            ("Croatian", "hr"),
            ("Serbian", "sr"),
            ("Bosnian", "bs"),
            ("Estonian", "et"),
            ("Tamil", "ta"),
            ("Indonesian", "id"),
            ("Telugu", "te"),
            ("Macedonian", "mk"),
            ("Slovenian", "sl"),
            ("Malayalam", "ml"),
            ("Kannada", "kn"),
            ("Albanian", "sq"),
            ("Afrikaans", "af"),
            ("Marathi", "mr"),
            ("Tagalog", "tl"),
            ("Urdu", "ur"),
            ("Romansh", "rm"),
            ("Mongolian", "mn"),
            ("Georgian", "ka"),
        ];
        assert_eq!(radarr.len(), 57);
        for (name, code) in radarr {
            assert_eq!(normalise(name).as_deref(), Some(code), "{name}");
        }
        assert_eq!(from_iso_639_3("kat").as_deref(), Some("ka"));
        assert_eq!(from_iso_639_3("geo").as_deref(), Some("ka"));
    }

    /// One spelling names one language and one country, or which code it
    /// reads as would depend on the order of a table.
    #[test]
    fn no_spelling_names_two_codes() {
        for table in [LANGUAGES, COUNTRIES] {
            let mut seen = std::collections::HashMap::new();
            for (code, spellings) in table {
                for spelling in *spellings {
                    let first = seen.insert(*spelling, *code);
                    assert!(
                        first.is_none_or(|other| other == *code),
                        "{spelling}: {first:?} {code}"
                    );
                }
            }
        }
    }

    /// Every ISO country reads from TheTVDB's three letters, the first and the
    /// last of the table among them, and a code that is none answers nothing.
    #[test]
    fn every_country_reads_from_its_three_letters() {
        for (alpha3, alpha2) in
            [("abw", "AW"), ("mys", "MY"), ("egy", "EG"), ("NGA", "NG"), ("zwe", "ZW")]
        {
            assert_eq!(country_from_alpha3(alpha3).as_deref(), Some(alpha2), "{alpha3}");
        }
        assert_eq!(country_from_alpha3("xyz"), None);
        assert!(ALPHA3.windows(2).all(|pair| pair[0].0 < pair[1].0), "the table is unsorted");
    }

    /// TMDb files a Soviet film under `SU`, so OMDb's "Soviet Union" does too,
    /// or one rule reads the same film two ways depending on the source.
    #[test]
    fn the_soviet_union_reads_as_tmdb_writes_it() {
        assert_eq!(country_codes("Soviet Union, Russia"), ["SU", "RU"]);
    }

    #[test]
    fn a_servarr_name_becomes_the_code_tmdb_would_have_returned() {
        assert_eq!(normalise("Japanese").as_deref(), Some("ja"));
        assert_eq!(normalise("  french  ").as_deref(), Some("fr"));
    }

    #[test]
    fn a_regional_spelling_maps_to_the_language() {
        assert_eq!(normalise("Portuguese (Brazil)").as_deref(), Some("pt"));
        assert_eq!(normalise("Spanish (Latino)").as_deref(), Some("es"));
    }

    #[test]
    fn unknown_is_no_answer_rather_than_a_wrong_one() {
        assert_eq!(normalise("Unknown"), None);
        assert_eq!(normalise("   "), None);
    }

    #[test]
    fn a_three_letter_code_becomes_the_two_letter_one() {
        assert_eq!(from_iso_639_3("jpn").as_deref(), Some("ja"));
        // Both the bibliographic and the terminological spelling exist in the
        // wild, and TheTVDB is not consistent about which it returns.
        assert_eq!(from_iso_639_3("fre").as_deref(), Some("fr"));
        assert_eq!(from_iso_639_3("fra").as_deref(), Some("fr"));
    }

    /// Romansh is in the vocabulary a rule is written against, as `rm`, so a
    /// source answering `roh` has to land there too.
    #[test]
    fn romansh_in_three_letters_matches_a_rule_written_against_rm() {
        assert_eq!(from_iso_639_3("roh").as_deref(), Some("rm"));
        assert_eq!(normalise("Romansh").as_deref(), Some("rm"));
    }

    #[test]
    fn a_code_already_two_letters_is_left_alone() {
        assert_eq!(from_iso_639_3("ja").as_deref(), Some("ja"));
    }

    #[test]
    fn a_country_name_becomes_the_code_a_rule_is_written_against() {
        assert_eq!(country_codes("Japan, United States"), vec!["JP", "US"]);
    }

    #[test]
    fn an_alpha_3_country_becomes_alpha_2() {
        assert_eq!(country_from_alpha3("usa").as_deref(), Some("US"));
        assert_eq!(country_from_alpha3("jpn").as_deref(), Some("JP"));
        assert_eq!(country_from_alpha3("JP").as_deref(), Some("JP"));
        assert_eq!(country_from_alpha3("xyz"), None);
    }

    // ------------------------------------------------- dropping, not guessing
    //
    // This module's contract is that an unmappable value produces *no answer*
    // rather than a plausible one. That matters because of what happens next: a
    // dropped field lets the source below the current one answer, while a wrong
    // one is accepted and silently routes a title into the wrong folder. The
    // failure is invisible either way, since nothing logs "this rule stopped
    // matching", so these tests stand in for a symptom.

    /// TheTVDB answers in three-letter codes and not all of them are ones we
    /// map. The unmappable one has to disappear, not become a neighbour.
    #[test]
    fn an_unmappable_country_code_is_no_answer_rather_than_a_neighbour() {
        assert_eq!(country_from_alpha3("xkx"), None);
        assert_eq!(country_from_alpha3("zzz"), None);
        assert_eq!(country_from_alpha3(""), None);
    }

    /// A code that is already ISO 3166-1 alpha-2 passes through, cased the way
    /// a rule is written.
    #[test]
    fn a_two_letter_country_code_is_normalised_not_rejected() {
        assert_eq!(country_from_alpha3("jp").as_deref(), Some("JP"));
        assert_eq!(country_from_alpha3(" Us ").as_deref(), Some("US"));
    }

    /// OMDb returns prose: "Japan, United States". One unrecognised entry must
    /// not cost the others: a partial answer is still an answer, and an empty
    /// one makes every country condition stop matching.
    #[test]
    fn an_unknown_entry_does_not_discard_the_countries_beside_it() {
        assert_eq!(country_codes("Japan, Atlantis, France"), vec!["JP", "FR"]);
        assert!(country_codes("Atlantis, Wakanda").is_empty());
        assert!(country_codes("").is_empty());
    }

    /// `first_language` takes the *first that maps*, not the first entry. OMDb
    /// puts the original language first, but it also writes "Unknown" there.
    #[test]
    fn the_first_language_is_the_first_one_that_maps() {
        assert_eq!(first_language("Japanese, English").as_deref(), Some("ja"));
        assert_eq!(first_language("Unknown, Japanese").as_deref(), Some("ja"));
        assert_eq!(first_language(""), None);
    }

    /// The asymmetry between the two entry points is deliberate and worth
    /// pinning. A *name* we do not know is kept as itself, because it is still
    /// a value a user can write a rule against. A *code* we do not know is
    /// dropped, because "tlh" means nothing to anyone reading the interface.
    #[test]
    fn an_unknown_name_is_kept_while_an_unknown_code_is_dropped() {
        assert_eq!(normalise("Klingon").as_deref(), Some("klingon"));
        assert_eq!(from_iso_639_3("tlh"), None);
    }
}
