use sqlx::AssertSqlSafe;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

use crate::config::Config;
use crate::crypto;

/// Migrations embedded in the binary, applied in order, exactly once.
///
/// Adding a `.sql` file to `migrations/` is not enough: it must be listed here.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial_schema", include_str!("../migrations/001_initial_schema.sql")),
    ("002_onboarding", include_str!("../migrations/002_onboarding.sql")),
    ("003_metadata_sources", include_str!("../migrations/003_metadata_sources.sql")),
    ("004_orphaned_proposals", include_str!("../migrations/004_orphaned_proposals.sql")),
    ("005_jikan_film_misses", include_str!("../migrations/005_jikan_film_misses.sql")),
    ("006_drop_override_lock", include_str!("../migrations/006_drop_override_lock.sql")),
    ("007_job_detail_keys", include_str!("../migrations/007_job_detail_keys.sql")),
    ("008_drop_move_files_default", include_str!("../migrations/008_drop_move_files_default.sql")),
];

/// Initialize the SQLite connection pool and run migrations.
pub async fn init_pool(config: &Config) -> Result<SqlitePool, sqlx::Error> {
    if let Err(e) = std::fs::create_dir_all(&config.data_dir)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        // Said here, because the error SQLite gives afterwards is "unable to
        // open database file" and names neither the directory nor the reason.
        // The shape this catches is the ordinary first run: Docker creates a
        // missing bind-mount directory owned by root, and the container is
        // uid 1000.
        tracing::error!("Cannot create the data directory {}: {e}", config.data_dir.display());
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
/// has to read it, and migrations only ever go forward, so there is no way
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
/// when the *last* connection closes, which dropping the pool does not do.
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
    apply_migrations(pool, MIGRATIONS).await
}

/// Apply the migrations up to `last` included, leaving the schema of the
/// release that shipped `last` for a test to upgrade from.
#[cfg(test)]
pub async fn run_migrations_through(pool: &SqlitePool, last: &str) -> Result<(), sqlx::Error> {
    let end = MIGRATIONS.iter().position(|(name, _)| *name == last).expect("a listed migration");
    apply_migrations(pool, &MIGRATIONS[..=end]).await
}

async fn apply_migrations(
    pool: &SqlitePool,
    migrations: &[(&str, &str)],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    for (name, sql) in migrations {
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
/// inside `BEGIN ... END` trigger bodies, so both are tracked here. Blocks are
/// counted by whole words: a `CASE` also closes with `END`, and `ended` is no
/// `END`.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut word = String::new();
    let mut in_string = false;
    let mut block_depth = 0usize;
    let mut chars = sql.chars().peekable();

    while let Some(c) = chars.next() {
        if !in_string && (c.is_alphanumeric() || c == '_') {
            word.push(c);
            current.push(c);
            continue;
        }
        count_block_word(&mut word, &mut block_depth);
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
            _ => current.push(c),
        }
    }
    count_block_word(&mut word, &mut block_depth);
    push_statement(&mut statements, &mut current);

    statements
}

/// Count a whole word that opens or closes a block, and start the next one.
fn count_block_word(word: &mut String, block_depth: &mut usize) {
    if word.eq_ignore_ascii_case("BEGIN") || word.eq_ignore_ascii_case("CASE") {
        *block_depth += 1;
    } else if word.eq_ignore_ascii_case("END") {
        *block_depth = block_depth.saturating_sub(1);
    }
    word.clear();
}

fn push_statement(statements: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }
    current.clear();
}

/// The `?, ?, ?` an `IN (...)` binds `n` values through.
///
/// One spelling for every `IN` list. The bind ceiling behind it is stated only
/// where a list is chunked, in `routing::BIND_CHUNK`.
/// Every caller keeps `AssertSqlSafe` at its own site: the fragment is made
/// of `?` alone, and the values it stands for are bound, never spliced.
pub fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

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

/// In-memory pool with the full schema applied, for tests.
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

    /// A table rebuilt through a copy carries none of the original's indexes,
    /// so the lookups the sync and the enrichment make on these two tables
    /// are pinned by name.
    #[tokio::test]
    async fn the_root_folder_and_identifier_indexes_exist() {
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
    /// In WAL mode an unfolded `routarr.db-wal` holds every table, so the backup
    /// the README describes (stop, copy the `.db`) would restore a database with
    /// **no tables at all**. The documentation relies on this property, checked
    /// against a real file rather than the in-memory pool the rest of the suite
    /// uses.
    #[tokio::test]
    async fn a_closed_database_needs_no_sidecar_files_to_be_complete() {
        let dir = crate::tests::TempDir::new("wal");
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
        // process exits, and within a test the pool close leaves it truncated to
        // zero, which contributes exactly as much: nothing.
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

    /// A `CASE` closes with `END` as a trigger's block does, and a word that
    /// only starts like a keyword is none: counted otherwise, a trigger is cut
    /// at the first `;` inside it, or two statements are run as one.
    #[test]
    fn a_trigger_stays_one_statement_whatever_its_body_holds() {
        let sql = "CREATE TRIGGER t AFTER UPDATE ON a BEGIN
                     UPDATE b SET ended = CASE WHEN new.x THEN 1 ELSE 0 END;
                     UPDATE b SET beginner = 1;
                   END;
                   SELECT 1;";
        let s = split_statements(sql);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].starts_with("CREATE TRIGGER") && s[0].ends_with("END"), "{s:?}");
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
