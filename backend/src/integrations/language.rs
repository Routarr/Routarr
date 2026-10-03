//! Servarr language names to ISO 639-1 codes.
//!
//! TMDb reports `original_language` as a code (`ja`), the Arrs as a word
//! (`Japanese`). One field, two vocabularies: untranslated, a rule matching
//! through TMDb silently stops matching the day the Arr answers first, the
//! worst failure a routing rule has.
//!
//! The list is the `Language` enum Radarr and Sonarr share. A name outside it
//! keeps its own spelling, lowercased, so a rule still has something to compare
//! against instead of the field going empty.

/// Every language this build knows, as (ISO 639-1 code, the spellings a source may use).
/// The first spelling is the one shown, the rest are what `normalise` accepts.
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
    // OMDb names the spoken Chinese, where TMDb gives `zh`, or `cn` for its
    // Cantonese, which `from_tmdb` reads as `zh`.
    ("zh", &["chinese", "mandarin", "cantonese"]),
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
    ("tl", &["tagalog", "filipino"]),
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
    ("RU", &["russia"]),
    // TMDb files a Soviet film under the withdrawn code `SU`.
    ("SU", &["soviet union", "ussr"]),
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
/// TheTVDB answers `jpn`, Routarr's rules are written against `ja`. Every
/// language of [`LANGUAGES`] is listed, the bibliographic form beside the
/// terminological one where they differ (`fre` and `fra`), and a code outside
/// the list answers `None`.
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
        // TheTVDB writes Cantonese `yue` and Taiwanese Mandarin `zhtw`.
        "zho" | "chi" | "yue" | "zhtw" => "zh",
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
        "roh" => "rm",
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
/// TheTVDB answers `usa` and `jpn`, where a rule reads `US` and `JP`. Every
/// country ISO 3166-1 lists, from the Debian iso-codes data: a country left
/// out answers nothing, and a rule on it never matches.
pub fn country_from_alpha3(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase();
    if code.len() == 2 {
        return Some(code.to_uppercase());
    }
    ALPHA3
        .binary_search_by(|(alpha3, _)| (*alpha3).cmp(code.as_str()))
        .ok()
        .map(|index| ALPHA3[index].1.to_string())
}

/// (alpha-3, alpha-2) for every ISO 3166-1 country, sorted by the first.
const ALPHA3: &[(&str, &str)] = &[
    ("abw", "AW"),
    ("afg", "AF"),
    ("ago", "AO"),
    ("aia", "AI"),
    ("ala", "AX"),
    ("alb", "AL"),
    ("and", "AD"),
    ("are", "AE"),
    ("arg", "AR"),
    ("arm", "AM"),
    ("asm", "AS"),
    ("ata", "AQ"),
    ("atf", "TF"),
    ("atg", "AG"),
    ("aus", "AU"),
    ("aut", "AT"),
    ("aze", "AZ"),
    ("bdi", "BI"),
    ("bel", "BE"),
    ("ben", "BJ"),
    ("bes", "BQ"),
    ("bfa", "BF"),
    ("bgd", "BD"),
    ("bgr", "BG"),
    ("bhr", "BH"),
    ("bhs", "BS"),
    ("bih", "BA"),
    ("blm", "BL"),
    ("blr", "BY"),
    ("blz", "BZ"),
    ("bmu", "BM"),
    ("bol", "BO"),
    ("bra", "BR"),
    ("brb", "BB"),
    ("brn", "BN"),
    ("btn", "BT"),
    ("bvt", "BV"),
    ("bwa", "BW"),
    ("caf", "CF"),
    ("can", "CA"),
    ("cck", "CC"),
    ("che", "CH"),
    ("chl", "CL"),
    ("chn", "CN"),
    ("civ", "CI"),
    ("cmr", "CM"),
    ("cod", "CD"),
    ("cog", "CG"),
    ("cok", "CK"),
    ("col", "CO"),
    ("com", "KM"),
    ("cpv", "CV"),
    ("cri", "CR"),
    ("cub", "CU"),
    ("cuw", "CW"),
    ("cxr", "CX"),
    ("cym", "KY"),
    ("cyp", "CY"),
    ("cze", "CZ"),
    ("deu", "DE"),
    ("dji", "DJ"),
    ("dma", "DM"),
    ("dnk", "DK"),
    ("dom", "DO"),
    ("dza", "DZ"),
    ("ecu", "EC"),
    ("egy", "EG"),
    ("eri", "ER"),
    ("esh", "EH"),
    ("esp", "ES"),
    ("est", "EE"),
    ("eth", "ET"),
    ("fin", "FI"),
    ("fji", "FJ"),
    ("flk", "FK"),
    ("fra", "FR"),
    ("fro", "FO"),
    ("fsm", "FM"),
    ("gab", "GA"),
    ("gbr", "GB"),
    ("geo", "GE"),
    ("ggy", "GG"),
    ("gha", "GH"),
    ("gib", "GI"),
    ("gin", "GN"),
    ("glp", "GP"),
    ("gmb", "GM"),
    ("gnb", "GW"),
    ("gnq", "GQ"),
    ("grc", "GR"),
    ("grd", "GD"),
    ("grl", "GL"),
    ("gtm", "GT"),
    ("guf", "GF"),
    ("gum", "GU"),
    ("guy", "GY"),
    ("hkg", "HK"),
    ("hmd", "HM"),
    ("hnd", "HN"),
    ("hrv", "HR"),
    ("hti", "HT"),
    ("hun", "HU"),
    ("idn", "ID"),
    ("imn", "IM"),
    ("ind", "IN"),
    ("iot", "IO"),
    ("irl", "IE"),
    ("irn", "IR"),
    ("irq", "IQ"),
    ("isl", "IS"),
    ("isr", "IL"),
    ("ita", "IT"),
    ("jam", "JM"),
    ("jey", "JE"),
    ("jor", "JO"),
    ("jpn", "JP"),
    ("kaz", "KZ"),
    ("ken", "KE"),
    ("kgz", "KG"),
    ("khm", "KH"),
    ("kir", "KI"),
    ("kna", "KN"),
    ("kor", "KR"),
    ("kwt", "KW"),
    ("lao", "LA"),
    ("lbn", "LB"),
    ("lbr", "LR"),
    ("lby", "LY"),
    ("lca", "LC"),
    ("lie", "LI"),
    ("lka", "LK"),
    ("lso", "LS"),
    ("ltu", "LT"),
    ("lux", "LU"),
    ("lva", "LV"),
    ("mac", "MO"),
    ("maf", "MF"),
    ("mar", "MA"),
    ("mco", "MC"),
    ("mda", "MD"),
    ("mdg", "MG"),
    ("mdv", "MV"),
    ("mex", "MX"),
    ("mhl", "MH"),
    ("mkd", "MK"),
    ("mli", "ML"),
    ("mlt", "MT"),
    ("mmr", "MM"),
    ("mne", "ME"),
    ("mng", "MN"),
    ("mnp", "MP"),
    ("moz", "MZ"),
    ("mrt", "MR"),
    ("msr", "MS"),
    ("mtq", "MQ"),
    ("mus", "MU"),
    ("mwi", "MW"),
    ("mys", "MY"),
    ("myt", "YT"),
    ("nam", "NA"),
    ("ncl", "NC"),
    ("ner", "NE"),
    ("nfk", "NF"),
    ("nga", "NG"),
    ("nic", "NI"),
    ("niu", "NU"),
    ("nld", "NL"),
    ("nor", "NO"),
    ("npl", "NP"),
    ("nru", "NR"),
    ("nzl", "NZ"),
    ("omn", "OM"),
    ("pak", "PK"),
    ("pan", "PA"),
    ("pcn", "PN"),
    ("per", "PE"),
    ("phl", "PH"),
    ("plw", "PW"),
    ("png", "PG"),
    ("pol", "PL"),
    ("pri", "PR"),
    ("prk", "KP"),
    ("prt", "PT"),
    ("pry", "PY"),
    ("pse", "PS"),
    ("pyf", "PF"),
    ("qat", "QA"),
    ("reu", "RE"),
    ("rou", "RO"),
    ("rus", "RU"),
    ("rwa", "RW"),
    ("sau", "SA"),
    ("sdn", "SD"),
    ("sen", "SN"),
    ("sgp", "SG"),
    ("sgs", "GS"),
    ("shn", "SH"),
    ("sjm", "SJ"),
    ("slb", "SB"),
    ("sle", "SL"),
    ("slv", "SV"),
    ("smr", "SM"),
    ("som", "SO"),
    ("spm", "PM"),
    ("srb", "RS"),
    ("ssd", "SS"),
    ("stp", "ST"),
    ("sur", "SR"),
    ("svk", "SK"),
    ("svn", "SI"),
    ("swe", "SE"),
    ("swz", "SZ"),
    ("sxm", "SX"),
    ("syc", "SC"),
    ("syr", "SY"),
    ("tca", "TC"),
    ("tcd", "TD"),
    ("tgo", "TG"),
    ("tha", "TH"),
    ("tjk", "TJ"),
    ("tkl", "TK"),
    ("tkm", "TM"),
    ("tls", "TL"),
    ("ton", "TO"),
    ("tto", "TT"),
    ("tun", "TN"),
    ("tur", "TR"),
    ("tuv", "TV"),
    ("twn", "TW"),
    ("tza", "TZ"),
    ("uga", "UG"),
    ("ukr", "UA"),
    ("umi", "UM"),
    ("ury", "UY"),
    ("usa", "US"),
    ("uzb", "UZ"),
    ("vat", "VA"),
    ("vct", "VC"),
    ("ven", "VE"),
    ("vgb", "VG"),
    ("vir", "VI"),
    ("vnm", "VN"),
    ("vut", "VU"),
    ("wlf", "WF"),
    ("wsm", "WS"),
    ("yem", "YE"),
    ("zaf", "ZA"),
    ("zmb", "ZM"),
    ("zwe", "ZW"),
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
