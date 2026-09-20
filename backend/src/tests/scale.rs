//! The simulation must not go to the database once per media item.
//!
//! Every other test seeds one or two media items, so a regression to a query
//! per item passes the whole suite and surfaces only on a large library.
//!
//! The assertion is an invariance, not a stopwatch: the number of times a
//! simulation reaches for the database must not change when the library grows
//! ten-fold. A wall-clock bound would measure the runner instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use crate::services::maintenance;
use crate::services::routing::{self, SimulationOptions};
use crate::tests::TestApp;

/// A pool that counts how many times it hands out its connection.
///
/// One connection, so every statement executed against the pool has to acquire
/// the same idle connection and trip the hook. Per-pool rather than a global
/// counter, so tests running in parallel cannot pollute each other's reading.
async fn counting_pool() -> (SqlitePool, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let hook = Arc::clone(&counter);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .before_acquire(move |_conn, _meta| {
            hook.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok(true) })
        })
        .connect_with(SqliteConnectOptions::new().in_memory(true).foreign_keys(true))
        .await
        .expect("in-memory sqlite");

    crate::db::run_migrations(&pool).await.expect("migrations");
    (pool, counter)
}

/// A library of `count` films across one instance, with a rule that matches
/// roughly half of them and a mapped root folder for each category.
async fn seed(pool: &SqlitePool, count: usize) {
    sqlx::query(
        "INSERT INTO instances (id, name, instance_type, base_url, api_key, enabled)
         VALUES ('inst-1', 'Radarr', 'radarr', 'http://radarr:7878', 'k', 1)",
    )
    .execute(pool)
    .await
    .unwrap();

    // `standard` is seeded by the initial migration; only `anime` is new.
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

/// Simulate `count` items and report how many times the database was reached.
async fn queries_for(count: usize) -> (usize, std::time::Duration) {
    let (pool, counter) = counting_pool().await;
    seed(&pool, count).await;

    // Seeding is not part of the measurement.
    counter.store(0, Ordering::Relaxed);
    let started = Instant::now();

    let result =
        routing::run_simulation(&pool, SimulationOptions { persist: true, ..Default::default() })
            .await
            .unwrap();
    assert_eq!(result.total_media, count, "the whole library was not evaluated");

    let elapsed = started.elapsed();
    let queries = counter.load(Ordering::Relaxed);
    pool.close().await;
    (queries, elapsed)
}

#[tokio::test]
async fn a_simulation_does_not_query_once_per_media_item() {
    let (small, small_time) = queries_for(200).await;
    let (large, large_time) = queries_for(2000).await;

    println!("200 items: {small} queries in {small_time:?}");
    println!("2000 items: {large} queries in {large_time:?}");

    // Ten times the library, the same handful of queries. A per-item query
    // would put roughly 1 800 more on the second reading.
    assert!(
        large <= small + 5,
        "the library grew ten-fold and the database was reached {large} times instead of {small}: \
         the simulation is querying per item again"
    );
}

/// A preview evaluates two rule sets over one library. Loaded once, an
/// evaluation reaches the database for nothing: every table it reads is in
/// memory, so the second rule set costs no query and a plain run is one load
/// and nothing more.
#[tokio::test]
async fn an_evaluation_over_a_loaded_library_costs_no_query() {
    let (pool, counter) = counting_pool().await;
    seed(&pool, 200).await;

    counter.store(0, Ordering::Relaxed);
    let library = routing::load_library(&pool, &SimulationOptions::default()).await.unwrap();
    let loading = counter.load(Ordering::Relaxed);
    assert!(loading > 0, "loading reads the library");

    let without_rules = routing::simulate_loaded(
        &pool,
        &library,
        SimulationOptions { rules_override: Some(Vec::new()), ..Default::default() },
    )
    .await
    .unwrap();
    let with_rules =
        routing::simulate_loaded(&pool, &library, SimulationOptions::default()).await.unwrap();
    assert_eq!(counter.load(Ordering::Relaxed), loading, "an evaluation reached the database");
    assert_eq!(without_rules.total_media, 200);
    assert_ne!(
        without_rules.moves_required, with_rules.moves_required,
        "the two rule sets must have been evaluated apart"
    );
    drop(library);

    counter.store(0, Ordering::Relaxed);
    routing::run_simulation(&pool, SimulationOptions::default()).await.unwrap();
    assert_eq!(counter.load(Ordering::Relaxed), loading, "a run is one load and nothing more");
}

#[tokio::test]
async fn a_simulation_stays_within_a_sane_time_at_scale() {
    // A loose ceiling, not a benchmark: this catches quadratic work that is not
    // in the queries — a merge that rescans, an O(n²) lookup — which the count
    // above cannot see. Generous enough not to fail on a busy CI runner.
    let (_, elapsed) = queries_for(5000).await;
    println!("5000 items simulated in {elapsed:?}");

    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "simulating 5 000 items took {elapsed:?}, which is not linear work any more"
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
                "INSERT INTO metadata_cache (source, external_id, media_type, genres, expires_at)
                 VALUES (?, ?, 'movie', '[\"Action\"]', '2099-01-01')",
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

/// The two queries that join `media` to `metadata_cache` across every identifier
/// namespace. Written as one OR over three cast columns they cannot use an
/// index, and on a large library the facets take minutes and the hourly purge
/// holds a connection for as long. The ceiling is loose — a busy runner must
/// not fail it — and still an order of magnitude under what a scan costs.
#[tokio::test]
async fn facets_and_the_orphan_sweep_stay_indexed_at_scale() {
    let app = TestApp::new().await;
    let count = 5000;
    seed(&app.state.pool, count).await;
    seed_cache(&app.state.pool, count).await;

    let started = Instant::now();
    let response = app.get("/api/v1/media/facets").await;
    let facets = started.elapsed();
    let body = response.assert_ok();
    assert_eq!(body["total_media"], count as i64);
    println!("facets over {count} items and {} cache rows: {facets:?}", count * 2);
    assert!(
        facets < std::time::Duration::from_secs(5),
        "facets took {facets:?}: a join stopped using its index"
    );

    let started = Instant::now();
    let report = maintenance::run(&app.state, "test").await.unwrap();
    let sweep = started.elapsed();
    println!("orphan sweep over {} cache rows: {sweep:?}", count * 2);
    assert_eq!(report.metadata_cache_removed, 0, "every cache row belongs to a media item");
    assert!(
        sweep < std::time::Duration::from_secs(5),
        "the sweep took {sweep:?}: a NOT EXISTS stopped using its index"
    );
}
