//! Properties that hold for any input, where an example covers only the
//! writings somebody thought of: the folders an Arr writes, the values a rule
//! folds, the stamps a row stores, the mount point, a field of the CSV export
//! and a language's name.

use proptest::prelude::*;
use unicode_normalization::UnicodeNormalization;

use crate::paths;

/// A folder's name with no space at either end, which `paths::trimmed` would
/// take for padding around a typed path, and no `.` or `..`.
fn name() -> impl Strategy<Value = String> {
    "[A-Za-z0-9._-]([A-Za-z0-9 ._-]{0,6}[A-Za-z0-9._-])?"
        .prop_filter("a name, not a step", |name| name != "." && name != "..")
}

/// A folder as Linux writes it, from the root.
fn linux() -> impl Strategy<Value = String> {
    prop::collection::vec(name(), 0..5).prop_map(|names| format!("/{}", names.join("/")))
}

/// A folder as Windows writes it: on a drive, either separator, or on a share.
fn windows() -> impl Strategy<Value = String> {
    let drive = ("[A-Za-z]", prop::collection::vec(name(), 0..4), prop::bool::ANY).prop_map(
        |(letter, names, slashes)| {
            let separator = if slashes { "/" } else { "\\" };
            format!("{letter}:{separator}{}", names.join(separator))
        },
    );
    let share =
        (name(), name(), prop::collection::vec(name(), 0..3)).prop_map(|(server, share, names)| {
            let rest: String = names.iter().map(|name| format!("\\{name}")).collect();
            format!("\\\\{server}\\{share}{rest}")
        });
    prop_oneof![drive, share]
}

fn folder() -> impl Strategy<Value = String> {
    prop_oneof![linux(), windows()]
}

fn separator_of(path: &str) -> &'static str {
    if paths::is_windows(path) { "\\" } else { "/" }
}

proptest! {
    /// A key names its folder as the folder itself does.
    #[test]
    fn a_key_is_its_own_key(path in folder()) {
        prop_assert_eq!(paths::key(&paths::key(&path)), paths::key(&path));
    }

    /// The separator an Arr closes a root folder with names no other folder.
    #[test]
    fn a_closing_separator_changes_no_folder(path in folder()) {
        let closed = format!("{path}{}", separator_of(&path));
        prop_assert!(paths::same(&path, &closed), "{path} against {closed}");
        prop_assert!(paths::within(&closed, &path) && paths::within(&path, &closed));
    }

    /// Windows reads a name whatever its case, and Linux reads it as written.
    #[test]
    fn case_tells_folders_apart_on_linux_alone(path in folder()) {
        let shouted = path.to_uppercase();
        if paths::is_windows(&path) {
            prop_assert!(paths::same(&path, &shouted), "{path}");
        } else {
            prop_assert_eq!(paths::same(&path, &shouted), path == shouted);
        }
    }

    /// A folder holds itself and what lies beneath it, and neither a sibling
    /// that shares its letters nor a path that climbs out of it.
    #[test]
    fn a_folder_holds_what_lies_beneath_it_and_nothing_else(
        folder in folder(),
        child in name(),
    ) {
        let separator = separator_of(&folder);
        let under = if folder.ends_with(separator) {
            format!("{folder}{child}")
        } else {
            format!("{folder}{separator}{child}")
        };
        prop_assert!(paths::within(&folder, &folder));
        prop_assert!(paths::within(&under, &folder), "{under} under {folder}");
        let climbing = format!("{under}{separator}..{separator}..{separator}{child}");
        prop_assert!(!paths::within(&climbing, &folder), "{climbing} under {folder}");
        if paths::trimmed(&folder).len() > 3 {
            let sibling = format!("{}-4k", paths::trimmed(&folder));
            prop_assert!(!paths::within(&sibling, &folder), "{sibling} under {folder}");
        }
    }

    /// SQL's `path` collation and `same` agree on every pair.
    #[test]
    fn the_collation_agrees_with_same(a in folder(), b in folder()) {
        let equal = paths::collate(&a, &b) == std::cmp::Ordering::Equal;
        prop_assert_eq!(equal, paths::same(&a, &b));
        prop_assert_eq!(paths::collate(&a, &a), std::cmp::Ordering::Equal);
    }

    /// A folded value folds to itself, and an accent written apart from its
    /// letter folds as the letter written with it, the hard cases of the
    /// folding table among the values tried.
    #[test]
    fn a_folded_value_is_settled_whatever_its_accents(raw in "\\PC{0,24}") {
        use crate::services::rule_engine::normalise_value;
        let cases: Vec<(String, String)> = serde_json::from_str(include_str!(
            "../services/rule_engine/normalise_value_cases.json"
        ))
        .unwrap();
        for raw in cases.into_iter().map(|(raw, _)| raw).chain([raw]) {
            let folded = normalise_value(&raw);
            prop_assert_eq!(normalise_value(&folded), folded.clone(), "{:?}", raw);
            let composed: String = raw.nfc().collect();
            let decomposed: String = raw.nfd().collect();
            prop_assert_eq!(normalise_value(&composed), normalise_value(&decomposed), "{:?}", raw);
        }
    }

    /// A stamp reads back as the second it was written at.
    #[test]
    fn a_stamp_reads_back_as_its_second(seconds in 0i64..4_102_444_800, nanos in 0u32..1_000_000_000) {
        use crate::services::routing::{format_timestamp, parse_timestamp};
        let written = chrono::DateTime::from_timestamp(seconds, nanos).unwrap();
        let read = parse_timestamp(&format_timestamp(written));
        prop_assert_eq!(read, chrono::DateTime::from_timestamp(seconds, 0));
    }

    /// A mount point is settled once normalised: empty, or one leading slash
    /// and none closing it.
    #[test]
    fn a_mount_point_is_settled_once_normalised(
        names in prop::collection::vec("[A-Za-z0-9._~-]{1,6}", 0..4),
        lead in "/{0,2}",
        close in "/{0,2}",
        pad in " {0,2}",
    ) {
        use crate::config::normalise_base_path;
        let raw = format!("{pad}{lead}{}{close}{pad}", names.join("/"));
        let mount = normalise_base_path(&raw);
        prop_assert_eq!(normalise_base_path(&mount), mount.clone());
        prop_assert!(mount.is_empty() || (mount.starts_with('/') && !mount.ends_with('/')), "{mount:?}");
    }

    /// A field of the export reads back whole, on one line, and a spreadsheet
    /// takes none of it for a formula. Each sign that opens a formula leads a
    /// value of every run, which a random one seldom starts with.
    #[test]
    fn an_exported_field_reads_back_whole_and_runs_nothing(
        chars in prop::collection::vec(any::<char>(), 0..30),
    ) {
        let tail: String = chars.into_iter().collect();
        for lead in ["", "=", "+", "-", "@", "\t", "\"", "\n"] {
            let raw = format!("{lead}{tail}");
            let field = crate::api::move_log::csv_field(&raw);
            let inner = field
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("a quoted field");
            prop_assert!(!inner.contains(['\n', '\r']), "{field:?}");
            prop_assert!(!inner.replace("\"\"", "").contains('"'), "a lone quote: {field:?}");
            let read = inner.replace("\"\"", "\"");
            let one_line = raw.replace(['\n', '\r'], " ");
            let formula = one_line.starts_with(['=', '+', '-', '@', '\t']);
            let expected = if formula { format!("'{one_line}") } else { one_line };
            prop_assert_eq!(read, expected);
        }
    }

    /// A language named once is named the same the second time.
    #[test]
    fn a_language_named_is_settled(raw in "[A-Za-z ()]{0,20}") {
        use crate::integrations::language::normalise;
        if let Some(named) = normalise(&raw) {
            prop_assert_eq!(normalise(&named), Some(named.clone()));
        }
    }
}
