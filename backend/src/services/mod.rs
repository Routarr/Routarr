pub mod accounts;
pub mod applications;
pub mod audit;
pub mod auto_apply;
pub mod backup;
pub mod connection;
pub mod enrichment;
pub mod executor;
pub mod maintenance;
pub mod metadata;
pub mod notify;
pub mod oidc;
pub mod placement;
pub mod quota;
pub mod rate_limit;
pub mod routing;
pub mod rule_engine;
pub mod rule_health;
pub mod rule_tests;
pub mod settings;
pub mod sync;
pub mod templates;
pub mod webhook_check;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Split in two so that this file, which the scan reads, does not name them.
    const HANDLERS: &str = concat!("crate::", "api");
    const SERVICES: &str = concat!("crate::", "services");
    const GROUP: &str = concat!("crate::", "{");

    /// Every line of code in these directories of `src/`, and in the
    /// directories under them, naming a path into `module`.
    fn naming(directories: &[&str], module: &str) -> Vec<String> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for directory in directories {
            for file in sources_under(&src.join(directory)) {
                let source = std::fs::read_to_string(&file).expect("a source file");
                let lines: Vec<&str> = source.lines().collect();
                for number in lines_naming(&source, module) {
                    let line = lines[number].trim();
                    found.push(format!("{}:{}: {line}", file.display(), number + 1));
                }
            }
        }
        found
    }

    /// The `.rs` files under `directory`, at any depth, in a stable order: a
    /// service split into a directory of its own is a service still.
    fn sources_under(directory: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![directory.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("a source directory") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    /// The lines of `source`, from zero, whose code names a path into `module`,
    /// a `crate::{...}` group counting on the line it opens. Comments are
    /// skipped, since a doc link names a module without depending on it.
    fn lines_naming(source: &str, module: &str) -> Vec<usize> {
        let code: Vec<&str> = source.lines().map(without_comment).collect();
        let mut found: Vec<usize> = code
            .iter()
            .enumerate()
            .filter(|(_, line)| names(line, module))
            .map(|(at, _)| at)
            .collect();
        let joined = code.join("\n");
        let prefix = GROUP.trim_end_matches('{');
        for (at, _) in joined.match_indices(GROUP) {
            let group = &joined[at + prefix.len()..];
            if ungrouped(prefix, group).iter().any(|path| names(path, module)) {
                found.push(joined[..at].matches('\n').count());
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }

    /// `line` up to its comment: the first `//` that does not follow a
    /// colon, as the one of an address (`http://`) does.
    fn without_comment(line: &str) -> &str {
        line.match_indices("//")
            .find(|(at, _)| !line[..*at].ends_with(':'))
            .map_or(line, |(at, _)| &line[..at])
    }

    /// Whether `code` names `module` itself, not a longer name it begins.
    fn names(code: &str, module: &str) -> bool {
        code.match_indices(module).any(|(at, _)| {
            !code[at + module.len()..]
                .starts_with(|next: char| next.is_alphanumeric() || next == '_')
        })
    }

    /// The paths the group opening `group` names, each after `prefix`, a nested
    /// group left whole: `{api::media, error::{A, B}}` after `crate::` names
    /// `crate::api::media` and `crate::error::{A, B}`.
    fn ungrouped(prefix: &str, group: &str) -> Vec<String> {
        let mut paths = Vec::new();
        let (mut depth, mut start) = (0, 1);
        for (at, next) in group.char_indices() {
            match next {
                '{' => depth += 1,
                '}' | ',' if depth == 1 => {
                    let item = group[start..at].trim();
                    if !item.is_empty() {
                        paths.push(format!("{prefix}{item}"));
                    }
                    if next == '}' {
                        break;
                    }
                    start = at + 1;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        paths
    }

    /// No service or model names a handler, or a change to the HTTP layer
    /// ripples into the logic it calls.
    #[test]
    fn neither_a_service_nor_a_model_reaches_up_into_the_handlers() {
        let found = naming(&["services", "models"], HANDLERS);
        assert!(found.is_empty(), "these reach up into the handlers:\n{}", found.join("\n"));
    }

    /// A model, which every layer reads, names no service.
    #[test]
    fn a_model_reaches_up_into_no_service() {
        let found = naming(&["models"], SERVICES);
        assert!(found.is_empty(), "these reach up into the services:\n{}", found.join("\n"));
    }

    /// The scan finds the paths it looks for, nested directories included, so
    /// its silence elsewhere proves something.
    #[test]
    fn the_layer_scan_finds_the_paths_the_handlers_hold() {
        assert!(!naming(&["api"], HANDLERS).is_empty(), "the scan saw no path into the handlers");
        assert!(!naming(&["api"], SERVICES).is_empty(), "the scan saw no path into the services");
        let services = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("services");
        assert!(
            sources_under(&services).iter().any(|file| file.parent() != Some(services.as_path())),
            "the scan reads no service held in a directory of its own"
        );
    }

    /// A path into the handlers is found however it is written, grouped,
    /// split, renamed or after an address. A nested module of that name, a
    /// longer name and a comment are not.
    #[test]
    fn the_layer_scan_finds_a_path_however_it_is_written() {
        for source in [
            format!("use {GROUP}api::media::metadata_predicate, error::AppError}};"),
            format!("use {GROUP}\n    error::AppError,\n    api::{{media, rules}},\n}};"),
            format!("use {GROUP}api as handlers}};"),
            format!("let at = (\"http://127.0.0.1:1\", {HANDLERS}::media::metadata_predicate);"),
        ] {
            assert!(!lines_naming(&source, HANDLERS).is_empty(), "missed in {source}");
        }
        for source in [
            format!("use {GROUP}services::{{routing, api}}, error}};"),
            format!("use {HANDLERS}s::media;"),
            format!("use {GROUP}error}}; // {HANDLERS}"),
        ] {
            assert!(lines_naming(&source, HANDLERS).is_empty(), "found in {source}");
        }
    }
}
