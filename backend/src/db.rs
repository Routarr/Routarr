use sqlx::AssertSqlSafe;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

use crate::config::Config;
use crate::crypto;

/// Migrations embedded in the binary, applied in order, exactly once. A `.sql` file in
/// `migrations/` runs only once listed here. Each release's files fold into one before its tag.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_release_0_1_5", include_str!("../migrations/001_release_0_1_5.sql")),
    ("002_release_0_2_0", include_str!("../migrations/002_release_0_2_0.sql")),
    ("003_rule_template", include_str!("../migrations/003_rule_template.sql")),
    ("004_rule_test_verdicts", include_str!("../migrations/004_rule_test_verdicts.sql")),
];

/// The names a database Routarr 0.1.5 left holds for what `001_release_0_1_5` builds, in the
/// order they ran. An archive 0.1.5 took is named by the last, an older release's by another.
pub(crate) const NAMES_OF_0_1_5: [&str; 13] = [
    "001_initial_schema",
    "002_onboarding",
    "003_metadata_sources",
    "004_orphaned_proposals",
    "005_jikan_film_misses",
    "006_drop_override_lock",
    "007_job_detail_keys",
    "008_drop_move_files_default",
    "009_application_keys",
    "010_job_result",
    "011_webhook_secrets",
    "012_hashed_sessions",
    "013_opened_by",
];

/// How large the write-ahead log stays once checkpointed, in bytes.
const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// Refuse a database that cannot live where it is configured, naming the path and the
/// remedy, since SQLite past this point names neither. A folder Docker made for a bind
/// mount belongs to root while the image runs as uid 1000.
pub fn prepare_data_dir(config: &Config) -> crate::error::AppResult<()> {
    use crate::error::AppError;
    if config.db_path == *":memory:" {
        return Ok(());
    }
    if config.db_path.is_dir() {
        let path = config.db_path.display();
        let file = config.db_path.join("routarr.db");
        return Err(AppError::Config(format!(
            "ROUTARR_DB_PATH names the directory {path}. Name the database file inside it, such \
             as {}",
            file.display()
        )));
    }
    let dir = &config.data_dir;
    let unwritable = |e: std::io::Error| {
        AppError::Config(format!(
            "The data directory {} cannot be written ({e}). Give it to the user Routarr runs as, \
             uid 1000 in the image (chown -R 1000:1000 on the host), or run the container as the \
             folder's owner with user:",
            dir.display()
        ))
    };
    std::fs::create_dir_all(dir).map_err(unwritable)?;
    let probe = dir.join(".routarr-write-check");
    std::fs::File::create(&probe).map_err(unwritable)?;
    std::fs::remove_file(&probe).map_err(unwritable)?;
    Ok(())
}

/// Initialize the SQLite connection pool and run migrations.
pub async fn init_pool(config: &Config) -> crate::error::AppResult<SqlitePool> {
    prepare_data_dir(config)?;

    // A path, never a URL, which would read `%41` as `A` and end the name at a `?`.
    let file = if config.db_path == *":memory:" {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else {
        SqliteConnectOptions::new().filename(&config.db_path)
    };
    // PRAGMAs go on the connect options: a one-off query sets only the connection serving it.
    let options = with_paths(file)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // A power cut may lose the last commits, so `record_outcome` writes moves under FULL.
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .foreign_keys(true)
        // Concurrent writers wait their turn instead of failing with "database is locked".
        .busy_timeout(Duration::from_secs(10))
        // The log grows to the largest transaction and is trimmed back to this once checkpointed.
        .pragma("journal_size_limit", JOURNAL_SIZE_LIMIT.to_string())
        // The planner's statistics, refreshed on what each connection ran.
        .optimize_on_close(true, Some(400));

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

    // Refused with the way back: an offline restore of the newest archive this build opens.
    if let Some(newer) = opened_ahead(&mut *pool.acquire().await?).await? {
        let passphrase =
            crate::services::backup::stored_passphrase(config, &pool).await.ok().flatten();
        let archive = crate::services::backup::newest_openable(config, passphrase.as_ref())
            .unwrap_or_else(|| "<archive>".to_string());
        return Err(crate::error::AppError::Config(format!(
            "{}. Start that release again, or restore a backup this one can open, with the \
             server stopped: routarr restore {archive}",
            ahead_of_this_build(&newer)
        )));
    }
    if on_disk && migrates_a_schema(&pool).await? {
        match crate::services::backup::before_migrating(config, &pool).await {
            Ok(file) => info!(
                "Backup taken before migrating to v{}: {}",
                env!("CARGO_PKG_VERSION"),
                file.name
            ),
            // No reason to stay on the old schema: the archives taken before this start remain.
            Err(e) => tracing::error!("No backup could be taken before migrating: {e}"),
        }
    }

    run_migrations(&pool).await?;

    // After the migrations: in WAL mode the sidecars exist only once something is written.
    if on_disk {
        crypto::restrict_database_permissions(&config.db_path);
    }

    Ok(pool)
}

/// Whether a migration name is ahead of everything this build knows. Migrations only go
/// forward, so a database ahead of the code reading it has no way back and is refused.
pub fn is_newer_schema(name: &str) -> bool {
    // "unknown" is a database with no migration recorded, older than anything.
    if name.is_empty() || name == "unknown" {
        return false;
    }
    !MIGRATIONS.iter().any(|(known, _)| *known == name) && !NAMES_OF_0_1_5.contains(&name)
}

/// Whether a migration name is one a release older than 0.1.5 stopped at. A database or an
/// archive at it goes through 0.1.5 before this release opens it.
pub fn is_before_0_1_5(name: &str) -> bool {
    NAMES_OF_0_1_5[..NAMES_OF_0_1_5.len() - 1].contains(&name)
}

/// Whether running the migrations would change a schema some release already built: the
/// database holds migrations, and this build lists one it lacks.
async fn migrates_a_schema(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let migrated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !migrated {
        return Ok(false);
    }
    let applied: Vec<String> =
        sqlx::query_scalar("SELECT name FROM _migrations").fetch_all(pool).await?;
    Ok(!applied.is_empty() && MIGRATIONS.iter().any(|(name, _)| !applied.iter().any(|a| a == name)))
}

/// Fold the write-ahead log back into the database file, then close the pool, since SQLite
/// checkpoints only when the last connection closes. A failure is only logged: the next
/// start recovers the log.
pub async fn checkpoint_and_close(pool: &SqlitePool) {
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(pool).await {
        tracing::warn!("Could not checkpoint the write-ahead log: {e}");
    }
    pool.close().await;
}

/// Apply any migration not yet recorded in `_migrations`, after refusing a database a newer
/// release migrated, or one an older release than 0.1.5 left.
pub async fn run_migrations(pool: &SqlitePool) -> crate::error::AppResult<()> {
    refuse_a_newer_schema(pool).await?;
    read_the_names_of_0_1_5(pool).await?;
    apply_migrations(pool, MIGRATIONS).await?;
    Ok(record_this_release(pool).await?)
}

/// Record a database 0.1.5 left at `001_release_0_1_5`, keeping its earliest date as the
/// install day, so the migrations after it apply on top. One short of 0.1.5's last name is
/// refused, naming 0.1.5, since no migration here starts from its schema.
async fn read_the_names_of_0_1_5(pool: &SqlitePool) -> crate::error::AppResult<()> {
    let migrated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !migrated {
        return Ok(());
    }
    let applied: Vec<String> =
        sqlx::query_scalar("SELECT name FROM _migrations").fetch_all(pool).await?;
    if !applied.iter().any(|name| NAMES_OF_0_1_5.contains(&name.as_str())) {
        return Ok(());
    }
    if !applied.iter().any(|name| name == NAMES_OF_0_1_5[NAMES_OF_0_1_5.len() - 1]) {
        return Err(crate::error::AppError::Config(format!(
            "the database was left by a Routarr older than v0.1.5. Start v0.1.5 on it once, \
             which brings it to the schema this v{} starts from",
            env!("CARGO_PKG_VERSION")
        )));
    }

    // Any other name came from a build between releases: built over, it would fail half way.
    if let Some(other) = applied.iter().find(|name| {
        !NAMES_OF_0_1_5.contains(&name.as_str())
            && !MIGRATIONS.iter().any(|(known, _)| known == name)
    }) {
        return Err(crate::error::AppError::Config(format!(
            "the database holds the migration {other}, which no release shipped beside those of \
             v0.1.5: restore an archive a release took, or start from a new database"
        )));
    }

    let names = placeholders(NAMES_OF_0_1_5.len());
    let mut tx = pool.begin().await?;
    let mut installed = sqlx::query_scalar::<_, Option<String>>(AssertSqlSafe(format!(
        "SELECT MIN(applied_at) FROM _migrations WHERE name IN ({names})"
    )));
    let mut forget =
        sqlx::query(AssertSqlSafe(format!("DELETE FROM _migrations WHERE name IN ({names})")));
    for name in NAMES_OF_0_1_5 {
        installed = installed.bind(name);
        forget = forget.bind(name);
    }
    let installed = installed.fetch_one(&mut *tx).await?;
    forget.execute(&mut *tx).await?;
    sqlx::query("INSERT INTO _migrations (name, applied_at) VALUES (?, ?)")
        .bind(MIGRATIONS[0].0)
        .bind(installed)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    info!("The schema v0.1.5 built is recorded as {}", MIGRATIONS[0].0);
    Ok(())
}

/// Refuse a database a newer release migrated further than this build knows, whose schema
/// this one would read and write blind.
async fn refuse_a_newer_schema(pool: &SqlitePool) -> crate::error::AppResult<()> {
    match opened_ahead(&mut *pool.acquire().await?).await? {
        None => Ok(()),
        Some(newer) => Err(crate::error::AppError::Config(ahead_of_this_build(&newer))),
    }
}

/// Why a database `newer` migrated is refused.
fn ahead_of_this_build(newer: &str) -> String {
    format!(
        "the database was opened by Routarr v{newer}, which migrated it further than this \
         v{} knows",
        env!("CARGO_PKG_VERSION")
    )
}

/// The newer release that migrated this database further than this build knows, if one
/// did: it recorded a migration this build does not list, or none. Names alone cannot tell,
/// since a database no newer release opened passes whatever it holds.
pub async fn opened_ahead(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Option<String>, sqlx::Error> {
    let recorded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_opened_by')",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !recorded {
        return Ok(None);
    }
    let with_schema: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('_opened_by') WHERE name = 'schema')",
    )
    .fetch_one(&mut *connection)
    .await?;
    let sql = if with_schema {
        "SELECT version, schema FROM _opened_by"
    } else {
        "SELECT version, NULL FROM _opened_by"
    };
    let openers: Vec<(String, Option<String>)> =
        sqlx::query_as(sql).fetch_all(&mut *connection).await?;
    let this = release(env!("CARGO_PKG_VERSION"));
    Ok(openers
        .into_iter()
        .filter(|(version, schema)| {
            release(version) > this && schema.as_deref().is_none_or(is_newer_schema)
        })
        .max_by_key(|(version, _)| release(version))
        .map(|(version, _)| version))
}

/// Record this release, and the last migration it applied, as having opened the database.
async fn record_this_release(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO _opened_by (version, schema) VALUES (?, ?)
         ON CONFLICT(version) DO UPDATE SET schema = excluded.schema",
    )
    .bind(env!("CARGO_PKG_VERSION"))
    .bind(MIGRATIONS.last().map(|(name, _)| *name))
    .execute(pool)
    .await?;
    Ok(())
}

/// A version as numbers to compare, `0.1.10` after `0.1.9`, a part not a number counting as 0.
fn release(version: &str) -> Vec<u64> {
    version.split(['.', '-']).take(3).map(|part| part.parse().unwrap_or(0)).collect()
}

/// Apply the migrations up to `last` included, leaving the schema of the release that shipped
/// `last` for a test to upgrade from.
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

        // Off outside the transaction, or a table rebuilt by copy loses what points at it.
        let mut connection = pool.acquire().await?;
        sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *connection).await?;
        let applied = apply_migration(&mut connection, name, sql).await;
        let restored = sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *connection).await;
        applied?;
        restored?;

        info!("Migration {} applied successfully", name);
    }

    info!("All migrations up to date");
    Ok(())
}

/// One migration and its record, in one transaction: a failure half-way leaves the schema
/// untouched instead of half-applied and unrecorded.
async fn apply_migration(
    connection: &mut sqlx::SqliteConnection,
    name: &str,
    sql: &str,
) -> Result<(), sqlx::Error> {
    use sqlx::Connection;
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
    // SQLite's own parser splits the script, so a quoted `;` or a trigger body stays whole.
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut *tx).await?;
    let orphans: Vec<(String,)> = sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&mut *tx)
        .await?;
    if let Some((table,)) = orphans.first() {
        return Err(sqlx::Error::Protocol(format!(
            "migration {name} leaves rows of {table} pointing at nothing"
        )));
    }
    sqlx::query("INSERT INTO _migrations (name) VALUES (?)").bind(name).execute(&mut *tx).await?;
    tx.commit().await
}

/// The `?, ?, ?` an `IN (...)` binds `n` values through, chunked by `routing::BIND_CHUNK`.
/// Each caller keeps `AssertSqlSafe` at its own site, the values bound, never spliced.
pub fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Escape `%`, `_` and `\` in user input for a `LIKE ? ESCAPE '\'` pattern. Unescaped, "100%"
/// matches every title starting with "100", and a backslash changes what follows it.
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

/// Conditions on one aliased table, each written as ` AND ...` and its value bound in order.
/// Only `&'static str` names reach the text, so a statement built from them is safe to mark.
#[derive(Debug, Clone)]
pub struct Conditions {
    alias: &'static str,
    sql: String,
    binds: Vec<String>,
}

impl Conditions {
    pub fn on(alias: &'static str) -> Self {
        Self { alias, sql: String::new(), binds: Vec::new() }
    }

    fn push(&mut self, column: &str, test: &str, value: String) -> &mut Self {
        self.sql.push_str(&format!(" AND {}.{column} {test}", self.alias));
        self.binds.push(value);
        self
    }

    /// The column holds `value`, when there is one.
    pub fn equal(&mut self, column: &'static str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.push(column, "= ?", value.to_string()),
            None => self,
        }
    }

    /// The column names the folder `value` names, as `crate::paths::same` reads two.
    pub fn same_path(&mut self, column: &'static str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.push(column, "= ? COLLATE path", value.to_string()),
            None => self,
        }
    }

    /// The column holds `value` anywhere, ASCII letters in any case.
    pub fn contains(&mut self, column: &'static str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => {
                self.push(column, "LIKE ? ESCAPE '\\'", format!("%{}%", escape_like(value)))
            }
            None => self,
        }
    }

    /// The stored timestamp in the column is `value` or later.
    pub fn from(&mut self, column: &'static str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.push(column, ">= ?", value.to_string()),
            None => self,
        }
    }

    /// The stored timestamp in the column is earlier than `value`.
    pub fn before(&mut self, column: &'static str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.push(column, "< ?", value.to_string()),
            None => self,
        }
    }

    /// A condition with nothing to bind, written for this alias by its caller.
    pub fn holds(&mut self, condition: &'static str) -> &mut Self {
        self.sql.push_str(&format!(" AND ({condition})"));
        self
    }

    /// A condition with nothing to bind that must not hold.
    pub fn fails(&mut self, condition: &'static str) -> &mut Self {
        self.sql.push_str(&format!(" AND NOT ({condition})"));
        self
    }

    /// What to append after a `WHERE`, each condition led by ` AND`.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The values the conditions bind, in the order they were written.
    pub fn binds(&self) -> &[String] {
        &self.binds
    }
}

/// A transaction that takes the write lock as it starts, for a check and the write it
/// guards: under a plain `BEGIN` another writer can change what was checked in between.
pub async fn write_transaction(
    pool: &SqlitePool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// `options` with the `path` collation, which compares two folders as [`crate::paths::key`]
/// does. A connection opened without it fails every statement that names it.
pub fn with_paths(options: SqliteConnectOptions) -> SqliteConnectOptions {
    options.collation(crate::paths::COLLATION, crate::paths::collate)
}

/// In-memory pool with the full schema applied, for tests.
#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(with_paths(SqliteConnectOptions::from_str("sqlite::memory:").unwrap()))
        .await
        .expect("in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys=ON").execute(&pool).await.unwrap();
    run_migrations(&pool).await.expect("migrations");
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A newer release that migrated no further than this build leaves a database this build
    /// reads as its own: going back a patch release is no reason to stop.
    #[tokio::test]
    async fn a_newer_release_with_the_same_schema_is_not_refused() {
        let pool = test_pool().await;
        let last = MIGRATIONS.last().unwrap().0;
        sqlx::query("INSERT INTO _opened_by (version, schema) VALUES ('99.0.0', ?)")
            .bind(last)
            .execute(&pool)
            .await
            .unwrap();

        run_migrations(&pool).await.expect("a database of this schema was refused");
    }

    /// A database a newer release migrated further, or opened without saying how far, is
    /// refused naming that release. A name no release lists, with no newer opener, is not.
    #[tokio::test]
    async fn a_newer_release_that_migrated_further_is_refused() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO _migrations (name) VALUES ('900_listed_by_no_release')")
            .execute(&pool)
            .await
            .unwrap();
        run_migrations(&pool).await.expect("an unknown migration alone was refused");

        for (version, schema) in [("99.0.0", Some("099_from_a_later_release")), ("98.0.0", None)] {
            let pool = test_pool().await;
            sqlx::query("INSERT INTO _opened_by (version, schema) VALUES (?, ?)")
                .bind(version)
                .bind(schema)
                .execute(&pool)
                .await
                .unwrap();

            let refused = run_migrations(&pool).await;

            let Err(crate::error::AppError::Config(message)) = refused else {
                panic!("a database v{version} migrated further was opened: {refused:?}");
            };
            assert!(message.contains(&format!("v{version}")), "{message}");
        }
        assert!(release("0.1.10") > release("0.1.9"));
    }

    /// A database 0.1.5 left is recorded at `001_release_0_1_5` and upgrades on top,
    /// keeping the day Routarr was installed, which the overdue-backup warning reads.
    #[tokio::test]
    async fn a_database_0_1_5_left_upgrades_and_keeps_the_day_it_was_installed() {
        let pool = crate::tests::database_of_0_1_5().await;
        sqlx::query(
            "UPDATE _migrations SET applied_at = CASE name
                WHEN '001_initial_schema' THEN '2026-09-21 10:00:00'
                ELSE '2026-10-04 10:00:00' END",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_migrations(&pool).await.expect("a database 0.1.5 left was refused");

        let recorded: Vec<(String, String)> =
            sqlx::query_as("SELECT name, applied_at FROM _migrations ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<&str> = recorded.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, MIGRATIONS.iter().map(|(name, _)| *name).collect::<Vec<_>>());
        assert_eq!(recorded[0].1, "2026-09-21 10:00:00", "the day of the install was lost");
    }

    /// A database an older release than 0.1.5 left holds a schema no migration here starts
    /// from: refused, naming 0.1.5, and left as it was.
    #[tokio::test]
    async fn a_database_older_than_0_1_5_is_refused_naming_the_release_to_go_through() {
        let pool = crate::tests::database_of_0_1_5().await;
        sqlx::query(
            "DELETE FROM _migrations WHERE name IN ('012_hashed_sessions', '013_opened_by')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let refused = run_migrations(&pool).await;

        let Err(crate::error::AppError::Config(message)) = refused else {
            panic!("a database 0.1.4 left was opened: {refused:?}");
        };
        assert!(message.contains("v0.1.5"), "{message}");
        let names: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _migrations").fetch_one(&pool).await.unwrap();
        assert_eq!(names, 11, "the refused database was changed");
    }

    /// A database a build between two releases migrated holds 0.1.5's names and others no
    /// release shipped: refused, naming one, and left as it was rather than built over half way.
    #[tokio::test]
    async fn a_database_a_build_between_releases_migrated_is_refused_untouched() {
        let pool = crate::tests::database_of_0_1_5().await;
        sqlx::query("INSERT INTO _migrations (name) VALUES ('039_job_error_code')")
            .execute(&pool)
            .await
            .unwrap();

        let refused = run_migrations(&pool).await;

        let Err(crate::error::AppError::Config(message)) = refused else {
            panic!("a database a development build migrated was opened: {refused:?}");
        };
        assert!(message.contains("039_job_error_code"), "{message}");
        let names: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _migrations").fetch_one(&pool).await.unwrap();
        assert_eq!(names, 14, "the refused database was changed");
    }

    /// An archive names its schema by the last migration it holds: one 0.1.5 took is of a
    /// schema this build upgrades, one an older release took is before it, and neither is ahead.
    #[test]
    fn an_archives_schema_reads_as_of_0_1_5_before_it_or_ahead() {
        for of_0_1_5 in ["013_opened_by", "001_release_0_1_5", "002_release_0_2_0"] {
            assert!(!is_newer_schema(of_0_1_5) && !is_before_0_1_5(of_0_1_5), "{of_0_1_5}");
        }
        for older in ["001_initial_schema", "010_job_result", "012_hashed_sessions"] {
            assert!(is_before_0_1_5(older) && !is_newer_schema(older), "{older}");
        }
        assert!(is_newer_schema("039_job_error_code"), "a name no release shipped is ahead");
    }

    /// The bundled SQLite fixes CVE-2025-6965, CVE-2025-29087 and CVE-2025-3277. RustSec
    /// does not flag a bundled copy, so nothing else notices the lock falling behind.
    #[tokio::test]
    async fn the_bundled_sqlite_has_its_known_flaws_fixed() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let version: String =
            sqlx::query_scalar("SELECT sqlite_version()").fetch_one(&pool).await.unwrap();
        let parts: Vec<u32> = version.split('.').map(|part| part.parse().unwrap()).collect();
        assert!(parts >= vec![3, 50, 2], "SQLite {version} predates 3.50.2");
    }

    /// A table rebuilt through a copy carries none of the original's indexes, so the lookups the
    /// sync and the enrichment make on these two tables are pinned by name.
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
                "idx_source_identifiers_external",
                "idx_source_identifiers_local",
                "idx_source_identifiers_resolved",
            ]
        );
    }

    /// A pool holding one instance, one title and one override of it.
    async fn library_with_an_override() -> SqlitePool {
        let pool = test_pool().await;
        for statement in [
            "INSERT INTO instances (id, name, instance_type, base_url, api_key)
             VALUES ('i-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k')",
            "INSERT INTO media (id, instance_id, arr_id, media_type, title)
             VALUES ('m-1', 'i-1', 10, 'movie', 'Totoro')",
            "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-1', 'm-1', 'anime')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn overrides(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM overrides").fetch_one(pool).await.unwrap()
    }

    /// SQLite changes a CHECK or a column type only by rebuilding the table, and dropping the
    /// original under foreign keys deletes what points at it.
    #[tokio::test]
    async fn a_migration_that_rebuilds_a_parent_table_keeps_its_children() {
        let pool = library_with_an_override().await;

        apply_migrations(
            &pool,
            &[(
                "test_rebuild",
                "CREATE TABLE media_new (id TEXT PRIMARY KEY, instance_id TEXT NOT NULL,
                     arr_id INTEGER NOT NULL, media_type TEXT NOT NULL, title TEXT NOT NULL);
                 INSERT INTO media_new SELECT id, instance_id, arr_id, media_type, title FROM media;
                 DROP TABLE media;
                 ALTER TABLE media_new RENAME TO media;",
            )],
        )
        .await
        .unwrap();

        assert_eq!(overrides(&pool).await, 1, "the rebuild deleted the override");
        let enforced: i64 =
            sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&pool).await.unwrap();
        assert_eq!(enforced, 1, "foreign keys stayed off after the migration");
    }

    /// With foreign keys off while it runs, a migration is held to them before it commits.
    #[tokio::test]
    async fn a_migration_that_leaves_a_row_pointing_at_nothing_is_refused() {
        let pool = library_with_an_override().await;

        let refused =
            apply_migrations(&pool, &[("test_orphan", "DELETE FROM media WHERE id = 'm-1';")])
                .await;

        assert!(refused.is_err(), "the orphaned override was committed");
        let titles: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&pool).await.unwrap();
        assert_eq!((titles, overrides(&pool).await), (1, 1), "the migration was not rolled back");
    }

    /// A migration is read as SQLite reads it: a block comment and a quoted name may hold a `;`
    /// or a `'` without cutting the statement.
    #[tokio::test]
    async fn a_migration_is_read_as_sqlite_reads_it() {
        let pool = test_pool().await;

        apply_migrations(
            &pool,
            &[(
                "test_parsing",
                "/* a note; it's not a statement */
                 CREATE TABLE \"a;b\" (x INTEGER);
                 CREATE TRIGGER t AFTER INSERT ON \"a;b\" BEGIN
                     UPDATE \"a;b\" SET x = CASE WHEN new.x > 0 THEN 1 ELSE 0 END;
                 END;",
            )],
        )
        .await
        .expect("the migration was cut");

        let tables: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('a;b', 't')")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tables, 2);
    }

    /// The query plans of `statement`, one line each.
    async fn plan(pool: &SqlitePool, statement: &str) -> String {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {statement}")))
                .fetch_all(pool)
                .await
                .unwrap();
        rows.into_iter().map(|(_, _, _, detail)| detail).collect::<Vec<_>>().join("\n")
    }

    /// A title's resolutions are read by its key, for every explanation and webhook, and pruned
    /// by key every hour: neither scans a table holding a row per title and per source.
    #[tokio::test]
    async fn the_identifier_lookups_seek_an_index() {
        let pool = test_pool().await;
        for statement in [
            "SELECT source, media_type, local_key, external_id FROM source_identifiers
              WHERE media_type = 'movie' AND local_key IN ('tmdb:1', 'tmdb:2')",
            "DELETE FROM source_identifiers WHERE local_key IN ('tmdb:1', 'tmdb:2')",
        ] {
            let plan = plan(&pool, statement).await;
            assert!(!plan.contains("SCAN source_identifiers"), "{statement}\n{plan}");
        }
    }

    /// A title's history and an instance's are read from the move log by the title or the
    /// instance, newest first.
    #[tokio::test]
    async fn the_log_filters_seek_an_index() {
        let pool = test_pool().await;
        for column in ["media_id", "instance_id"] {
            for statement in [
                format!(
                    "SELECT id FROM execution_logs WHERE {column} = 'x'
                      ORDER BY executed_at DESC, id DESC LIMIT 50"
                ),
                format!("SELECT COUNT(*) FROM execution_logs WHERE {column} = 'x'"),
            ] {
                let plan = plan(&pool, &statement).await;
                assert!(plan.contains(&format!("({column}=?)")), "{statement}\n{plan}");
            }
        }
    }

    /// An index whose columns start another's is kept up on every write for nothing: the longer
    /// one answers its lookups.
    #[tokio::test]
    async fn no_index_repeats_the_start_of_another() {
        let pool = test_pool().await;
        let indexes: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT m.name, l.name, l.\"unique\" FROM sqlite_master m
               JOIN pragma_index_list(m.name) l
              WHERE m.type = 'table' AND l.partial = 0",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let mut columns = Vec::new();
        for (table, index, unique) in indexes {
            let keyed: Vec<(String, String)> = sqlx::query_as(
                "SELECT COALESCE(name, ''), coll FROM pragma_index_xinfo(?) WHERE key = 1
                  ORDER BY seqno",
            )
            .bind(&index)
            .fetch_all(&pool)
            .await
            .unwrap();
            columns.push((table, index, unique, keyed));
        }
        let mut repeated = Vec::new();
        for (table, index, unique, keyed) in &columns {
            for (other_table, other, _, longer) in &columns {
                let starts = longer.len() >= keyed.len() && longer[..keyed.len()] == keyed[..];
                if table == other_table && index != other && starts && !unique {
                    repeated.push(format!("{index} starts {other}"));
                }
            }
        }
        assert_eq!(repeated, Vec::<String>::new());
    }

    /// A database opened where the path says, whatever it holds: a URL would read `%41` as `A`
    /// and stop at a `?`.
    #[tokio::test]
    async fn the_database_opens_at_the_path_as_written() {
        let dir = crate::tests::TempDir::new("db-path");
        let mut config = Config::for_tests();
        config.set_db_path(dir.join("a%41b").join("routarr.db"));

        let pool = init_pool(&config).await.expect("the database opened");
        pool.close().await;

        assert!(dir.join("a%41b").join("routarr.db").exists(), "not created where named");
        assert!(!dir.join("aAb").exists(), "the path was read as a URL");
    }

    /// The write-ahead log takes the size of the largest transaction, a sync of the whole
    /// library, and keeps it unless a limit trims it.
    #[tokio::test]
    async fn the_write_ahead_log_is_kept_small() {
        let dir = crate::tests::TempDir::new("wal-limit");
        let mut config = Config::for_tests();
        config.set_db_path(dir.join("routarr.db"));

        let pool = init_pool(&config).await.unwrap();
        let limit: i64 =
            sqlx::query_scalar("PRAGMA journal_size_limit").fetch_one(&pool).await.unwrap();
        pool.close().await;

        assert_eq!(limit, 64 * 1024 * 1024);
    }

    /// The schema refuses what the code never writes: a state nothing sets, a folder of no known
    /// origin, a flag neither 0 nor 1, a title's countries NULL where every reader expects a list.
    #[tokio::test]
    async fn the_schema_refuses_values_the_code_never_writes() {
        let pool = library_with_an_override().await;
        for statement in [
            "INSERT INTO jobs (id, kind, status) VALUES ('j-1', 'sync', 'queued')",
            "INSERT INTO root_folders (id, instance_id, path, origin)
             VALUES ('rf-1', 'i-1', '/movies', 'somewhere')",
            "INSERT INTO root_folders (id, instance_id, path, accessible)
             VALUES ('rf-2', 'i-1', '/movies', 2)",
            "INSERT INTO metadata_cache (source, external_id, media_type, origin_countries)
             VALUES ('tmdb', '1', 'movie', NULL)",
            "INSERT INTO metadata_cache (source, external_id, media_type, stale)
             VALUES ('tmdb', '2', 'movie', 2)",
            "UPDATE instances SET enabled = 2",
            "UPDATE media SET monitored = 2",
            "UPDATE media SET has_files = -1",
        ] {
            let refused = sqlx::query(statement).execute(&pool).await;
            assert!(refused.is_err(), "accepted: {statement}");
        }
    }

    /// The tables the schema rebuilds keep every row, and the rows that point at them, through
    /// an upgrade.
    #[tokio::test]
    async fn an_upgrade_that_tightens_the_schema_keeps_every_row() {
        let pool = crate::tests::database_of_0_1_5().await;
        for statement in [
            "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
             VALUES ('i-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, monitored, has_files)
             VALUES ('m-1', 'i-1', 10, 'movie', 'Totoro', 1, 0)",
            "INSERT INTO overrides (id, media_id, target_category) VALUES ('o-1', 'm-1', 'anime')",
            "INSERT INTO root_folders (id, instance_id, path, origin)
             VALUES ('rf-1', 'i-1', '/movies', 'declared')",
            "INSERT INTO metadata_cache (source, external_id, media_type, origin_countries,
                                         expires_at)
             VALUES ('tmdb', '1', 'movie', NULL, '2099-01-01')",
            "INSERT INTO jobs (id, kind, status) VALUES ('j-1', 'sync', 'success')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }

        run_migrations(&pool).await.unwrap();

        let kept: (i64, i64, i64, i64, i64, String) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM instances), (SELECT COUNT(*) FROM media),
                    (SELECT COUNT(*) FROM overrides), (SELECT COUNT(*) FROM root_folders),
                    (SELECT COUNT(*) FROM jobs),
                    (SELECT origin_countries FROM metadata_cache)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept, (1, 1, 1, 1, 1, "[]".to_string()));
    }

    #[test]
    fn placeholders_are_one_question_mark_per_value() {
        assert_eq!(placeholders(0), "");
        assert_eq!(placeholders(1), "?");
        assert_eq!(placeholders(3), "?, ?, ?");
    }

    /// A stopped Routarr leaves a database file complete without its `-wal`, checked
    /// against a real file instead of the in-memory pool.
    #[tokio::test]
    async fn a_closed_database_needs_no_sidecar_files_to_be_complete() {
        let dir = crate::tests::TempDir::new("wal");
        let path = dir.join("r.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options.clone())
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::query("INSERT INTO categories (id, name) VALUES ('c-1', 'survives')")
            .execute(&pool)
            .await
            .unwrap();
        // A second connection, as an operator's `sqlite3`, so only the checkpoint folds the log.
        let bystander =
            SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        sqlx::query("SELECT 1").execute(&bystander).await.unwrap();

        // The write is in the WAL at this point, not in the database file.
        assert!(path.with_extension("db-wal").exists(), "expected a WAL to exist while running");

        checkpoint_and_close(&pool).await;

        // Truncated to zero, the WAL contributes exactly as much as a removed one.
        let wal = path.with_extension("db-wal");
        let leftover = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(leftover, 0, "the WAL still holds {leftover} bytes after closing");

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
        bystander.close().await;
    }

    /// A migration file is applied only once `MIGRATIONS` lists it: one written and left out
    /// reaches no database, with every test green.
    #[test]
    fn every_migration_file_is_listed_in_order() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".sql").map(str::to_string)
            })
            .collect();
        files.sort();
        let listed: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
        assert_eq!(listed, files);
    }

    /// The process folds the log back once the server has stopped serving, read out of its
    /// source since no test runs it: a shutdown that drops the pool loses the last writes.
    #[test]
    fn the_server_checkpoints_the_database_once_it_stops_serving() {
        const MAIN: &str = include_str!("process.rs");
        let after_serving =
            MAIN.split_once("listener::serve(").expect("main serves until a stop is asked").1;
        let shutdown =
            after_serving.split_once("Routarr stopped cleanly").expect("main says it stopped").0;
        let (drained, closed) = shutdown
            .split_once("db::checkpoint_and_close(&pool)")
            .unwrap_or_else(|| panic!("main stops without folding the log back:\n{shutdown}"));
        assert!(
            drained.contains("jobs.drain(") && !closed.contains("jobs.drain("),
            "main closes the database before the moves in flight are recorded:\n{shutdown}"
        );
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
