//! The process: its command line, its start, its signals and its stop.
//!
//! Only a running process reaches this file, so the coverage measure sets it
//! apart. What a test can drive lives elsewhere: the route table, the storage
//! and what a start settles before it serves in `main.rs`, the state in `state`,
//! the subcommands in `commands`, the stop in `listener`.

use std::sync::Arc;

use tracing::error as log_error;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;
use crate::listener::{fired, stop_on, until_stopped};
use crate::logging::LocalTime;
use crate::state::AppState;
use crate::{commands, db, error, jobs, listener};

pub fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match commands::invocation(&args) {
        Ok(invocation) => invocation,
        Err(refused) => {
            eprintln!("Error: {refused}");
            return std::process::ExitCode::FAILURE;
        }
    };
    #[cfg(windows)]
    if invocation == commands::Invocation::Service {
        return crate::service::dispatch();
    }
    std::process::ExitCode::from(run_to_end(invocation, Stops::Console))
}

/// What asks the server to stop.
pub enum Stops {
    /// A signal, or the console it runs in.
    Console,
    /// The stop controls the Windows service manager sent, counted.
    #[cfg(windows)]
    Service(tokio::sync::watch::Receiver<u32>),
}

impl Stops {
    /// Where an error goes that ends the start before the log is open: a service has no console,
    /// so the installer's data folder keeps it in the log file it would have written.
    fn unlogged_errors_to(&self) -> Option<std::path::PathBuf> {
        match self {
            Self::Console => None,
            #[cfg(windows)]
            Self::Service(_) => std::env::var_os("ProgramData")
                .map(|data| std::path::Path::new(&data).join("Routarr").join("routarr.log")),
        }
    }

    /// The stop, and a second one that hurries it.
    fn heard(self, hangup: bool) -> (BoxFuture<'static, ()>, BoxFuture<'static, ()>) {
        match self {
            Self::Console => (Box::pin(shutdown_signal(hangup)), Box::pin(shutdown_signal(hangup))),
            #[cfg(windows)]
            Self::Service(asked) => {
                (Box::pin(asked_times(asked.clone(), 1)), Box::pin(asked_times(asked, 2)))
            }
        }
    }
}

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Run `invocation` on a runtime of its own until it ends, and answer its exit status.
pub fn run_to_end(invocation: commands::Invocation, stops: Stops) -> u8 {
    let unlogged = stops.unlogged_errors_to();
    let refused = |said: &str| {
        eprintln!("Error: {said}");
        if let Some(file) = &unlogged {
            keep_unlogged(file, said);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            refused(&format!("the runtime cannot start: {e}"));
            return 1;
        }
    };
    let status = runtime.block_on(async {
        match run(invocation, stops).await {
            Ok(()) => 0,
            Err(error) => {
                // One sentence, in the log once set up so a JSON log keeps it, else on stderr.
                if tracing::dispatcher::has_been_set() {
                    log_error!("{error}");
                } else {
                    refused(&error.to_string());
                }
                error::exit_status(error.as_ref())
            }
        }
    });
    listener::end(runtime, listener::ABANDON);
    status
}

async fn run(
    invocation: commands::Invocation,
    stops: Stops,
) -> Result<(), Box<dyn std::error::Error>> {
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

    // The working directory's, or the package's for the binary a package installed, held to the
    // rule of every other variable.
    let exe = std::env::current_exe().ok();
    let dotenv = crate::config::variables_file(&std::env::current_dir()?, exe.as_deref())?;
    if let Some(file) = &dotenv {
        dotenvy::from_path(file)?;
    }

    let mut config = Config::from_env()?;
    if let Some(file) = dotenv {
        config.startup_notes.push(format!("Variables read from {}", file.display()));
    }
    crate::install::settle(config.install);
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
        #[cfg(windows)]
        commands::Invocation::Service => {}
    }
    // A service has no console to log to. Beside a relative library the start is refused, and the
    // file would land in the Windows folder first.
    #[cfg(windows)]
    if matches!(stops, Stops::Service(_)) && config.data_dir.is_absolute() {
        config.log_file.get_or_insert_with(|| config.data_dir.join("routarr.log"));
    }
    config.validate()?;
    let log_file = match &config.log_file {
        Some(path) => Some(crate::logging::RotatingFile::open(path).map_err(|e| {
            error::AppError::Config(format!(
                "ROUTARR_LOG_FILE: {} cannot be written ({e})",
                path.display()
            ))
        })?),
        None => None,
    };
    init_tracing(&config, log_file);
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

    // Held until the process ends, so a second server on this library is refused.
    db::prepare_data_dir(&config)?;
    let _instance = db::hold_the_data_dir(&config)?;

    // Heard from here on, so a stop asked while the start archives or migrates ends it. Read
    // before either handler replaces the disposition it inherited.
    let (first, second) = stops.heard(!listener::hangup_ignored());
    let (stopping, hurrying) = stop_on(first, second);
    let told = stopping.clone();
    tokio::spawn(async move {
        fired(told).await;
        crate::systemd::notify("STOPPING=1");
        #[cfg(windows)]
        crate::service::stopping();
    });
    let Some(storage) = until_stopped(crate::open_storage(&config), stopping.clone()).await else {
        info!("Routarr stopped before its database was open");
        return Ok(());
    };
    let (api_key, pool, secrets) = storage?;
    let state = AppState::new(config, pool, secrets, api_key)?;

    match until_stopped(crate::prepare(&state), stopping.clone()).await {
        Some(prepared) => prepared?,
        None => {
            db::checkpoint_and_close(&state.pool).await;
            info!("Routarr stopped before it served");
            return Ok(());
        }
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

    // Not a refused configuration: an address not up yet at boot is there at a later try.
    let socket = tokio::net::TcpListener::bind(&bind_addr).await.map_err(|e| {
        format!("Routarr cannot listen on {bind_addr} (ROUTARR_HOST, ROUTARR_PORT): {e}")
    })?;
    info!("Routarr web server listening on http://{bind_addr}");
    crate::systemd::notify("READY=1");
    #[cfg(windows)]
    crate::service::ready();

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

/// The log on standard output, and in `file` too when `ROUTARR_LOG_FILE` names one.
fn init_tracing(config: &Config, file: Option<std::sync::Mutex<crate::logging::RotatingFile>>) {
    let filter = config
        .log_filter
        .as_deref()
        .and_then(|filter| tracing_subscriber::EnvFilter::try_new(filter).ok())
        .unwrap_or_else(|| {
            format!("routarr={},tower_http=warn,sqlx=warn", config.log_level).into()
        });

    let registry = tracing_subscriber::registry().with(filter);

    if config.log_format.eq_ignore_ascii_case("json") {
        let file = file.map(|file| {
            tracing_subscriber::fmt::layer().json().with_timer(LocalTime).with_writer(file)
        });
        registry
            .with(tracing_subscriber::fmt::layer().json().with_timer(LocalTime))
            .with(file)
            .init();
    } else {
        // Colours only on a terminal: in `docker logs` and in a file the
        // escape codes stand between a fail2ban filter and the address.
        let colours = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let file = file.map(|file| crate::logging::text_layer(file, false));
        registry.with(crate::logging::text_layer(std::io::stdout, colours)).with(file).init();
    }
}

/// Add the error that ended a start to `file`, the time first, as a log line would say it.
fn keep_unlogged(file: &std::path::Path, said: &str) {
    use std::io::Write;
    let line = format!("{} ERROR Routarr did not start: {said}\n", jiff::Timestamp::now());
    let kept = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut log| log.write_all(line.as_bytes()));
    if let Err(e) = kept {
        eprintln!("Error: {} could not keep it: {e}", file.display());
    }
}

/// Resolve once the service manager has sent `times` stop controls.
#[cfg(windows)]
async fn asked_times(mut asked: tokio::sync::watch::Receiver<u32>, times: u32) {
    if asked.wait_for(|count| *count >= times).await.is_err() {
        std::future::pending::<()>().await;
    }
    info!("The Windows service manager asked Routarr to stop, shutting down");
}

/// Resolve on Ctrl-C or on the system's own stop, logging which.
async fn shutdown_signal(hangup: bool) {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl-C handler");
    };
    let heard = tokio::select! {
        () = ctrl_c => "Ctrl-C",
        heard = system_stop(hangup) => heard,
    };
    info!("Received {heard}, shutting down");
}

/// SIGTERM or, when `hangup`, SIGHUP, the terminal it ran in closing.
#[cfg(unix)]
async fn system_stop(hangup: bool) -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let heard = |kind: SignalKind| async move {
        signal(kind).expect("failed to install a signal handler").recv().await;
    };
    let hangup = async move {
        match hangup {
            true => heard(SignalKind::hangup()).await,
            false => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        () = heard(SignalKind::terminate()) => "SIGTERM",
        () = hangup => "SIGHUP",
    }
}

/// Ctrl-Break, the console window closing, the user logging off or Windows shutting down. Windows
/// ends the process some seconds after the last three, the checkpoint started meanwhile.
#[cfg(windows)]
async fn system_stop(_: bool) -> &'static str {
    use tokio::signal::windows;
    let installed = "failed to install a console handler";
    let mut ctrl_break = windows::ctrl_break().expect(installed);
    let mut close = windows::ctrl_close().expect(installed);
    let mut logoff = windows::ctrl_logoff().expect(installed);
    let mut shutdown = windows::ctrl_shutdown().expect(installed);
    tokio::select! {
        _ = ctrl_break.recv() => "Ctrl-Break",
        _ = close.recv() => "the console closing",
        _ = logoff.recv() => "the user logging off",
        _ = shutdown.recv() => "Windows shutting down",
    }
}

#[cfg(not(any(unix, windows)))]
async fn system_stop(_: bool) -> &'static str {
    std::future::pending().await
}
