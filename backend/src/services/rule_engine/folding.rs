//! Value folding, shared by matching and by the rule editor.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The form two spellings of one value must share to be treated as equal.
///
/// Sources disagree on case, accents and punctuation for what is the same
/// value: `Science-Fiction` against `Science Fiction`, `Comédie` against
/// `Comedie`. It normalises and never guesses: distinct values stay distinct
/// and no synonym is invented, so `Sci-Fi` is still not `Science Fiction`.
///
/// A Latin letter loses its accents, however a source wrote them: `Shōgun` is
/// `shogun`, and an `é` written as `e` and a combining accent is `e`. Other
/// scripts keep theirs, which tell letters apart: `ガ` is not `カ`, nor `й`
/// `и`. A mark stays inside its word rather than splitting it.
pub fn normalise_value(raw: &str) -> String {
    let mut latin = false;
    let unaccented = raw.nfkd().filter(|&c| {
        if is_latin_accent(c) {
            return !latin;
        }
        latin = is_latin_letter(c);
        true
    });
    let mut out = String::with_capacity(raw.len());
    let mut gap = false;
    for c in unaccented.nfc() {
        if c.is_alphanumeric() || is_combining_mark(c) {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(c.to_lowercase().map(fold_stroke));
        } else {
            gap = true;
        }
    }
    out
}

/// The accents a decomposed Latin letter carries, the block of combining
/// diacritical marks. The rule editor reads the same block
/// (`frontend/src/api/conditions.ts`).
fn is_latin_accent(c: char) -> bool {
    ('\u{300}'..='\u{36F}').contains(&c)
}

/// The Latin letters, as decomposition leaves them: the basic ones, and the
/// extended blocks where a letter with no accent to shed lives.
fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || ('\u{C0}'..='\u{24F}').contains(&c)
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

/// The Latin letters whose mark is drawn into them and does not decompose,
/// reduced to the letter underneath. Only the one-to-one cases: `ß` and `œ`
/// expand, and no genre needs them. The rule editor holds a copy
/// (`STROKED` in `frontend/src/api/conditions.ts`), and
/// `normalise_value_cases.json` holds both to the same answers.
fn fold_stroke(c: char) -> char {
    match c {
        'ø' => 'o',
        'ł' => 'l',
        'đ' => 'd',
        'ħ' => 'h',
        'ı' => 'i',
        _ => c,
    }
}

/// An Arr's status as one word, folded: Radarr writes `inCinemas`, and a
/// person `In Cinemas`.
pub fn status_key(raw: &str) -> String {
    normalise_value(raw).replace(' ', "")
}

/// The form two spellings of one rating code share: case and the separator
/// aside, every character counts. `R+` is MyAnimeList's rating for mild
/// nudity, and folded as free text it would be every film rated `R`. The
/// library's facets group codes the same way (`api::media`).
pub fn certification_key(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c.to_ascii_lowercase() })
        .collect()
}

/// True when any needle equals any straw, compared through [`normalise_value`].
///
/// The `any` quantifier: the values of a condition are alternatives.
pub(super) fn contains_any(haystack: &[String], needles: &[String]) -> bool {
    if needles.is_empty() {
        return false;
    }
    let straw: Vec<String> = haystack.iter().map(|h| normalise_value(h)).collect();
    needles.iter().map(|n| normalise_value(n)).filter(|n| !n.is_empty()).any(|n| straw.contains(&n))
}

/// True when every needle is among the straw.
///
/// The `all` quantifier, and the reason a rule needs no second condition to
/// require two genres at once. An empty list matches nothing rather than
/// everything: a condition with no operand is a condition nobody finished
/// writing, and `Condition::is_empty` refuses it before it can be stored.
pub(super) fn contains_all(haystack: &[String], needles: &[String]) -> bool {
    let straw: Vec<String> = haystack.iter().map(|h| normalise_value(h)).collect();
    let wanted: Vec<String> =
        needles.iter().map(|n| normalise_value(n)).filter(|n| !n.is_empty()).collect();
    !wanted.is_empty() && wanted.iter().all(|n| straw.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule editor folds with its own copy (`canonicalKey` in
    /// `frontend/src/api/conditions.ts`) to refuse a value given twice, and
    /// reads this same table: a pair one side joins and the other keeps apart
    /// is a value the editor drops or a rule the engine refuses.
    #[test]
    fn the_folding_the_rule_editor_shares_is_the_one_matching_uses() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("normalise_value_cases.json")).unwrap();
        for (raw, folded) in cases {
            assert_eq!(normalise_value(&raw), folded, "{raw:?}");
        }
    }

    /// Folded once is folded for good, and a source writing an accent apart
    /// from its letter names the value one writing them together does.
    #[test]
    fn folding_is_stable_and_blind_to_how_an_accent_is_written() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("normalise_value_cases.json")).unwrap();
        for (raw, folded) in cases {
            assert_eq!(normalise_value(&folded), folded, "{folded:?} folds again");
            let composed: String = raw.nfc().collect();
            let decomposed: String = raw.nfd().collect();
            assert_eq!(normalise_value(&composed), normalise_value(&decomposed), "{raw:?}");
        }
    }

    /// The rule editor keys rating codes with its own copy
    /// (`certificationKey` in `frontend/src/api/conditions.ts`), held to these
    /// same cases.
    #[test]
    fn a_rating_code_keeps_every_character_but_case_and_separators() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("certification_key_cases.json")).unwrap();
        for (raw, key) in cases {
            assert_eq!(certification_key(&raw), key, "{raw:?}");
        }
    }
}
