//! Servarr language names to ISO 639-1 codes.
//!
//! TMDb reports `original_language` as a code (`ja`), the Arrs as a word
//! (`Japanese`). One field, two vocabularies: untranslated, a rule matching
//! through TMDb stops matching the day the Arr answers first — silently, which
//! is the worst failure a routing rule has.
//!
//! The list is the `Language` enum Radarr and Sonarr share. A name outside it
//! keeps its own spelling, lowercased, so a rule still has something to compare
//! against instead of the field going empty.

/// ISO 639-1 code for a language *name*, or the name itself, lowercased, when it
/// is not one we know.
///
/// `None` for "Unknown", the value the Arr uses when it has no idea — an
/// absent field is the truth there, and it lets the source below answer.
/// Every language this build knows, as (ISO 639-1 code, the spellings a source may use).
/// The first spelling is the one shown; the rest are what `normalise` accepts.
///
/// One table read both ways. A rule is written against the code, which no user
/// would guess from a library that happens to hold five languages, so the rule
/// builder offers this rather than what has been synced.
pub const LANGUAGES: &[(&str, &[&str])] = &[
    ("en", &["english"]),
    ("fr", &["french"]),
    ("es", &["spanish"]),
    ("de", &["german"]),
    ("it", &["italian"]),
    ("da", &["danish"]),
    ("nl", &["dutch", "flemish"]),
    ("ja", &["japanese"]),
    ("is", &["icelandic"]),
    ("zh", &["chinese"]),
    ("ru", &["russian"]),
    ("pl", &["polish"]),
    ("vi", &["vietnamese"]),
    ("sv", &["swedish"]),
    ("no", &["norwegian"]),
    ("fi", &["finnish"]),
    ("tr", &["turkish"]),
    ("pt", &["portuguese"]),
    ("el", &["greek"]),
    ("ko", &["korean"]),
    ("hu", &["hungarian"]),
    ("he", &["hebrew"]),
    ("lt", &["lithuanian"]),
    ("cs", &["czech"]),
    ("hi", &["hindi"]),
    ("ro", &["romanian"]),
    ("th", &["thai"]),
    ("bg", &["bulgarian"]),
    ("ar", &["arabic"]),
    ("uk", &["ukrainian"]),
    ("fa", &["persian"]),
    ("bn", &["bengali"]),
    ("sk", &["slovak"]),
    ("lv", &["latvian"]),
    ("ca", &["catalan"]),
    ("hr", &["croatian"]),
    ("sr", &["serbian"]),
    ("bs", &["bosnian"]),
    ("et", &["estonian"]),
    ("ta", &["tamil"]),
    ("id", &["indonesian"]),
    ("te", &["telugu"]),
    ("mk", &["macedonian"]),
    ("sl", &["slovenian"]),
    ("ml", &["malayalam"]),
    ("kn", &["kannada"]),
    ("sq", &["albanian"]),
    ("af", &["afrikaans"]),
    ("mr", &["marathi"]),
    ("tl", &["tagalog"]),
    ("ur", &["urdu"]),
    ("rm", &["romansh"]),
    ("mn", &["mongolian"]),
];

/// Every country this build knows, as (ISO 3166-1 alpha-2 code, its spellings).
/// Read both ways for the same reason as [`LANGUAGES`].
pub const COUNTRIES: &[(&str, &[&str])] = &[
    ("US", &["united states", "usa", "united states of america"]),
    ("GB", &["united kingdom", "uk", "great britain"]),
    ("JP", &["japan"]),
    ("FR", &["france"]),
    ("DE", &["germany", "west germany", "east germany"]),
    ("IT", &["italy"]),
    ("ES", &["spain"]),
    ("CA", &["canada"]),
    ("AU", &["australia"]),
    ("NZ", &["new zealand"]),
    ("CN", &["china"]),
    ("HK", &["hong kong"]),
    ("TW", &["taiwan"]),
    ("KR", &["south korea", "korea, south", "korea"]),
    ("IN", &["india"]),
    ("RU", &["russia", "soviet union"]),
    ("BR", &["brazil"]),
    ("MX", &["mexico"]),
    ("AR", &["argentina"]),
    ("SE", &["sweden"]),
    ("NO", &["norway"]),
    ("DK", &["denmark"]),
    ("FI", &["finland"]),
    ("IS", &["iceland"]),
    ("NL", &["netherlands"]),
    ("BE", &["belgium"]),
    ("IE", &["ireland"]),
    ("PL", &["poland"]),
    ("CZ", &["czech republic", "czechia"]),
    ("AT", &["austria"]),
    ("CH", &["switzerland"]),
    ("PT", &["portugal"]),
    ("GR", &["greece"]),
    ("TR", &["turkey"]),
    ("IL", &["israel"]),
    ("ZA", &["south africa"]),
    ("TH", &["thailand"]),
    ("ID", &["indonesia"]),
    ("PH", &["philippines"]),
    ("VN", &["vietnam"]),
    ("UA", &["ukraine"]),
    ("HU", &["hungary"]),
    ("RO", &["romania"]),
];

pub fn normalise(name: &str) -> Option<String> {
    // "Portuguese (Brazil)" and "Spanish (Latino)" are regional spellings of a
    // language TMDb reports without the region.
    let base = name.split('(').next().unwrap_or(name).trim().to_lowercase();
    if base.is_empty() || base == "unknown" {
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

/// ISO 639-1 from a three-letter code.
///
/// TheTVDB answers `jpn`, Routarr's rules are written against `ja`. Only the
/// bibliographic/terminological pairs that differ are listed explicitly; the
/// rest map by their own table above once the name is unavailable.
pub fn from_iso_639_3(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase();
    if code.len() == 2 {
        return Some(code);
    }

    let two = match code.as_str() {
        "eng" => "en",
        "fra" | "fre" => "fr",
        "spa" => "es",
        "deu" | "ger" => "de",
        "ita" => "it",
        "dan" => "da",
        "nld" | "dut" => "nl",
        "jpn" => "ja",
        "isl" | "ice" => "is",
        "zho" | "chi" => "zh",
        "rus" => "ru",
        "pol" => "pl",
        "vie" => "vi",
        "swe" => "sv",
        "nor" | "nob" => "no",
        "fin" => "fi",
        "tur" => "tr",
        "por" => "pt",
        "ell" | "gre" => "el",
        "kor" => "ko",
        "hun" => "hu",
        "heb" => "he",
        "lit" => "lt",
        "ces" | "cze" => "cs",
        "hin" => "hi",
        "ron" | "rum" => "ro",
        "tha" => "th",
        "bul" => "bg",
        "ara" => "ar",
        "ukr" => "uk",
        "fas" | "per" => "fa",
        "ben" => "bn",
        "slk" | "slo" => "sk",
        "lav" => "lv",
        "cat" => "ca",
        "hrv" => "hr",
        "srp" => "sr",
        "bos" => "bs",
        "est" => "et",
        "tam" => "ta",
        "ind" => "id",
        "tel" => "te",
        "mkd" | "mac" => "mk",
        "slv" => "sl",
        "mal" => "ml",
        "kan" => "kn",
        "sqi" | "alb" => "sq",
        "afr" => "af",
        "mar" => "mr",
        "tgl" => "tl",
        "urd" => "ur",
        "mon" => "mn",
        _ => return None,
    };

    Some(two.to_string())
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
/// TheTVDB answers `usa` and `jpn`; a rule reads `US` and `JP`.
pub fn country_from_alpha3(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase();
    if code.len() == 2 {
        return Some(code.to_uppercase());
    }

    let two = match code.as_str() {
        "usa" => "US",
        "gbr" => "GB",
        "jpn" => "JP",
        "fra" => "FR",
        "deu" => "DE",
        "ita" => "IT",
        "esp" => "ES",
        "can" => "CA",
        "aus" => "AU",
        "nzl" => "NZ",
        "chn" => "CN",
        "hkg" => "HK",
        "twn" => "TW",
        "kor" => "KR",
        "ind" => "IN",
        "rus" => "RU",
        "bra" => "BR",
        "mex" => "MX",
        "arg" => "AR",
        "swe" => "SE",
        "nor" => "NO",
        "dnk" => "DK",
        "fin" => "FI",
        "isl" => "IS",
        "nld" => "NL",
        "bel" => "BE",
        "irl" => "IE",
        "pol" => "PL",
        "cze" => "CZ",
        "aut" => "AT",
        "che" => "CH",
        "prt" => "PT",
        "grc" => "GR",
        "tur" => "TR",
        "isr" => "IL",
        "zaf" => "ZA",
        "tha" => "TH",
        "idn" => "ID",
        "phl" => "PH",
        "vnm" => "VN",
        "ukr" => "UA",
        "hun" => "HU",
        "rou" => "RO",
        _ => return None,
    };

    Some(two.to_string())
}

/// Split a comma-separated list of country names into ISO codes.
pub fn country_codes(list: &str) -> Vec<String> {
    list.split(',').filter_map(country_code).collect()
}

/// The first language of a comma-separated list of names, as a code.
///
/// OMDb's `Language` is "Japanese, English" — the first is the original one.
pub fn first_language(list: &str) -> Option<String> {
    list.split(',').find_map(normalise)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_language_we_do_not_know_keeps_its_own_name() {
        assert_eq!(normalise("Klingon").as_deref(), Some("klingon"));
    }

    #[test]
    fn a_three_letter_code_becomes_the_two_letter_one() {
        assert_eq!(from_iso_639_3("jpn").as_deref(), Some("ja"));
        // Both the bibliographic and the terminological spelling exist in the
        // wild, and TheTVDB is not consistent about which it returns.
        assert_eq!(from_iso_639_3("fre").as_deref(), Some("fr"));
        assert_eq!(from_iso_639_3("fra").as_deref(), Some("fr"));
    }

    #[test]
    fn a_code_already_two_letters_is_left_alone() {
        assert_eq!(from_iso_639_3("ja").as_deref(), Some("ja"));
    }

    #[test]
    fn an_unknown_three_letter_code_is_no_answer() {
        // Unlike a name, an unrecognised code is not usable as-is: a rule reads
        // `ja`, never `tlh`. Better to let the source below answer.
        assert_eq!(from_iso_639_3("tlh"), None);
    }

    #[test]
    fn a_country_name_becomes_the_code_a_rule_is_written_against() {
        assert_eq!(country_codes("Japan, United States"), vec!["JP", "US"]);
    }

    #[test]
    fn an_unknown_country_is_dropped_rather_than_guessed() {
        assert_eq!(country_codes("Japan, Atlantis"), vec!["JP"]);
    }

    #[test]
    fn an_alpha_3_country_becomes_alpha_2() {
        assert_eq!(country_from_alpha3("usa").as_deref(), Some("US"));
        assert_eq!(country_from_alpha3("jpn").as_deref(), Some("JP"));
        assert_eq!(country_from_alpha3("JP").as_deref(), Some("JP"));
        assert_eq!(country_from_alpha3("xyz"), None);
    }

    #[test]
    fn the_first_language_of_a_list_is_the_original_one() {
        assert_eq!(first_language("Japanese, English").as_deref(), Some("ja"));
    }

    // ------------------------------------------------- dropping, not guessing
    //
    // This module's contract is that an unmappable value produces *no answer*
    // rather than a plausible one. That matters because of what happens next: a
    // dropped field lets the source below the current one answer, while a wrong
    // one is accepted and silently routes a title into the wrong folder. The
    // failure is invisible either way — nothing logs "this rule stopped
    // matching" — so these are the tests standing in for a symptom.

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
    /// not cost the others — a partial answer is still an answer, an empty one
    /// makes every country condition stop matching.
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
    /// pinning: a *name* we do not know is kept as itself, because it is still
    /// a value a user can write a rule against; a *code* we do not know is
    /// dropped, because "tlh" means nothing to anyone reading the interface.
    #[test]
    fn an_unknown_name_is_kept_while_an_unknown_code_is_dropped() {
        assert_eq!(normalise("Klingon").as_deref(), Some("klingon"));
        assert_eq!(from_iso_639_3("tlh"), None);
    }
}
