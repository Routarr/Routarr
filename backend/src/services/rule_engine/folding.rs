//! Value folding, shared by matching and by the rule editor.

/// The form two spellings of one value must share to be treated as equal.
///
/// Sources disagree on case, accents and punctuation for what is the same
/// value: `Science-Fiction` against `Science Fiction`, `Comédie` against
/// `Comedie`. It normalises and never guesses: distinct values stay distinct
/// and no synonym is invented, so `Sci-Fi` is still not `Science Fiction`.
pub fn normalise_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut gap = false;
    for c in raw.chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(c.to_lowercase().map(fold_diacritic));
        } else {
            gap = true;
        }
    }
    out
}

/// Latin letters that carry a mark, reduced to the letter underneath. Only the
/// one-to-one cases: `ß` and `œ` expand, and no genre needs them. The rule
/// editor holds a copy (`FOLDED` in `frontend/src/api/conditions.ts`), and
/// `normalise_value_cases.json` holds both to the same answers.
fn fold_diacritic(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => c,
    }
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
}
