//! What a certification code means, where that is not in dispute.
//!
//! `U`, `TP` and `TV-PG` say nothing to most readers, and the panel that exists
//! to show what the library holds was listing them bare. Beside `language.rs`
//! because it answers the same kind of question — what does this code stand for
//! — and for the same reason: the rule is written against the code, and the
//! name is only ever what is shown.
//!
//! Only codes whose meaning agrees across the systems that use them are named.
//! `12` is twelve-and-over for the BBFC, the FSK and the CNC alike, so it is
//! safe; `M` is fifteen-and-over in Australia and something else in the United
//! States, so it is left bare. A wrong name on a right value is worse than no
//! name: the value is what the engine matches, and the reader would trust the
//! name.
//!
//! The media row carries the certification and not the system that issued it,
//! so nothing here can disambiguate — which is precisely why the list is
//! restricted to what needs no disambiguation.

/// What a code stands for, as a dictionary key and its parameter.
pub enum Meaning {
    AllAges,
    Guidance,
    /// Suitable from this age.
    From(u8),
    NotRated,
}

/// The meaning of a certification code, or `None` for one we will not guess at.
pub fn meaning(code: &str) -> Option<Meaning> {
    let key = code.trim().to_ascii_uppercase();
    let key = key.as_str();
    Some(match key {
        // Everyone: the BBFC's U, Spain's TP, the MPA's G, the American
        // television G and Y, the Dutch AL, Brazil's Livre.
        "U" | "TP" | "G" | "TV-G" | "TV-Y" | "AL" | "L" | "0" => Meaning::AllAges,
        // A recommendation to a parent rather than a bound.
        "PG" | "TV-PG" => Meaning::Guidance,
        "TV-Y7" => Meaning::From(7),
        "PG-13" => Meaning::From(13),
        "TV-14" => Meaning::From(14),
        // Both mean "an adult must be present under 17" in the American
        // systems, which is the only place either is issued.
        "R" | "TV-MA" => Meaning::From(17),
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
        assert!(matches!(meaning("U"), Some(Meaning::AllAges)));
        assert!(matches!(meaning("tv-pg"), Some(Meaning::Guidance)));
        assert!(matches!(meaning("R"), Some(Meaning::From(17))));
        // Fifteen-and-over in Australia, something else in the United States.
        assert!(meaning("M").is_none());
        assert!(meaning("Sortie nationale").is_none());
    }

    #[test]
    fn a_bare_number_is_an_age_and_a_large_one_is_not() {
        assert!(matches!(meaning("12"), Some(Meaning::From(12))));
        assert!(matches!(meaning(" 16 "), Some(Meaning::From(16))));
        // A year, or a count that wandered in: not an age, so not named.
        assert!(meaning("1999").is_none());
        assert!(meaning("42").is_none());
    }
}
