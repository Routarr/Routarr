//! The simulation must not go to the database once per media item.
//!
//! Every other test seeds one or two media items, so a regression to a query
//! per item passes the whole suite and surfaces only on a large library.
//!
//! The query assertions are invariances, not stopwatches: the number of
//! statements a simulation runs must not change when the library grows
//! ten-fold. The timing assertions compare one run with another ten times its
//! size, so they measure how the work grows rather than how fast the runner is.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use tracing::Instrument;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::layer::SubscriberExt;

use crate::services::maintenance;
use crate::services::routing::{self, SimulationOptions};
use crate::tests::TestApp;

/// Counts the statements a future sends to SQLite.
///
/// sqlx runs every statement on the connection's worker thread, inside the
/// span that was current where the statement was issued. Run inside a span
/// this subscriber created, each statement enters that span once on the
/// worker, whether it went through the pool, over a connection held across a
/// loop, or inside a transaction. The caller's own thread enters it on every
/// poll, and is not counted. The subscriber is the future's own, so tests
/// running in parallel cannot pollute each other's reading.
#[derive(Clone)]
struct Statements {
    entered: Arc<AtomicUsize>,
    caller: std::thread::ThreadId,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Statements {
    fn on_enter(&self, _: &tracing::span::Id, _: tracing_subscriber::layer::Context<'_, S>) {
        if std::thread::current().id() != self.caller {
            self.entered.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Run `work` and report how many statements it sent to the database.
async fn statements_of<T>(work: impl Future<Output = T>) -> (T, usize) {
    let counter = Statements { entered: Arc::default(), caller: std::thread::current().id() };
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(counter.clone()));
    let output = async { work.instrument(tracing::info_span!("measured")).await }
        .with_subscriber(dispatch)
        .await;
    (output, counter.entered.load(Ordering::Relaxed))
}

/// An in-memory library on one connection.
async fn library_pool() -> SqlitePool {
    library_on(SqliteConnectOptions::new()).await
}

/// An in-memory library on one connection opened with `options`.
async fn library_on(options: SqliteConnectOptions) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .connect_with(crate::db::with_paths(options.in_memory(true).foreign_keys(true)))
        .await
        .expect("in-memory sqlite");
    crate::db::run_migrations(&pool).await.expect("migrations");
    pool
}

/// A clock for the work one library does: the CPU time of the test's thread,
/// of the library's worker thread, where its statements run, and of the
/// threads a pass runs on, when it is told their name.
///
/// Waiting on a library-pass permit, on a lock or on a core another test holds
/// is not in it, so the reading follows the work rather than how busy the
/// runner is. Where the kernel publishes no per-thread figure, the wall clock
/// stands in.
struct WorkClock {
    worker: Option<std::path::PathBuf>,
    passes: Option<String>,
}

impl WorkClock {
    /// A library on its own named worker, and the clock that reads it.
    async fn with_library() -> (SqlitePool, WorkClock) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // Linux keeps fifteen bytes of a thread name, and this stays under.
        let name = format!("scale-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let named = name.clone();
        let pool =
            library_on(SqliteConnectOptions::new().thread_name(move |_| named.clone())).await;
        let worker = std::fs::read_dir("/proc/self/task").ok().and_then(|tasks| {
            tasks.flatten().map(|task| task.path()).find(|task| {
                std::fs::read_to_string(task.join("comm")).is_ok_and(|comm| comm.trim() == name)
            })
        });
        (pool, WorkClock { worker: worker.map(|task| task.join("schedstat")), passes: None })
    }

    /// Count the threads named `name` too, where a runtime built with that
    /// name runs its passes.
    fn counting_passes(self, name: String) -> Self {
        WorkClock { passes: Some(name), ..self }
    }

    fn now(&self) -> Duration {
        let on_cpu = |path: &std::path::Path| -> Option<u64> {
            std::fs::read_to_string(path).ok()?.split_whitespace().next()?.parse().ok()
        };
        let passes = |name: &str| -> u64 {
            let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return 0 };
            tasks
                .flatten()
                .map(|task| task.path())
                .filter(|task| {
                    std::fs::read_to_string(task.join("comm")).is_ok_and(|comm| comm.trim() == name)
                })
                .filter_map(|task| on_cpu(&task.join("schedstat")))
                .sum()
        };
        let threads = self.worker.as_deref().and_then(|worker| {
            Some(
                on_cpu(worker)?
                    + on_cpu(std::path::Path::new("/proc/thread-self/schedstat"))?
                    + self.passes.as_deref().map_or(0, passes),
            )
        });
        match threads {
            Some(nanoseconds) => Duration::from_nanos(nanoseconds),
            None => wall_clock(),
        }
    }

    /// The least of three runs of `work`.
    async fn least<F: Future>(&self, mut work: impl FnMut() -> F) -> Duration {
        let mut least = Duration::MAX;
        for _ in 0..3 {
            let started = self.now();
            work().await;
            least = least.min(self.now() - started);
        }
        least
    }
}

/// Time since the first reading, for a kernel with no per-thread figure.
fn wall_clock() -> Duration {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// A library of `count` films across one instance, with a rule that matches
/// roughly half of them and a mapped root folder for each category.
async fn seed(pool: &SqlitePool, count: usize) {
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', 'k', 1)",
    )
    .execute(pool)
    .await
    .unwrap();

    // The initial migration seeds `standard`, and only `anime` is new.
    for (id, name) in [("cat-anime", "anime"), ("cat-standard", "standard")] {
        sqlx::query("INSERT OR IGNORE INTO categories (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }

    for (id, arr_id, path, category) in
        [("rf-1", 1, "/movies/standard", "standard"), ("rf-2", 2, "/movies/anime", "anime")]
    {
        sqlx::query(
            "INSERT INTO root_folders (id, instance_id, arr_id, path, accessible, category)
             VALUES (?, 'inst-1', ?, ?, 1, ?)",
        )
        .bind(id)
        .bind(arr_id)
        .bind(path)
        .bind(category)
        .execute(pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('default_category', 'standard')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO rules (id, name, priority, enabled, media_type, conditions,
         target_category, match_mode)
         VALUES ('r-1', 'Anime', 10, 1, 'both',
                 '[{\"type\":\"genre_contains\",\"value\":[\"Animation\"]}]', 'anime', 'all')",
    )
    .execute(pool)
    .await
    .unwrap();

    // One transaction for the whole library: seeding is not what is being
    // measured, and ten thousand separate inserts would dominate the run.
    let mut tx = pool.begin().await.unwrap();
    for i in 0..count {
        let anime = i % 2 == 0;
        sqlx::query(
            "INSERT INTO media (id, instance_id, arr_id, media_type, title, year, tmdb_id,
             current_root_folder, monitored, has_files, genres)
             VALUES (?, 'inst-1', ?, 'movie', ?, 1988, ?, '/movies/standard', 1, 1, ?)",
        )
        .bind(format!("m-{i}"))
        .bind(i as i64)
        .bind(format!("Film {i}"))
        .bind(i as i64)
        .bind(if anime { r#"["Animation"]"# } else { r#"["Drama"]"# })
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
}

/// A library of `count` items described by the Arr and by two fetched
/// sources, as most are once enrichment has run.
async fn described_library(count: usize) -> SqlitePool {
    let pool = library_pool().await;
    seed(&pool, count).await;
    seed_cache(&pool, count).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('metadata_providers', 'arr,tmdb,omdb')")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

/// A simulation of `count` items, the statements it ran, and how many
/// decisions it stored.
async fn statements_for(count: usize, persist: bool) -> (usize, i64) {
    let pool = described_library(count).await;

    let (result, statements) = statements_of(routing::run_simulation(
        &pool,
        SimulationOptions { persist, ..Default::default() },
    ))
    .await;
    assert_eq!(result.unwrap().total_media, count, "the whole library was not evaluated");
    let stored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decisions").fetch_one(&pool).await.unwrap();
    pool.close().await;
    (statements, stored)
}

/// Reading and evaluating the library costs the same handful of statements
/// whatever its size, and storing the run adds one row per decision it keeps
/// and nothing else per item.
#[tokio::test]
async fn a_simulation_does_not_query_once_per_media_item() {
    let (small, _) = statements_for(200, false).await;
    let (large, _) = statements_for(2000, false).await;
    println!("evaluated: 200 items in {small} statements, 2000 in {large}");
    // A per-item query would put roughly 1 800 more on the second reading.
    assert!(
        large <= small + 5,
        "the library grew ten-fold and the database was reached {large} times instead of {small}: \
         the simulation is querying per item again"
    );

    let (small, small_rows) = statements_for(200, true).await;
    let (large, large_rows) = statements_for(2000, true).await;
    println!("stored: {small_rows} rows in {small} statements, {large_rows} in {large}");
    assert!(small_rows > 0, "the fixture stores nothing, so this proves nothing");
    // Supersession binds its ids in chunks, a statement per chunk: the few
    // statements the tolerance leaves are those.
    let (beyond_small, beyond_large) = (small - small_rows as usize, large - large_rows as usize);
    assert!(
        beyond_large <= beyond_small + 5,
        "beyond one per stored row, {beyond_large} statements against {beyond_small}: \
         storing the run queries per item"
    );
}

/// An apply revalidates every move against what the rules decide now, for a
/// slice of up to a thousand: the same handful of statements whatever the
/// slice, the ids bound in chunks aside.
#[tokio::test]
async fn revalidating_moves_does_not_query_once_per_item() {
    let mut counts = Vec::new();
    for count in [200, 2000] {
        let pool = described_library(count).await;
        let ids: Vec<String> = (0..count).map(|i| format!("m-{i}")).collect();
        let (targets, statements) = statements_of(routing::revalidated_targets(&pool, &ids)).await;
        assert_eq!(targets.unwrap().len(), count, "not every item was revalidated");
        counts.push(statements);
        pool.close().await;
    }
    let (small, large) = (counts[0], counts[1]);
    println!("revalidated: 200 items in {small} statements, 2000 in {large}");
    // The ids are bound a chunk of `BIND_CHUNK` at a time, into the titles,
    // their pins, and per media type their identifiers and each source's
    // answers: a statement per chunk for each is the growth allowed, a
    // statement per item is not.
    let chunk = routing::BIND_CHUNK;
    let more_chunks = 2000_usize.div_ceil(chunk) - 200_usize.div_ceil(chunk);
    let per_chunk = 2 + 2 * (1 + crate::services::metadata::PROVIDERS.len());
    assert!(
        large <= small + per_chunk * more_chunks,
        "{large} statements for 2000 items against {small} for 200"
    );
}

/// A preview evaluates two rule sets over one library. Loaded once, an
/// evaluation reaches the database for nothing: every table it reads is in
/// memory, so the second rule set costs no query and a plain run is one load
/// and nothing more.
#[tokio::test]
async fn an_evaluation_over_a_loaded_library_costs_no_query() {
    let pool = library_pool().await;
    seed(&pool, 200).await;

    let (library, loading) =
        statements_of(routing::load_library(&pool, &SimulationOptions::default())).await;
    let library = library.unwrap();
    assert!(loading > 0, "loading reads the library");

    let ((without_rules, with_rules), evaluating) = statements_of(async {
        let without = routing::simulate_loaded(
            &pool,
            &library,
            SimulationOptions { rules_override: Some(Vec::new()), ..Default::default() },
        )
        .await
        .unwrap();
        let with =
            routing::simulate_loaded(&pool, &library, SimulationOptions::default()).await.unwrap();
        (without, with)
    })
    .await;
    assert_eq!(evaluating, 0, "an evaluation reached the database");
    assert_eq!(without_rules.total_media, 200);
    assert_ne!(
        without_rules.moves_required, with_rules.moves_required,
        "the two rule sets must have been evaluated apart"
    );
    drop(library);

    let (_, running) =
        statements_of(routing::run_simulation(&pool, SimulationOptions::default())).await;
    assert_eq!(running, loading, "a run is one load and nothing more");
}

/// Titles that are gone, as a webhook for a title just deleted names, are
/// read and nothing more: no library pass waited for, no context loaded.
#[tokio::test]
async fn a_pass_over_titles_that_are_gone_reads_nothing_else() {
    let pool = library_pool().await;
    seed(&pool, 200).await;

    let options = SimulationOptions {
        media_ids: Some(vec!["gone".into()]),
        persist: true,
        ..Default::default()
    };
    let (result, statements) = statements_of(routing::run_simulation(&pool, options)).await;

    assert_eq!(result.unwrap().total_media, 0);
    // The one query in its read transaction, and the check the pool makes of
    // its connection before lending it. A whole-library load is a dozen more.
    assert!(statements <= 4, "a pass over no title read the library: {statements} statements");
}

/// A pass computes for seconds over a large library with nothing to wait on.
/// On the runtime's own thread it would hold it, and the status polling, the
/// webhooks and the health checks would wait for its end.
#[tokio::test]
async fn a_pass_over_the_library_leaves_the_runtime_free() {
    let pool = library_pool().await;
    seed(&pool, 5000).await;
    let library = routing::load_library(&pool, &SimulationOptions::default()).await.unwrap();
    let longest_wait = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticking = {
        let longest_wait = Arc::clone(&longest_wait);
        tokio::spawn(async move {
            let mut last = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let waited = last.elapsed().as_nanos() as u64;
                longest_wait.fetch_max(waited, Ordering::Relaxed);
                last = Instant::now();
            }
        })
    };
    // Ticking before the pass starts, or a pass that never lets it run goes
    // unseen.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let started = Instant::now();
    routing::simulate_loaded(&pool, &library, SimulationOptions::default()).await.unwrap();
    let pass = started.elapsed();
    ticking.abort();

    let longest = Duration::from_nanos(longest_wait.load(Ordering::Relaxed));
    println!("a pass of {pass:?}, the runtime held at most {longest:?}");
    assert!(longest * 4 < pass, "the pass held the runtime {longest:?} out of {pass:?}");
}

/// The hourly prune of the search resolutions no title holds any more removes
/// them a chunk at a time, not a statement per key: after an instance of
/// thousands of titles is deleted, a statement each held the write lock for
/// minutes.
#[tokio::test]
async fn pruning_stale_resolutions_does_not_query_once_per_key() {
    let mut counts = Vec::new();
    for count in [200, 2000] {
        let app = TestApp::around(crate::state::AppState::for_tests_on(library_pool().await));
        let mut tx = app.state.pool.begin().await.unwrap();
        for i in 0..count {
            sqlx::query(
                "INSERT INTO source_identifiers (source, media_type, local_key, external_id)
                 VALUES ('anilist', 'movie', ?, ?)",
            )
            .bind(format!("tmdb:{i}"))
            .bind(i.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();

        let by = crate::jobs::Attribution::manual(None);
        let (report, statements) = statements_of(maintenance::run(&app.state, &by)).await;
        assert_eq!(report.unwrap().source_identifiers_removed, count as u64);
        counts.push(statements);
    }
    let (small, large) = (counts[0], counts[1]);
    println!("pruned: 200 keys in {small} statements, 2000 in {large}");
    let more_chunks =
        2000_usize.div_ceil(routing::BIND_CHUNK) - 200_usize.div_ceil(routing::BIND_CHUNK);
    assert!(large <= small + 2 * more_chunks, "{large} statements for 2000 keys against {small}");
}

/// The ceiling on how much more work ten times the library may cost. Linear
/// work costs about ten times as much, less since a run has fixed costs, and
/// a rescan of the library per item about a hundred times.
const TEN_FOLD_CEILING: u32 = 15;

/// The work a stored simulation of `count` items costs, at its least.
///
/// On a runtime of its own, whose threads carry a name the clock finds: the
/// evaluation runs on one of them, off the test's thread.
fn simulation_work(count: usize) -> Duration {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let passes = format!("pass-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name(&passes)
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let (pool, clock) = WorkClock::with_library().await;
        let clock = clock.counting_passes(passes);
        seed(&pool, count).await;
        let options = || SimulationOptions { persist: true, ..Default::default() };
        let work =
            clock.least(|| async { routing::run_simulation(&pool, options()).await.unwrap() });
        let work = work.await;
        pool.close().await;
        work
    })
}

/// Quadratic work that is not in the queries (a merge that rescans, an O(n²)
/// lookup), which the statement count above cannot see.
#[test]
fn a_simulation_grows_linearly_with_the_library() {
    let small = simulation_work(500);
    let large = simulation_work(5000);
    println!("500 items simulated in {small:?}, 5000 in {large:?}");

    assert!(
        large < small * TEN_FOLD_CEILING,
        "ten times the library cost {large:?} against {small:?}: the work is not linear any more"
    );
}

/// A cache row per source for every item, keyed the way each source keys.
async fn seed_cache(pool: &SqlitePool, count: usize) {
    sqlx::query("UPDATE media SET imdb_id = 'tt' || printf('%07d', arr_id)")
        .execute(pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    for i in 0..count {
        for (source, external_id) in [("tmdb", i.to_string()), ("omdb", format!("tt{i:07}"))] {
            sqlx::query(
                "INSERT INTO metadata_cache (source, external_id, media_type, genres)
                 VALUES (?, ?, 'movie', '[\"Action\"]')",
            )
            .bind(source)
            .bind(external_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit().await.unwrap();
}

/// The work the facets and the orphan sweep cost over `count` items and a
/// cache row per source for each, at their least.
async fn facets_and_sweep_work(count: usize) -> (Duration, Duration) {
    let (pool, clock) = WorkClock::with_library().await;
    let app = TestApp::around(crate::state::AppState::for_tests_on(pool));
    seed(&app.state.pool, count).await;
    seed_cache(&app.state.pool, count).await;
    app.list_tmdb().await;

    let facets = clock
        .least(|| async {
            let response = app.get("/api/v1/media/facets").await;
            assert_eq!(response.assert_ok()["total_media"], count as i64);
        })
        .await;
    let sweep = clock
        .least(|| async {
            let report = maintenance::run(&app.state, &crate::jobs::Attribution::manual(None))
                .await
                .unwrap();
            assert_eq!(report.metadata_cache_removed, 0, "every cache row belongs to a media item");
        })
        .await;
    (facets, sweep)
}

/// The two queries that join `media` to `metadata_cache` across every identifier
/// namespace. Written as one OR over three cast columns they cannot use an
/// index, and on a large library the facets take minutes and the hourly purge
/// holds a connection for as long. A join that stopped using its index scans
/// the cache once per item, and ten times the library takes a hundred times
/// as long.
#[tokio::test]
async fn facets_and_the_orphan_sweep_stay_indexed_at_scale() {
    let (small_facets, small_sweep) = facets_and_sweep_work(500).await;
    let (large_facets, large_sweep) = facets_and_sweep_work(5000).await;
    println!("facets: {small_facets:?} for 500 items, {large_facets:?} for 5000");
    println!("orphan sweep: {small_sweep:?} for 500 items, {large_sweep:?} for 5000");

    assert!(
        large_facets < small_facets * TEN_FOLD_CEILING,
        "facets cost {large_facets:?} against {small_facets:?}: a join stopped using its index"
    );
    assert!(
        large_sweep < small_sweep * TEN_FOLD_CEILING,
        "the sweep cost {large_sweep:?} against {small_sweep:?}: a NOT EXISTS stopped using its index"
    );
}
