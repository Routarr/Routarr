//! Value folding, shared by matching and by the rule editor.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The form two spellings of one value share to compare equal. Case, punctuation and a Latin
/// letter's accents fold away, however a source wrote them (`Science-Fiction` is `Science
/// Fiction`, `Shōgun` is `shogun`). Other scripts keep their marks, which tell letters apart
/// (`ガ` is not `カ`) and stay inside their word. No synonym is invented: `Sci-Fi` is not
/// `Science Fiction`.
pub fn normalise_value(raw: &str) -> String {
    #[cfg(test)]
    FOLDED.with(|folded| folded.set(folded.get() + 1));
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

#[cfg(test)]
thread_local! {
    static FOLDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What `run` returns, and how many values it folded on this thread.
#[cfg(test)]
pub(crate) fn folds_in<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let before = FOLDED.with(std::cell::Cell::get);
    let done = run();
    (done, FOLDED.with(std::cell::Cell::get) - before)
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

/// True when any wanted value is among the title's, both folded through [`normalise_value`]
/// and the blanks left out of `wanted`.
pub(super) fn contains_any(straw: &[String], wanted: &[String]) -> bool {
    wanted.iter().any(|value| straw.contains(value))
}

/// True when every wanted value is among the title's, folded as for [`contains_any`], and false
/// for none at all: a condition with no value is unfinished, and `Condition::is_empty` refuses it.
pub(super) fn contains_all(straw: &[String], wanted: &[String]) -> bool {
    !wanted.is_empty() && wanted.iter().all(|value| straw.contains(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule editor's copy (`canonicalKey` in `frontend/src/api/conditions.ts`) reads this same
    /// table: a pair the two fold apart is a value the editor drops or a rule the engine refuses.
    #[test]
    fn the_folding_the_rule_editor_shares_is_the_one_matching_uses() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("normalise_value_cases.json")).unwrap();
        for (raw, folded) in cases {
            assert_eq!(normalise_value(&raw), folded, "{raw:?}");
        }
    }

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
