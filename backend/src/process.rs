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
use crate::listener::{fired, stop_on, until_stopped};
use crate::state::AppState;
use crate::{api, commands, db, error, http, jobs, listener, services};

pub fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Error: the runtime cannot start: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(async {
        match run().await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                // One sentence, in the log once set up so a JSON log keeps it, else on stderr.
                if tracing::dispatcher::has_been_set() {
                    log_error!("{error}");
                } else {
                    eprintln!("Error: {error}");
                }
                std::process::ExitCode::FAILURE
            }
        }
    });
    listener::end(runtime, listener::ABANDON);
    code
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

    // The working directory's alone, held to the rule of every other variable.
    let dotenv = crate::config::read_dotenv(&std::env::current_dir()?)?;
    if let Some(file) = &dotenv {
        dotenvy::from_path(file)?;
    }

    let mut config = Config::from_env()?;
    if let Some(file) = dotenv {
        config.startup_notes.push(format!("Variables read from {}", file.display()));
    }
    match invocation {
        commands::Invocation::Healthcheck => return commands::healthcheck(&config).await,
        commands::Invocation::ResetAccount { revoke_keys, force } => {
            return commands::reset_account(&config, revoke_keys, force).await;
        }
        commands::Invocation::Restore(archive) => return commands::restore(&config, archive).await,
        commands::Invocation::DecryptBackup(archive, out) => {
            return commands::decrypt_backup(&config, archive, out).await;
        }
        commands::Invocation::Serve
        | commands::Invocation::Help
        | commands::Invocation::Version => {}
    }
    config.validate()?;
    init_tracing(&config);
    crate::log_panics();
    for note in config.startup_notes.iter().chain(&config.source_notes()) {
        warn!("{note}");
    }

    info!("Starting Routarr v{}", env!("CARGO_PKG_VERSION"));

    // The data directory, absolute: `ROUTARR_DB_PATH` defaults to a relative path, so the
    // working directory decides which library opens.
    info!(
        "Data directory: {}",
        // `canonicalize` fails before the directory exists, at the first start, where `absolute`
        // still answers.
        std::fs::canonicalize(&config.data_dir)
            .or_else(|_| std::path::absolute(&config.data_dir))
            .unwrap_or_else(|_| config.data_dir.clone())
            .display()
    );

    // Heard from here on, so a stop asked while the start archives or migrates ends it.
    let (stopping, hurrying) = stop_on(shutdown_signal(), shutdown_signal());
    let Some(storage) = until_stopped(crate::open_storage(&config), stopping.clone()).await else {
        info!("Routarr stopped before its database was open");
        return Ok(());
    };
    let (api_key, pool, secrets) = storage?;
    let job_registry = jobs::JobRegistry::new(pool.clone());
    job_registry.recover_orphans().await?;

    let state = AppState {
        http: http::build_client(&config)?,
        audit: Arc::new(services::audit::Log::storing(pool.clone())),
        deprecated_calls: Arc::new(api::deprecated::Calls::counting(pool.clone())),
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
        backup_copy_written: Arc::default(),
        config: Arc::new(config),
    };

    match until_stopped(converge(&state), stopping.clone()).await {
        Some(converged) => converged?,
        None => {
            db::checkpoint_and_close(&state.pool).await;
            info!("Routarr stopped before it served");
            return Ok(());
        }
    }
    // The archive a migration took, and those a share asleep missed, copied away.
    let started = jobs::Attribution::unattended(jobs::TRIGGER_SCHEDULE);
    services::backup::copy_in_background(&state, started);

    // The single account, generated on first start like the API key, in `forms` mode only.
    if state.config.auth_mode == AuthMode::Forms {
        services::accounts::ensure_account(&state.pool, &state.config.password_path()).await?;
    }

    crate::warn_on_insecure_defaults(&state);

    // Stopped before the pool closes: SQLite will not truncate a WAL another
    // connection is writing, which is what a sweep in flight is doing. Told
    // with the server, so no pass starts while the requests drain.
    let scheduler = jobs::scheduler::start(state.clone(), stopping.clone());
    state.jobs.close_when(fired(stopping.clone()));

    let bind_addr = state.config.bind_address();
    let limits = listener::Limits::of(&state.config);
    // Kept past `build_router`, which consumes the state: the pool closes once the server and
    // the work in flight have stopped.
    let pool = state.pool.clone();
    let jobs = state.jobs.clone();
    let audit = Arc::clone(&state.audit);
    let deprecated_calls = Arc::clone(&state.deprecated_calls);
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
        limits,
    )
    .await;

    // Fits the drain and this wait in the Compose file's 20 seconds of its 30-second grace: past
    // it a sweep rolls back and only the WAL truncation is lost. A second signal stops waiting.
    let waits = async {
        tokio::join!(
            jobs.wait(std::time::Duration::from_secs(15)),
            tokio::time::timeout(std::time::Duration::from_secs(10), scheduler)
        )
    };
    tokio::select! {
        (drained, scheduler) = waits => {
            if !drained {
                warn!("Moves were still being recorded after 15s, closing the database anyway");
            }
            if scheduler.is_err() {
                warn!("The scheduler did not stop within 10s, closing the database anyway");
            }
        }
        () = fired(hurrying) => warn!("Closing the database without waiting for the work in flight"),
    }

    audit.flush().await;
    deprecated_calls.flush().await;
    db::checkpoint_and_close(&pool).await;

    // `scripts/smoke-image.sh` looks for this line: reworded, it fails the image check.
    info!("Routarr stopped cleanly");
    Ok(())
}

/// What a start does once the database is open and before it serves, each step left as safe
/// by a stop as by a kill: a step cut short is done again at the next start.
async fn converge(state: &AppState) -> error::AppResult<()> {
    services::maintenance::converge(state).await?;
    api::categories::converge_names(state).await?;
    api::instances::converge_names(&state.pool).await?;
    // What a conversion interrupted by the last stop left in the clear.
    services::backup::resume_sealing(state).await;
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

    if config.log_format.eq_ignore_ascii_case("json") {
        let local = tracing_subscriber::fmt::time::ChronoLocal::rfc_3339();
        registry.with(tracing_subscriber::fmt::layer().json().with_timer(local)).init();
    } else {
        // Colours only on a terminal: in `docker logs` and in a file the
        // escape codes stand between a fail2ban filter and the address.
        let colours = std::io::IsTerminal::is_terminal(&std::io::stdout());
        registry.with(crate::logging::text_layer(std::io::stdout, colours)).init();
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
