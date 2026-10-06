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
pub mod rate_limit;
pub mod routing;
pub mod rule_engine;
pub mod rule_health;
pub mod rule_tests;
pub mod settings;
pub mod sync;

#[cfg(test)]
mod tests {
    /// Split in two so that this file, which the scan reads, does not name them.
    const HANDLERS: &str = concat!("crate::", "api");
    const SERVICES: &str = concat!("crate::", "services");

    /// Every line of code in these directories of `src/` naming a path into
    /// `module`. Comments are skipped: a doc link names a module without
    /// depending on it.
    fn naming(directories: &[&str], module: &str) -> Vec<String> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for directory in directories {
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(src.join(directory))
                .expect("a source directory")
                .map(|entry| entry.expect("a directory entry").path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
                .collect();
            files.sort();
            for file in files {
                let source = std::fs::read_to_string(&file).expect("a source file");
                for (number, line) in source.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    let names_it = code.match_indices(module).any(|(at, _)| {
                        !code[at + module.len()..]
                            .starts_with(|next: char| next.is_alphanumeric() || next == '_')
                    });
                    if names_it {
                        found.push(format!("{}:{}: {}", file.display(), number + 1, line.trim()));
                    }
                }
            }
        }
        found
    }

    /// The layers run `api/`, then `services/`, then `integrations/`, with the
    /// models under all three. A service or a model reaching up into a handler
    /// makes a change to the HTTP layer ripple into the logic it calls.
    #[test]
    fn neither_a_service_nor_a_model_reaches_up_into_the_handlers() {
        let found = naming(&["services", "models"], HANDLERS);
        assert!(found.is_empty(), "these reach up into the handlers:\n{}", found.join("\n"));
    }

    /// A model is a shape every layer reads, so it names no service: one that
    /// did would make a change to the logic ripple into every reader.
    #[test]
    fn a_model_reaches_up_into_no_service() {
        let found = naming(&["models"], SERVICES);
        assert!(found.is_empty(), "these reach up into the services:\n{}", found.join("\n"));
    }

    /// The scan finds what it looks for, or its silence proves nothing.
    #[test]
    fn the_layer_scan_finds_the_paths_the_handlers_hold() {
        assert!(!naming(&["api"], HANDLERS).is_empty(), "the scan saw no path into the handlers");
        assert!(!naming(&["api"], SERVICES).is_empty(), "the scan saw no path into the services");
    }
}
