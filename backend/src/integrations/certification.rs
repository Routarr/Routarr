//! What a certification code means, where that is not in dispute.
//!
//! `U`, `TP` and `TV-PG` say nothing to most readers, and the panel that shows
//! what the library holds would list them bare. It sits beside `language.rs`
//! because it answers the same kind of question (what a code stands for) and
//! for the same reason: the rule is written against the code, and the name is
//! only ever what is shown.
//!
//! Only codes whose meaning agrees across the systems that use them are named.
//! `12` is twelve-and-over for the BBFC, the FSK and the CNC alike, so it is
//! safe. `M` is fifteen-and-over in Australia and something else in the United
//! States, so it is left bare. A wrong name on a right value is worse than no
//! name: the value is what the engine matches, and the reader would trust the
//! name.
//!
//! Where the system that issued a rating is known, its scale (a country code,
//! or MAL for MyAnimeList's), a code it settles is named too: `R` is
//! seventeen and over for the MPA and for MyAnimeList, eighteen in Canada.

/// What a code stands for, as a dictionary key and its parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meaning {
    AllAges,
    Guidance,
    /// Suitable from this age.
    From(u8),
    NotRated,
}

/// The system MyAnimeList rates in, as a rating's scale.
pub const MAL: &str = "MAL";

/// MyAnimeList's own words for one of its codes, as a dictionary key: the
/// codes say little alone, and `R+` and `Rx` are MyAnimeList's alone.
pub fn mal_words(code: &str) -> Option<&'static str> {
    Some(match code.trim().to_ascii_uppercase().as_str() {
        "G" => "CertMalG",
        "PG" => "CertMalPg",
        "PG-13" => "CertMalPg13",
        "R" => "CertMalR",
        "R+" => "CertMalRPlus",
        "RX" => "CertMalRx",
        _ => return None,
    })
}

/// The meaning of a certification code in the system `scale` names, or `None`
/// for one we will not guess at. `None` for `scale` is a system unknown.
pub fn meaning(code: &str, scale: Option<&str>) -> Option<Meaning> {
    let key = code.trim().to_ascii_uppercase();
    let key = key.as_str();
    // Settled by the system: seventeen and over for the MPA and MyAnimeList,
    // eighteen for the Canadian boards, and nothing France or any other system
    // issues.
    if key == "R" {
        return match scale {
            Some("US" | MAL) => Some(Meaning::From(17)),
            Some("CA") => Some(Meaning::From(18)),
            _ => None,
        };
    }
    Some(match key {
        // MyAnimeList's own: mild nudity, then explicit content.
        "R+" => Meaning::From(17),
        "RX" => Meaning::From(18),
        // Everyone: the BBFC's U, Spain's TP, the MPA's G, the American
        // television G and Y, the Dutch AL, Brazil's Livre.
        "U" | "TP" | "G" | "TV-G" | "TV-Y" | "AL" | "L" | "0" => Meaning::AllAges,
        // A recommendation to a parent rather than a bound.
        "PG" | "TV-PG" => Meaning::Guidance,
        "TV-Y7" => Meaning::From(7),
        "PG-13" => Meaning::From(13),
        "TV-14" => Meaning::From(14),
        // "An adult must be present under 17" in the American television
        // system, the only one issuing it. `R` is left bare: the MPA's
        // seventeen is Canada's eighteen.
        "TV-MA" => Meaning::From(17),
        "NC-17" | "X" | "R18+" | "18+" => Meaning::From(18),
        "12A" => Meaning::From(12),
        "15A" => Meaning::From(15),
        "NR" | "UR" | "UNRATED" | "NOT RATED" | "N/A" => Meaning::NotRated,
        // A bare number is an age everywhere it is used, and saying so is what
        // tells "12" apart from a count.
        _ => {
            let age: u8 = key.parse().ok()?;
            if age > 21 {
                return None;
            }
            Meaning::From(age)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_code_is_named_only_where_the_systems_agree() {
        assert!(matches!(meaning("U", None), Some(Meaning::AllAges)));
        assert!(matches!(meaning("tv-pg", None), Some(Meaning::Guidance)));
        assert!(matches!(meaning("TV-MA", None), Some(Meaning::From(17))));
        // Fifteen-and-over in Australia, something else in the United States.
        assert!(meaning("M", None).is_none());
        // Seventeen-and-over with an adult in the United States, eighteen in
        // Canada.
        assert!(meaning("R", None).is_none());
        assert!(meaning("Sortie nationale", None).is_none());
    }

    #[test]
    fn myanimelists_own_codes_carry_their_age() {
        assert!(matches!(meaning("R+", None), Some(Meaning::From(17))));
        assert!(matches!(meaning("Rx", None), Some(Meaning::From(18))));
        assert!(matches!(meaning("R", Some(MAL)), Some(Meaning::From(17))));
        assert_eq!(mal_words("R+"), Some("CertMalRPlus"));
        assert_eq!(mal_words("PG-13"), Some("CertMalPg13"));
        assert_eq!(mal_words("TV-14"), None);
    }

    #[test]
    fn r_is_named_once_its_country_is_known() {
        assert!(matches!(meaning("R", Some("US")), Some(Meaning::From(17))));
        assert!(matches!(meaning("R", Some("CA")), Some(Meaning::From(18))));
        assert!(meaning("R", Some("FR")).is_none(), "France rates no film R");
        assert!(meaning("R", None).is_none());
    }

    #[test]
    fn a_bare_number_is_an_age_and_a_large_one_is_not() {
        assert!(matches!(meaning("12", None), Some(Meaning::From(12))));
        assert!(matches!(meaning(" 16 ", None), Some(Meaning::From(16))));
        // A year, or a count that wandered in: not an age, so not named.
        assert!(meaning("1999", None).is_none());
        assert!(meaning("42", None).is_none());
    }
}
