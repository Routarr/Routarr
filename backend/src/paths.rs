//! Folders as an Arr writes them, on Linux or on Windows.
//!
//! Routarr runs in a container, the Arrs wherever their owner put them, so a
//! path is either the Unix one a container sees (`/data/movies/`) or a Windows
//! one: a drive (`D:\Media\Movies\`) or a share (`\\nas\films`). The two Arrs
//! end a root folder with its separator and a title's folder without one, a
//! person types either separator on Windows, and Windows compares names
//! without their case. Every comparison of two folders goes through [`key`],
//! here and in SQL through the `path` collation, or one of those writings
//! reads as another folder and the title as one to move.

use std::cmp::Ordering;

/// The name SQLite knows [`collate`] by: `a = b COLLATE path`.
pub const COLLATION: &str = "path";

/// Whether `path` is written the Windows way: a drive letter and a colon, or
/// a share opening with two backslashes.
pub fn is_windows(path: &str) -> bool {
    let path = path.trim();
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || path.starts_with("\\\\")
}

/// Whether `path` names a folder from the root of its system: `/…` on Linux,
/// `X:\…` or `\\server\share…` on Windows. `D:movies`, relative to the
/// current folder of a drive, and a share with no share name are not.
pub fn is_absolute(path: &str) -> bool {
    let path = path.trim();
    if let Some(share) = path.strip_prefix("\\\\") {
        let mut parts = share.split(['\\', '/']).filter(|part| !part.is_empty());
        return parts.next().is_some() && parts.next().is_some();
    }
    if is_windows(path) {
        return matches!(path.as_bytes().get(2), Some(b'\\' | b'/'));
    }
    path.starts_with('/')
}

/// `path` without the separators that close it, a root keeping the one it
/// needs (`/`, `D:\`), and a Windows path written with backslashes alone:
/// how Routarr stores a folder it is given.
pub fn trimmed(path: &str) -> String {
    let path = path.trim();
    if !is_windows(path) {
        let rest = path.trim_end_matches('/');
        return if rest.is_empty() && path.starts_with('/') { "/".into() } else { rest.into() };
    }
    let unified = path.replace('/', "\\");
    let rest = unified.trim_end_matches('\\');
    if rest.len() == 2 && rest.ends_with(':') { format!("{rest}\\") } else { rest.to_string() }
}

/// Whether `path` names its folder plainly: no `.` or `..` segment, which
/// names another folder than its letters read (`/data/movies/../tv` is
/// `/data/tv`), and no empty one, which names the folder of a shorter
/// spelling that no comparison here takes it for.
pub fn is_plain(path: &str) -> bool {
    let path = trimmed(path);
    let windows = is_windows(&path);
    let separators: &[char] = if windows { &['\\', '/'] } else { &['/'] };
    let rest = path.strip_prefix("\\\\").unwrap_or(&path).trim_end_matches(separators);
    rest.split(separators)
        .enumerate()
        .all(|(at, segment)| !matches!(segment, "." | "..") && (at == 0 || !segment.is_empty()))
}

/// Whether a segment of `path` is `.` or `..`.
fn climbs(path: &str) -> bool {
    let separators: &[char] = if is_windows(path) { &['\\', '/'] } else { &['/'] };
    path.split(separators).any(|segment| matches!(segment, "." | ".."))
}

/// What two writings of one folder share: [`trimmed`], and a Windows path
/// lowered, as Windows reads names whatever their case.
pub fn key(path: &str) -> String {
    let path = trimmed(path);
    if is_windows(&path) { path.to_lowercase() } else { path }
}

/// Whether `a` and `b` name the same folder.
pub fn same(a: &str, b: &str) -> bool {
    key(a) == key(b)
}

/// Whether `path` is `folder` or lies under it, compared by name: a folder
/// does not hold a sibling that shares its letters (`/data/movies` against
/// `/data/movies-4k`), and a Linux folder holds no Windows path.
pub fn within(path: &str, folder: &str) -> bool {
    if is_windows(path) != is_windows(folder) || climbs(path) || climbs(folder) {
        return false;
    }
    let (path, folder) = (key(path), key(folder));
    let separator = if is_windows(&folder) { '\\' } else { '/' };
    path == folder
        || path
            .strip_prefix(folder.as_str())
            .is_some_and(|rest| folder.ends_with(separator) || rest.starts_with(separator))
}

/// The `path` collation: two folders compare as their [`key`]s do.
pub fn collate(a: &str, b: &str) -> Ordering {
    key(a).cmp(&key(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/data/movies/../tv` is `/data/tv`, on another disk perhaps: it is not
    /// under `/data/movies`, whatever its letters say.
    #[test]
    fn a_folder_does_not_hold_a_path_that_climbs_out_of_it() {
        assert!(!within("/data/movies/../tv", "/data/movies"));
        assert!(!within("D:\\Media\\..\\TV", "D:\\Media"));
        assert!(within("/data/movies/anime", "/data/movies"));
        assert!(within("/data/movies/..anime", "/data/movies"), "a name may start with dots");
    }

    #[test]
    fn a_plain_path_has_no_dot_and_no_empty_segment() {
        for plain in
            ["/", "/data/movies/", "D:\\", "D:\\Media\\4K", "\\\\nas\\films", "/data/.hidden"]
        {
            assert!(is_plain(plain), "{plain}");
        }
        for not in ["/data/movies/../tv", "/data/./movies", "/data//movies", "D:\\Media\\..\\TV"] {
            assert!(!is_plain(not), "{not}");
        }
    }

    #[test]
    fn a_folder_reads_the_same_with_or_without_its_closing_separator() {
        assert!(same("/movies/anime/", "/movies/anime"));
        assert!(same("D:\\Media\\Movies\\", "D:\\Media\\Movies"));
        assert!(same("\\\\nas\\films\\", "\\\\nas\\films"));
        assert_eq!(trimmed("/"), "/");
        assert_eq!(trimmed("D:\\"), "D:\\");
        assert_eq!(trimmed("D:/"), "D:\\");
        assert_eq!(trimmed("  /movies/  "), "/movies");
    }

    #[test]
    fn windows_reads_a_name_whatever_its_case_and_either_separator() {
        assert!(same("D:\\Media\\Movies", "d:/media/MOVIES/"));
        assert!(same("\\\\NAS\\Films", "\\\\nas\\films"));
        assert_eq!(trimmed("d:/media/movies/"), "d:\\media\\movies");
        assert_eq!(key("D:\\Média\\Ünïcode"), "d:\\média\\ünïcode");
    }

    #[test]
    fn linux_keeps_the_case_and_the_backslash_of_a_name() {
        assert!(!same("/movies/Anime", "/movies/anime"));
        assert!(!same("/movies/a\\b", "/movies/a/b"));
        assert!(!is_windows("/mnt/d:/x"));
    }

    #[test]
    fn a_folder_holds_what_lies_beneath_it_and_no_sibling() {
        assert!(within("/movies/anime/films", "/movies/anime"));
        assert!(within("/movies/anime", "/movies/anime/"));
        assert!(!within("/movies/anime-4k", "/movies/anime"));
        assert!(within("/anything", "/"));
        assert!(within("d:\\media\\movies\\anime\\films", "D:\\Media\\Movies\\Anime\\"));
        assert!(!within("D:\\Media\\Movies-4k", "D:\\Media\\Movies"));
        assert!(within("D:\\Media", "D:\\"));
        assert!(within("\\\\nas\\films\\4k", "\\\\NAS\\films"));
        assert!(!within("D:\\movies", "/movies"));
        assert!(!within("/movies", "D:\\"));
    }

    #[test]
    fn only_a_path_from_the_root_of_its_system_is_absolute() {
        for absolute in ["/movies", "/", "D:\\Movies", "d:/movies", "D:\\", "\\\\nas\\films"] {
            assert!(is_absolute(absolute), "{absolute}");
        }
        for relative in ["movies", "movies\\4k", "D:movies", "D:", "\\\\nas", "\\\\", ""] {
            assert!(!is_absolute(relative), "{relative}");
        }
    }

    #[test]
    fn the_collation_orders_folders_by_their_key() {
        assert_eq!(collate("D:\\Movies\\", "d:/movies"), Ordering::Equal);
        assert_eq!(collate("/a", "/b"), Ordering::Less);
    }
}
