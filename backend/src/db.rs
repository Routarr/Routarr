use sqlx::AssertSqlSafe;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

use crate::config::Config;
use crate::crypto;

/// Migrations embedded in the binary, applied in order, exactly once.
///
/// Adding a `.sql` file to `migrations/` is not enough — it must be listed here.
const MIGRATIONS: &[(&str, &str)] =
    &[("001_initial_schema", include_str!("../migrations/001_initial_schema.sql"))];

/// Initialize the SQLite connection pool and run migrations.
pub async fn init_pool(config: &Config) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = config.db_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        // Said here, because the error SQLite gives afterwards is "unable to
        // open database file" and names neither the directory nor the reason.
        // The shape this catches is the ordinary first run: Docker creates a
        // missing bind-mount directory owned by root, and the container is
        // uid 1000.
        tracing::error!("Cannot create the data directory {}: {e}", parent.display());
    }

    // PRAGMAs belong on the connect options: setting them with a one-off query
    // only configures whichever pooled connection happened to serve it.
    let options = SqliteConnectOptions::from_str(&config.database_url())?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // NORMAL is the documented companion of WAL: durable across app crashes,
        // and an order of magnitude faster than FULL for the write bursts a
        // simulation produces.
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .foreign_keys(true)
        // Without this, concurrent writers surface as "database is locked"
        // instead of waiting their turn.
        .busy_timeout(Duration::from_secs(10));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(15))
        .connect_with(options)
        .await?;

    let on_disk = config.db_path != *":memory:";
    if on_disk {
        crypto::restrict_permissions(&config.db_path);
        info!("Database connected at {}", config.db_path.display());
    }

    run_migrations(&pool).await?;

    // After the migrations, not before: in WAL mode the sidecars only exist
    // once something has been written.
    if on_disk {
        crypto::restrict_database_permissions(&config.db_path);
    }

    Ok(pool)
}

/// Whether a migration name is ahead of everything this build knows.
///
/// A backup taken on a newer Routarr carries a schema this binary has no
/// migration for. Restoring it would leave the database ahead of the code that
/// has to read it — and migrations only ever go forward, so there is no way
/// back. Refusing is the only honest answer.
pub fn is_newer_schema(name: &str) -> bool {
    // "unknown" comes from a database with no migration recorded at all, which
    // is older than anything, not newer.
    if name.is_empty() || name == "unknown" {
        return false;
    }
    !MIGRATIONS.iter().any(|(known, _)| *known == name)
}

/// Fold the write-ahead log back into the database file, then close the pool.
///
/// In WAL mode a commit lands in `routarr.db-wal`, and SQLite only checkpoints
/// when the *last* connection closes — which dropping the pool does not do.
/// Without this, a stopped Routarr leaves an almost-empty `routarr.db` beside a
/// WAL holding everything, and the documented "copy the database" backup takes
/// a file with no tables in it.
///
/// A failure is logged rather than propagated: the data is still in the WAL and
/// the next start recovers it.
pub async fn checkpoint_and_close(pool: &SqlitePool) {
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(pool).await {
        tracing::warn!("Could not checkpoint the write-ahead log: {e}");
    }
    pool.close().await;
}

/// Apply any migration not yet recorded in `_migrations`.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    run_migrations_upto(pool).await
}

async fn run_migrations_upto(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    for (name, sql) in MIGRATIONS {
        let already_applied: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?)")
                .bind(name)
                .fetch_one(pool)
                .await?;

        if already_applied {
            continue;
        }

        info!("Running migration: {}", name);

        // One transaction per migration: a failure half-way leaves the schema
        // untouched instead of half-applied and unrecorded.
        let mut tx = pool.begin().await?;
        for statement in split_statements(sql) {
            sqlx::query(AssertSqlSafe(statement.as_str())).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO _migrations (name) VALUES (?)")
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        info!("Migration {} applied successfully", name);
    }

    info!("All migrations up to date");
    Ok(())
}

/// Split a migration into statements.
///
/// Naively splitting on `;` breaks on semicolons inside string literals and
/// inside `BEGIN ... END` trigger bodies, so both are tracked here.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut block_depth = 0usize;
    let mut chars = sql.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '-' if !in_string && chars.peek() == Some(&'-') => {
                // Line comment: drop through to the newline.
                for c in chars.by_ref() {
                    if c == '\n' {
                        current.push('\n');
                        break;
                    }
                }
            }
            '\'' => {
                // '' inside a string is an escaped quote, not a terminator.
                if in_string && chars.next_if_eq(&'\'').is_some() {
                    current.push('\'');
                    current.push('\'');
                } else {
                    in_string = !in_string;
                    current.push(c);
                }
            }
            ';' if !in_string && block_depth == 0 => {
                push_statement(&mut statements, &mut current);
            }
            _ => {
                current.push(c);
                if !in_string {
                    let upper = current.to_uppercase();
                    if upper.ends_with("BEGIN") && is_word_boundary(&upper, "BEGIN") {
                        block_depth += 1;
                    } else if upper.ends_with("END") && is_word_boundary(&upper, "END") {
                        block_depth = block_depth.saturating_sub(1);
                    }
                }
            }
        }
    }
    push_statement(&mut statements, &mut current);

    statements
}

fn push_statement(statements: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }
    current.clear();
}

/// True when the keyword at the end of `haystack` is not part of a longer word.
fn is_word_boundary(haystack: &str, keyword: &str) -> bool {
    let before = haystack.len() - keyword.len();
    before == 0
        || !haystack[..before].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// The `?, ?, ?` an `IN (...)` binds `n` values through.
///
/// Written once because nine sites spelled it, and the bind ceiling behind
/// it — see `routing::BIND_CHUNK` — is only stated where a list is chunked.
/// Every caller keeps `AssertSqlSafe` at its own site: the fragment is made
/// of `?` alone, and the values it stands for are bound, never spliced.
pub fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// In-memory pool with the full schema applied, for tests.
/// Escape `%`, `_` and `\` in user input destined for a `LIKE ? ESCAPE '\'`
/// pattern. Without it, searching for "100%" matches every title starting with
/// "100", and a stray backslash changes the meaning of whatever follows it.
pub fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys=ON").execute(&pool).await.unwrap();
    run_migrations(&pool).await.expect("migrations");
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 018 rebuilt `root_folders` through a copy, and a copied table carries
    /// none of the indexes the original had: the two declared by 001 and 002
    /// were gone on every upgraded and every fresh database alike.
    #[tokio::test]
    async fn the_indexes_a_rebuilt_table_lost_are_declared_again() {
        let pool = test_pool().await;
        let indexes: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master
              WHERE type = 'index' AND tbl_name IN ('root_folders', 'source_identifiers')
                AND name NOT LIKE 'sqlite_%'
              ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let names: Vec<&str> = indexes.iter().map(|(n,)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "idx_root_folders_category",
                "idx_root_folders_instance",
                "idx_source_identifiers_external",
                "idx_source_identifiers_resolved",
            ]
        );
    }

    #[test]
    fn placeholders_are_one_question_mark_per_value() {
        assert_eq!(placeholders(0), "");
        assert_eq!(placeholders(1), "?");
        assert_eq!(placeholders(3), "?, ?, ?");
    }

    /// A stopped Routarr must leave a database file that is complete on its own.
    ///
    /// In WAL mode it did not: `routarr.db` stayed at 4 KB while `routarr.db-wal`
    /// held every table, so the backup the README described — stop, copy the
    /// `.db` — restored a database with **no tables at all**. This asserts the
    /// property the documentation now relies on, against a real file rather than
    /// the in-memory pool the rest of the suite uses.
    #[tokio::test]
    async fn a_closed_database_needs_no_sidecar_files_to_be_complete() {
        let dir = std::env::temp_dir().join(format!("routarr-wal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());

        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await.unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::query("INSERT INTO categories (id, name) VALUES ('c-1', 'survives')")
            .execute(&pool)
            .await
            .unwrap();

        // The write is in the WAL at this point, not in the database file.
        assert!(path.with_extension("db-wal").exists(), "expected a WAL to exist while running");

        checkpoint_and_close(&pool).await;

        // The WAL must no longer carry anything. SQLite removes the file when the
        // process exits; within a test the pool close leaves it truncated to
        // zero, which contributes exactly as much — nothing.
        let wal = path.with_extension("db-wal");
        let leftover = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(leftover, 0, "the WAL still holds {leftover} bytes after closing");

        // Copy only the `.db`, exactly as the documented backup does.
        let copy = dir.join("backup.db");
        std::fs::copy(&path, &copy).unwrap();
        let restored = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", copy.display()))
            .await
            .unwrap();
        let name: String = sqlx::query_scalar("SELECT name FROM categories WHERE id = 'c-1'")
            .fetch_one(&restored)
            .await
            .expect("the copied file must carry the data on its own");
        assert_eq!(name, "survives");

        restored.close().await;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn splits_plain_statements() {
        let s = split_statements("CREATE TABLE a (x INT);\nCREATE TABLE b (y INT);");
        assert_eq!(s.len(), 2);
        assert!(s[1].starts_with("CREATE TABLE b"));
    }

    #[test]
    fn keeps_semicolons_inside_string_literals() {
        let s = split_statements("INSERT INTO t VALUES ('a;b');\nSELECT 1;");
        assert_eq!(s.len(), 2);
        assert!(s[0].contains("'a;b'"));
    }

    #[test]
    fn handles_escaped_quotes() {
        let s = split_statements("INSERT INTO t VALUES ('it''s; fine');");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn keeps_trigger_bodies_intact() {
        let sql = "CREATE TRIGGER t AFTER INSERT ON x BEGIN UPDATE y SET a = 1; END;\nSELECT 1;";
        let s = split_statements(sql);
        assert_eq!(s.len(), 2, "trigger body must stay in one statement: {s:?}");
        assert!(s[0].contains("UPDATE y SET a = 1"));
    }

    #[test]
    fn drops_line_comments() {
        let s = split_statements("-- a comment; not a statement\nSELECT 1;");
        assert_eq!(s.len(), 1);
        assert!(s[0].contains("SELECT 1"));
    }

    #[test]
    fn trailing_statement_without_semicolon_is_kept() {
        let s = split_statements("SELECT 1;\nSELECT 2");
        assert_eq!(s.len(), 2);
    }

    #[tokio::test]
    async fn migrations_apply_cleanly_and_are_idempotent() {
        let pool = crate::db::test_pool().await;
        // A second run must be a no-op rather than an error.
        run_migrations(&pool).await.unwrap();

        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();

        for expected in ["instances", "media", "rules", "decisions", "jobs", "settings"] {
            assert!(tables.iter().any(|t| t == expected), "missing table {expected}");
        }
    }
}
