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
    ("009_application_keys", include_str!("../migrations/009_application_keys.sql")),
    ("010_job_result", include_str!("../migrations/010_job_result.sql")),
    ("011_webhook_secrets", include_str!("../migrations/011_webhook_secrets.sql")),
    ("012_hashed_sessions", include_str!("../migrations/012_hashed_sessions.sql")),
    ("013_opened_by", include_str!("../migrations/013_opened_by.sql")),
    ("014_routing_generation", include_str!("../migrations/014_routing_generation.sql")),
    ("015_certification_scale", include_str!("../migrations/015_certification_scale.sql")),
    ("016_requested_moves", include_str!("../migrations/016_requested_moves.sql")),
    ("017_drop_refresh_after_move", include_str!("../migrations/017_drop_refresh_after_move.sql")),
    ("018_drop_routing_generation", include_str!("../migrations/018_drop_routing_generation.sql")),
    ("019_instance_reads", include_str!("../migrations/019_instance_reads.sql")),
    ("020_media_routing", include_str!("../migrations/020_media_routing.sql")),
    ("021_added_within_one_day", include_str!("../migrations/021_added_within_one_day.sql")),
    ("022_indexes_that_serve", include_str!("../migrations/022_indexes_that_serve.sql")),
    (
        "023_constraints_the_code_keeps",
        include_str!("../migrations/023_constraints_the_code_keeps.sql"),
    ),
    ("024_opened_by_schema", include_str!("../migrations/024_opened_by_schema.sql")),
    (
        "025_added_dates_stored_shape",
        include_str!("../migrations/025_added_dates_stored_shape.sql"),
    ),
    ("026_unique_instance_names", include_str!("../migrations/026_unique_instance_names.sql")),
    (
        "027_stateless_sign_in_attempts",
        include_str!("../migrations/027_stateless_sign_in_attempts.sql"),
    ),
    ("028_secret_salt", include_str!("../migrations/028_secret_salt.sql")),
    ("029_subject_keys", include_str!("../migrations/029_subject_keys.sql")),
    ("030_security_events", include_str!("../migrations/030_security_events.sql")),
    ("031_zero_ids", include_str!("../migrations/031_zero_ids.sql")),
    ("032_every_regions_rating", include_str!("../migrations/032_every_regions_rating.sql")),
    ("033_daily_quotas", include_str!("../migrations/033_daily_quotas.sql")),
    ("034_full_vocabularies", include_str!("../migrations/034_full_vocabularies.sql")),
    ("035_status_words", include_str!("../migrations/035_status_words.sql")),
    ("036_rating_country_changes", include_str!("../migrations/036_rating_country_changes.sql")),
];

/// How large the write-ahead log stays once checkpointed, in bytes.
const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// Initialize the SQLite connection pool and run migrations.
pub async fn init_pool(config: &Config) -> crate::error::AppResult<SqlitePool> {
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

    // A path, never a URL: read as one, `%41` would be `A` and a `?` would end
    // the file name.
    let file = if config.db_path == *":memory:" {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else {
        SqliteConnectOptions::new().filename(&config.db_path)
    };
    // PRAGMAs belong on the connect options: setting them with a one-off query
    // only configures whichever pooled connection happened to serve it.
    let options = with_paths(file)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // NORMAL is the documented companion of WAL: durable across app crashes,
        // and an order of magnitude faster than FULL for the write bursts a
        // simulation produces. A power cut can roll back the last commits, so
        // the record of a move the Arr made is written under FULL
        // (`executor::record::record_outcome`).
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .foreign_keys(true)
        // Without this, concurrent writers surface as "database is locked"
        // instead of waiting their turn.
        .busy_timeout(Duration::from_secs(10))
        // The log grows to the largest transaction, a sync of the whole
        // library, and is trimmed back to this once checkpointed.
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

    // Refused with the way back: the restore that needs no server, of the
    // newest archive this build can open.
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
            // Not a reason to stay on the old schema: the archives taken
            // before this start are still there.
            Err(e) => tracing::error!("No backup could be taken before migrating: {e}"),
        }
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

/// Whether running the migrations would change a schema some release already
/// built: the database holds migrations, and this build lists one it lacks.
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

/// Fold the write-ahead log back into the database file, then close the pool.
///
/// In WAL mode a commit lands in `routarr.db-wal`, and SQLite only checkpoints
/// when the *last* connection closes, which dropping the pool does not do.
/// After a clean stop `routarr.db` is complete on its own, so a copy of it
/// taken without its `-wal` loses nothing.
///
/// A failure is logged rather than propagated: the data is still in the WAL and
/// the next start recovers it.
pub async fn checkpoint_and_close(pool: &SqlitePool) {
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(pool).await {
        tracing::warn!("Could not checkpoint the write-ahead log: {e}");
    }
    pool.close().await;
}

/// Apply any migration not yet recorded in `_migrations`, after refusing a
/// database a newer release migrated.
pub async fn run_migrations(pool: &SqlitePool) -> crate::error::AppResult<()> {
    refuse_a_newer_schema(pool).await?;
    apply_migrations(pool, MIGRATIONS).await?;
    Ok(record_this_release(pool).await?)
}

/// Refuse a database a newer release migrated further than this build knows:
/// run on, this one would read and write a schema it does not know.
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

/// The newer release that migrated this database further than this build
/// knows, if one did. Every release records itself and the last migration it
/// had applied, and one newer than this build that recorded a migration this
/// build does not list, or recorded none, is ahead of it. Migration names
/// alone cannot tell: a database a newer release never opened passes whatever
/// it holds.
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

/// Record this release, and the last migration it applied, as having opened
/// the database.
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

/// A version as numbers to compare, `0.1.10` after `0.1.9`. A part that is
/// not a number counts as 0.
fn release(version: &str) -> Vec<u64> {
    version.split(['.', '-']).take(3).map(|part| part.parse().unwrap_or(0)).collect()
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

        // A table rebuilt through a copy drops the original, and with foreign
        // keys on that deletes or refuses every row pointing at it. SQLite
        // takes the switch only outside a transaction, so it is turned off on
        // the connection that migrates, and `foreign_key_check` stands in for
        // it before the commit.
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

/// One migration and its record, in one transaction: a failure half-way
/// leaves the schema untouched instead of half-applied and unrecorded.
async fn apply_migration(
    connection: &mut sqlx::SqliteConnection,
    name: &str,
    sql: &str,
) -> Result<(), sqlx::Error> {
    use sqlx::Connection;
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
    // SQLite's own parser walks the script, statement after statement, so a
    // comment, a quoted name or a trigger body is read as SQLite reads it.
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

/// A transaction that takes the write lock as it starts, for a check and the
/// write it guards. Under a plain `BEGIN` the lock comes with the first write,
/// and another writer can change what was checked in between: a category
/// removed after a rule naming it was judged valid, say.
pub async fn write_transaction(
    pool: &SqlitePool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// `options` with what every query of the application may name: the `path`
/// collation, which compares two folders as [`crate::paths::key`] does. A
/// connection opened without it fails every statement that names it.
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

    /// A newer release that migrated no further than this build leaves a
    /// database this build reads as its own: going back a patch release is
    /// no reason to stop.
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

    /// A database a newer release migrated further is refused, naming that
    /// release, rather than read and written as if its schema were this
    /// build's. So is one a newer release opened without saying how far it
    /// had migrated. A name no release lists, with no newer opener, is no
    /// reason to refuse.
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

    /// The SQLite the binary carries, at least the release that fixes
    /// CVE-2025-6965, CVE-2025-29087 and CVE-2025-3277. `libsqlite3-sys`
    /// bundles it, and RustSec does not flag a bundled copy, so nothing else
    /// would notice the lock falling behind.
    #[tokio::test]
    async fn the_bundled_sqlite_has_its_known_flaws_fixed() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let version: String =
            sqlx::query_scalar("SELECT sqlite_version()").fetch_one(&pool).await.unwrap();
        let parts: Vec<u32> = version.split('.').map(|part| part.parse().unwrap()).collect();
        assert!(parts >= vec![3, 50, 2], "SQLite {version} predates 3.50.2");
    }

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

    /// SQLite changes a CHECK or a column type only by rebuilding the table,
    /// and dropping the original under foreign keys deletes what points at it.
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

    /// With foreign keys off while it runs, a migration is held to them
    /// before it commits.
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

    /// A migration is read as SQLite reads it: a block comment and a quoted
    /// name may hold a `;` or a `'` without cutting the statement.
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

    /// A title's resolutions are read by its key, for every explanation and
    /// every webhook, and pruned by key every hour: neither reads the whole
    /// table, which holds a row per title and per source found by search.
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

    /// A title's history and an instance's are read from the move log by the
    /// title or the instance, newest first.
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

    /// An index whose columns start another's is kept up on every write for
    /// nothing: the longer one answers its lookups.
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

    /// A database opened where the path says, whatever it holds: a URL would
    /// read `%41` as `A` and stop at a `?`.
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

    /// The write-ahead log takes the size of the largest transaction, a sync
    /// of the whole library, and keeps it unless a limit trims it.
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

    /// The schema refuses what the code never writes: a state nothing sets, a
    /// folder of no known origin, a flag that is neither 0 nor 1, a title's
    /// countries left NULL where every reader expects a list.
    #[tokio::test]
    async fn the_schema_refuses_values_the_code_never_writes() {
        let pool = library_with_an_override().await;
        for statement in [
            "INSERT INTO jobs (id, kind, status) VALUES ('j-1', 'sync', 'queued')",
            "INSERT INTO root_folders (id, instance_id, path, origin)
             VALUES ('rf-1', 'i-1', '/movies', 'somewhere')",
            "INSERT INTO root_folders (id, instance_id, path, accessible)
             VALUES ('rf-2', 'i-1', '/movies', 2)",
            "INSERT INTO metadata_cache (source, external_id, media_type, origin_countries,
                                         expires_at)
             VALUES ('tmdb', '1', 'movie', NULL, '2099-01-01')",
            "UPDATE instances SET enabled = 2",
            "UPDATE media SET monitored = 2",
            "UPDATE media SET has_files = -1",
        ] {
            let refused = sqlx::query(statement).execute(&pool).await;
            assert!(refused.is_err(), "accepted: {statement}");
        }
    }

    /// The tables the schema rebuilds keep every row, and the rows that point
    /// at them, through an upgrade.
    #[tokio::test]
    async fn an_upgrade_that_tightens_the_schema_keeps_every_row() {
        let pool = crate::tests::database_through("022_indexes_that_serve").await;
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

    /// A stopped Routarr must leave a database file that is complete on its own:
    /// an unfolded `routarr.db-wal` holds the latest writes, and a copy of the
    /// `.db` alone would lack them. Checked against a real file rather than the
    /// in-memory pool the rest of the suite uses.
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
        // Another connection to the file, as an operator's `sqlite3` shell holds
        // one. SQLite folds the log back when the *last* connection closes, so
        // with this one open, closing the pool alone leaves everything in the
        // WAL, and only the checkpoint moves it into the file.
        let bystander =
            SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        sqlx::query("SELECT 1").execute(&bystander).await.unwrap();

        // The write is in the WAL at this point, not in the database file.
        assert!(path.with_extension("db-wal").exists(), "expected a WAL to exist while running");

        checkpoint_and_close(&pool).await;

        // The WAL must no longer carry anything: truncated to zero, it
        // contributes exactly as much as a removed one.
        let wal = path.with_extension("db-wal");
        let leftover = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(leftover, 0, "the WAL still holds {leftover} bytes after closing");

        // Copy only the `.db`.
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

    /// `main` folds the log back once the server has stopped serving. Read out
    /// of its source, since no test runs `main`: a shutdown that drops the pool
    /// instead leaves the `.db` without the last writes.
    #[test]
    fn the_server_checkpoints_the_database_once_it_stops_serving() {
        const MAIN: &str = include_str!("main.rs");
        let after_serving = MAIN
            .split_once("listener::serve(socket, app, shutdown_signal()")
            .expect("main serves until a shutdown signal")
            .1;
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
