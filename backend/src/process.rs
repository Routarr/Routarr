//! The process: its command line, its start, its signals and its stop.
//!
//! Only a running process reaches this file, so the coverage measure sets it
//! apart. What a test can drive lives elsewhere: the route table and the
//! storage in `main.rs`, the subcommands in `commands`, the stop in `listener`.

use std::sync::Arc;

use tracing::error as log_error;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::{AuthMode, Config};
use crate::listener::{fired, stop_on};
use crate::state::AppState;
use crate::{api, commands, db, error, http, jobs, listener, services};

#[tokio::main]
pub async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // In one sentence, never Rust's dump of the error: through the log
            // once it is set up, so a JSON log keeps the line, else on stderr.
            if tracing::dispatcher::has_been_set() {
                log_error!("{error}");
            } else {
                eprintln!("Error: {error}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = commands::invocation(&args)?;
    // Before the configuration is read: neither needs it, and neither may
    // fail on it.
    match invocation {
        commands::Invocation::Help => {
            println!("{}", commands::USAGE);
            return Ok(());
        }
        commands::Invocation::Version => {
            println!("routarr {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }

    dotenvy::dotenv().ok();

    let config = Config::from_env()?;
    match invocation {
        commands::Invocation::Healthcheck => return commands::healthcheck(&config).await,
        commands::Invocation::ResetAccount { revoke_keys } => {
            return commands::reset_account(&config, revoke_keys).await;
        }
        commands::Invocation::Restore(archive) => return commands::restore(&config, archive).await,
        commands::Invocation::DecryptBackup(archive, out) => {
            return commands::decrypt_backup(&config, archive, out).await;
        }
        commands::Invocation::Serve
        | commands::Invocation::Help
        | commands::Invocation::Version => {}
    }
    // Before anything binds a port: a value the server cannot honour should
    // stop it with a sentence naming the variable, not with a panic from a
    // dependency four layers down.
    config.validate()?;
    init_tracing(&config);
    crate::log_panics();
    for note in config.startup_notes.iter().chain(&config.source_notes()) {
        warn!("{note}");
    }

    info!("Starting Routarr v{}", env!("CARGO_PKG_VERSION"));

    // Say where the data is, absolutely. `ROUTARR_DB_PATH` defaults to a
    // *relative* path, so the working directory decides which library the
    // server opens. Started from two places, it makes two installations, each
    // with its own key and its own backups, and nothing on screen says which
    // one is in use.
    info!(
        "Data directory: {}",
        // `canonicalize` fails until the directory exists, which is the first
        // start, the one where this line matters most. `absolute` answers
        // then, since it only needs the working directory.
        std::fs::canonicalize(&config.data_dir)
            .or_else(|_| std::path::absolute(&config.data_dir))
            .unwrap_or_else(|_| config.data_dir.clone())
            .display()
    );

    let (api_key, pool, secrets) = crate::open_storage(&config).await?;
    let job_registry = jobs::JobRegistry::new(pool.clone());
    job_registry.recover_orphans().await?;

    let state = AppState {
        http: http::build_client(&config)?,
        audit: Arc::new(services::audit::Log::storing(pool.clone())),
        pool,
        secrets,
        tvdb_token: Arc::new(tokio::sync::Mutex::new(None)),
        paces: Default::default(),
        library_passes: Default::default(),
        jobs: job_registry,
        api_key: Arc::new(std::sync::RwLock::new(api_key)),
        sign_in: Arc::new(Default::default()),
        oidc_provider: Arc::new(tokio::sync::RwLock::new(None)),
        post_sync: Arc::new(tokio::sync::Mutex::new(None)),
        auto_apply_held: Arc::default(),
        notifications: Arc::default(),
        key_rates: Arc::default(),
        route_misses: Arc::default(),
        last_probe: Arc::default(),
        config: Arc::new(config),
    };

    services::maintenance::converge(&state).await?;
    api::categories::converge_names(&state).await?;
    // What a conversion interrupted by the last stop left in the clear.
    services::backup::resume_sealing(&state).await;

    // The single account, generated on first start like the API key. Only in
    // the mode that reads it: creating one for an installation that
    // authenticates another way would write a password nobody asked for.
    if state.config.auth_mode == AuthMode::Forms {
        services::accounts::ensure_account(&state.pool, &state.config.password_path()).await?;
    }

    crate::warn_on_insecure_defaults(&state);

    // Stopped before the pool closes: SQLite will not truncate a WAL another
    // connection is writing, which is what a sweep in flight is doing. Told
    // with the server, so no pass starts while the requests drain.
    let (stopping, hurrying) = stop_on(shutdown_signal(), shutdown_signal());
    let scheduler = jobs::scheduler::start(state.clone(), stopping.clone());

    let bind_addr = state.config.bind_address();
    // Kept past `build_router`, which consumes the state: the pool has to be
    // closed *after* the server stops and the work in flight has recorded
    // what it did, not dropped along with it.
    let pool = state.pool.clone();
    let jobs = state.jobs.clone();
    let audit = Arc::clone(&state.audit);
    let app = crate::build_router(state);

    let socket = tokio::net::TcpListener::bind(&bind_addr).await.map_err(|e| {
        error::AppError::Config(format!(
            "Routarr cannot listen on {bind_addr} (ROUTARR_HOST, ROUTARR_PORT): {e}"
        ))
    })?;
    info!("Routarr web server listening on http://{bind_addr}");

    listener::serve(
        socket,
        app,
        fired(stopping),
        fired(hurrying.clone()),
        listener::HEADER_READ_TIMEOUT,
        listener::DRAIN,
    )
    .await;

    // Bounded: a sweep talking to an unreachable Arr would otherwise hold the
    // shutdown open for the full connect timeout, and a runtime that has sent
    // SIGTERM is counting. Past the deadline SQLite rolls the sweep back and
    // only the WAL truncation is lost. An apply ends before its next move and
    // records the moves the Arr has made. With the drain, the stop fits the
    // Compose file's 30s grace, and a second signal stops waiting.
    let waits = async {
        tokio::join!(
            jobs.drain(std::time::Duration::from_secs(20)),
            tokio::time::timeout(std::time::Duration::from_secs(10), scheduler)
        )
    };
    tokio::select! {
        (drained, scheduler) = waits => {
            if !drained {
                warn!("Moves were still being recorded after 20s, closing the database anyway");
            }
            if scheduler.is_err() {
                warn!("The scheduler did not stop within 10s, closing the database anyway");
            }
        }
        () = fired(hurrying) => warn!("Closing the database without waiting for the work in flight"),
    }

    audit.flush().await;
    db::checkpoint_and_close(&pool).await;

    // `scripts/smoke-image.sh` looks for this line: reworded, it fails the image check.
    info!("Routarr stopped cleanly");
    Ok(())
}

fn init_tracing(config: &Config) {
    let filter = config
        .log_filter
        .as_deref()
        .and_then(|filter| tracing_subscriber::EnvFilter::try_new(filter).ok())
        .unwrap_or_else(|| {
            format!("routarr={},tower_http=warn,sqlx=warn", config.log_level).into()
        });

    let registry = tracing_subscriber::registry().with(filter);
    // The time of the place the operator set in `TZ`, as Radarr and Sonarr log
    // it, its offset written so the line reads the same anywhere.
    let local = tracing_subscriber::fmt::time::ChronoLocal::rfc_3339();

    if config.log_format.eq_ignore_ascii_case("json") {
        registry.with(tracing_subscriber::fmt::layer().json().with_timer(local)).init();
    } else {
        // Colours only on a terminal: in `docker logs` and in a file the
        // escape codes stand between a fail2ban filter and the address.
        let colours = std::io::IsTerminal::is_terminal(&std::io::stdout());
        registry.with(tracing_subscriber::fmt::layer().with_ansi(colours).with_timer(local)).init();
    }
}

/// Resolve on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl-C, shutting down"),
        _ = terminate => info!("Received SIGTERM, shutting down"),
    }
}
