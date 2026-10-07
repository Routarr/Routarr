use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Router, middleware};
use std::sync::Arc;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::error as log_error;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod config;
mod crypto;
mod db;
mod error;
mod http;
mod integrations;
mod jobs;
mod listener;
mod localization;
mod models;
mod paths;
mod race;
mod services;
mod state;

#[cfg(test)]
mod tests;

use config::{AuthMode, Config};
use state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let config = Config::from_env()?;
    match std::env::args().nth(1).as_deref() {
        Some("healthcheck") => return healthcheck(&config).await,
        Some("reset-account") => {
            let revoke_keys = std::env::args().skip(2).any(|arg| arg == "--revoke-keys");
            return reset_account(&config, revoke_keys).await;
        }
        Some("restore") => return restore(&config, std::env::args().nth(2)).await,
        Some("decrypt-backup") => {
            let mut args = std::env::args().skip(2);
            return decrypt_backup(&config, args.next(), args.next()).await;
        }
        _ => {}
    }
    // Before anything binds a port: a value the server cannot honour should
    // stop it with a sentence naming the variable, not with a panic from a
    // dependency four layers down.
    config.validate()?;
    init_tracing(&config);
    log_panics();
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

    let (api_key, pool, secrets) = open_storage(&config).await?;
    let job_registry = jobs::JobRegistry::new(pool.clone());
    job_registry.recover_orphans().await?;

    let state = AppState {
        http: http::build_client(&config)?,
        audit: Arc::new(services::audit::Log::storing(pool.clone())),
        pool,
        secrets,
        tvdb_token: Arc::new(tokio::sync::Mutex::new(None)),
        paces: Default::default(),
        jobs: job_registry,
        api_key: Arc::new(std::sync::RwLock::new(api_key)),
        sign_in: Arc::new(Default::default()),
        oidc_provider: Arc::new(tokio::sync::RwLock::new(None)),
        post_sync: Arc::new(tokio::sync::Mutex::new(None)),
        auto_apply_held: Arc::default(),
        notifications: Arc::default(),
        key_rates: Arc::default(),
        route_misses: Arc::default(),
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

    warn_on_insecure_defaults(&state);

    // Stopped before the pool closes: SQLite will not truncate a WAL another
    // connection is writing, which is what a sweep in flight is doing.
    let (stop_scheduler, scheduler_stopped) = tokio::sync::watch::channel(false);
    let scheduler = jobs::scheduler::start(state.clone(), scheduler_stopped);

    let bind_addr = state.config.bind_address();
    // Kept past `build_router`, which consumes the state: the pool has to be
    // closed *after* the server stops and the work in flight has recorded
    // what it did, not dropped along with it.
    let pool = state.pool.clone();
    let jobs = state.jobs.clone();
    let audit = Arc::clone(&state.audit);
    let app = build_router(state);

    let socket = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("Routarr web server listening on http://{bind_addr}");

    listener::serve(socket, app, shutdown_signal(), listener::HEADER_READ_TIMEOUT).await;

    // Bounded: a sweep talking to an unreachable Arr would otherwise hold the
    // shutdown open for the full connect timeout, and a runtime that has sent
    // SIGTERM is counting. Past the deadline SQLite rolls the sweep back and
    // only the WAL truncation is lost. An apply ends before its next move and
    // records the moves the Arr has made, within the Compose file's 30s grace.
    let _ = stop_scheduler.send(true);
    let (drained, scheduler) = tokio::join!(
        jobs.drain(std::time::Duration::from_secs(20)),
        tokio::time::timeout(std::time::Duration::from_secs(10), scheduler)
    );
    if !drained {
        warn!("Moves were still being recorded after 20s, closing the database anyway");
    }
    if scheduler.is_err() {
        warn!("The scheduler did not stop within 10s, closing the database anyway");
    }

    audit.flush().await;
    db::checkpoint_and_close(&pool).await;

    // `scripts/smoke-image.sh` looks for this line: reworded, it fails the image check.
    info!("Routarr stopped cleanly");
    Ok(())
}

/// `routarr healthcheck`, the image's HEALTHCHECK. Only the API's own answer
/// counts: a mount point the probe got wrong reaches the page the interface
/// falls back to, which answers 200 as well.
async fn healthcheck(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let answer: serde_json::Value =
        client.get(config.ping_url()).send().await?.error_for_status()?.json().await?;
    if answer["status"] == "ok" {
        Ok(())
    } else {
        Err(format!("{} answered without the ping's status", config.ping_url()).into())
    }
}

/// `routarr restore <archive>`: stage a backup for the next start with the
/// server stopped, which is the way back when a start refuses the database.
async fn restore(
    config: &Config,
    archive: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(archive) = archive else {
        return Err(
            "usage: routarr restore <archive>, a name from the backup folder or a path".into()
        );
    };
    let manifest =
        asking_passphrase(|given| services::backup::stage_offline(config, &archive, given)).await?;
    println!(
        "{archive}, taken by Routarr v{} at schema {}, is restored at the next start.",
        manifest.version, manifest.schema
    );
    if !manifest.includes_master_key {
        println!(
            "It carries no master key: the credentials in it open only with the key in place."
        );
    }
    Ok(())
}

/// `routarr decrypt-backup <archive> <zip>`: write an encrypted archive
/// opened, to read it by hand.
async fn decrypt_backup(
    config: &Config,
    archive: Option<String>,
    out: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (Some(archive), Some(out)) = (archive, out) else {
        return Err("usage: routarr decrypt-backup <archive> <zip>, the archive a name from the \
                    backup folder or a path, the zip a path outside it"
            .into());
    };
    let written =
        asking_passphrase(|given| services::backup::decrypt_offline(config, &archive, &out, given))
            .await?;
    println!(
        "{} written. It carries the master key in clear: delete it once read.",
        written.display()
    );
    Ok(())
}

/// Run `attempt` with the archive passphrase from the environment, and again
/// with one typed at the prompt each time it answers that one is needed,
/// while there is a terminal to type it in.
async fn asking_passphrase<T, Attempt, Answer>(
    mut attempt: Attempt,
) -> Result<T, Box<dyn std::error::Error>>
where
    Attempt: FnMut(Option<services::backup::Passphrase>) -> Answer,
    Answer: std::future::Future<Output = error::AppResult<T>>,
{
    let mut given = config::archive_passphrase().map(services::backup::passphrase_of);
    loop {
        match attempt(given.clone()).await {
            Err(error::AppError::PassphraseRequired(refusal))
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) =>
            {
                println!("{refusal}");
                let typed = rpassword::prompt_password("Passphrase: ")?;
                given = Some(services::backup::passphrase_of(typed));
            }
            done => return Ok(done?),
        }
    }
}

/// What a start reads from disk, in the one order that works: a staged
/// restore first, since it swaps the database, the master key and the API key
/// files, then what those files hold.
pub(crate) async fn open_storage(
    config: &config::Config,
) -> error::AppResult<(Option<String>, sqlx::SqlitePool, crypto::SecretBox)> {
    // Before the pool opens, since a restore swaps the database file itself,
    // which cannot be done safely underneath live connections. And before the
    // API key is read, since the archive can carry another one: read first,
    // the old key is served until the restart after, when it changes under
    // every client without a word.
    services::backup::sweep_leftovers(config);
    if services::backup::apply_pending_restore(config).await? {
        info!("A staged backup was restored");
    }

    // Resolve authentication before anything can serve a request. An API that
    // moves files must not be open because a variable was forgotten.
    //
    // The environment wins over the stored key rather than the other way round.
    // The stored one is generated, not chosen: an operator who pins
    // ROUTARR_API_KEY in their compose file after a first start would otherwise
    // find the value they declared silently ignored in favour of a file they
    // never wrote. Declared configuration outranks generated state, and that is
    // also why a rotation is refused while the variable is set: it could not
    // survive the next restart.
    // The other modes need no key, but one minted from the interface for a
    // script has to survive a restart: they read the stored one, never make one.
    let api_key = if config.api_key.is_none() && config.auth_mode == AuthMode::ApiKey {
        let path = config.api_key_path();
        let (key, generated) = crypto::load_or_generate_api_key(&path)?;
        if generated {
            // The path and never the key, which a log shipper would keep.
            // `scripts/smoke-image.sh` looks for this line: reworded, it fails the image check.
            info!("Generated an API key at {}. Read it from that file", path.display());
        }
        Some(key)
    } else {
        state::resolve_api_key(config)
    };

    let pool = db::init_pool(config).await?;
    refuse_a_lost_master_key(config, &pool).await?;
    let salt = crypto::installation_salt(&pool).await?;
    let secrets = crypto::SecretBox::load(
        config.secret_key.as_deref(),
        config.previous_secret_key.as_deref(),
        &config.secret_key_path(),
        Some(salt.as_bytes()),
    )?;
    Ok((api_key, pool, secrets))
}

/// Refuse a start that would make a new master key while the database holds
/// values sealed with the one it had: `routarr.db` copied alone to a new
/// volume, a key file deleted, or emptied by a power cut. A new key opens none
/// of them, and the start would go on with every instance and source failing.
async fn refuse_a_lost_master_key(
    config: &config::Config,
    pool: &sqlx::SqlitePool,
) -> error::AppResult<()> {
    let path = config.secret_key_path();
    if config.secret_key.is_some() {
        return Ok(());
    }
    match std::fs::read_to_string(&path) {
        Ok(key) if !key.trim().is_empty() => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            // Refused by `SecretBox::load`, which names the file.
            return Ok(());
        }
        Ok(_) | Err(_) => {}
    }
    let sealed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM instances WHERE api_key LIKE 'enc:v%')
             OR EXISTS(SELECT 1 FROM settings WHERE value LIKE 'enc:v%')
             OR EXISTS(SELECT 1 FROM webhook_secrets WHERE secret LIKE 'enc:v%')",
    )
    .fetch_one(pool)
    .await?;
    if !sealed {
        return Ok(());
    }
    if config.allow_new_master_key {
        warn!(
            "A new master key is made at {}: every Arr key, source key, notification address and \
             signing secret stored before it has to be entered again",
            path.display()
        );
        return Ok(());
    }
    Err(error::AppError::Config(format!(
        "{} is missing or empty, and the database holds credentials sealed with the key it held. \
         Put the file back from a backup, or set ROUTARR_SECRET_KEY to the key they were sealed \
         with. To start with a new key and enter every credential again, set \
         ROUTARR_ALLOW_NEW_MASTER_KEY=true",
        path.display()
    )))
}

/// `routarr reset-account`, for an operator locked out of the `forms` account:
/// `docker exec routarr /app/routarr reset-account`. The image carries no
/// `sqlite3`, and the server can keep running, since a sign-in reads the
/// account each time.
///
/// With `--revoke-keys`, every application key is revoked and a new API key is
/// written, for an operator who believes keys leaked with the password.
async fn reset_account(
    config: &Config,
    revoke_keys: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::init_pool(config).await?;
    let password = services::accounts::reset_account(&pool, &config.password_path()).await?;
    let revoked =
        if revoke_keys { Some(services::applications::revoke_all(&pool).await?) } else { None };
    pool.close().await;
    println!(
        "The account is reset. Sign in as '{}' with: {password}",
        services::accounts::DEFAULT_USERNAME
    );
    println!(
        "The password is also in {}. Every session was closed.",
        config.password_path().display()
    );
    if let Some(revoked) = revoked {
        println!("Every application key was revoked: {revoked}.");
        if config.api_key.is_some() {
            println!("ROUTARR_API_KEY pins the API key: change the variable and restart.");
        } else {
            let path = config.api_key_path();
            crypto::write_api_key(&path, &crypto::generate_secret()?)?;
            println!(
                "A new API key is in {}. Restart Routarr for it to replace the old one.",
                path.display()
            );
        }
    }
    Ok(())
}

/// The one route authenticated by its path, for an Arr that sends no custom
/// header: the per-instance token travels in the URL.
const WEBHOOK_ROUTE: &str = "/webhook/{instance_id}/{token}";

async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "not_found",
            "message": "No such API route.",
        })),
    )
        .into_response()
}

/// What the request span records as the path.
///
/// The webhook token is a credential, and the span is on every line logged
/// while a delivery is served, the error line an operator pastes into a
/// ticket included, so that route is recorded as its template. Every other
/// path is logged as sent: an id in it is what makes a line findable.
fn loggable_path(request: &axum::extract::Request) -> String {
    match request.extensions().get::<axum::extract::MatchedPath>() {
        Some(matched) if matched.as_str().ends_with(WEBHOOK_ROUTE) => matched.as_str().to_owned(),
        _ => request.uri().path().to_owned(),
    }
}

/// Assemble the API, middleware stack and static frontend.
fn build_router(state: AppState) -> Router {
    let config = Arc::clone(&state.config);

    // Unauthenticated liveness probe: safe to expose to a container runtime or
    // an uptime monitor because it reveals nothing about the library.
    let public = Router::new()
        .route("/ping", get(api::health::ping))
        // The login-less shell needs its strings before it can render anything,
        // including an authentication error.
        .route("/localization", get(api::localization::dictionary))
        .route("/localization/languages", get(api::localization::languages))
        // Which gate the shell must show, and the exchange behind it. Outside
        // the middleware by necessity: a browser with no session cannot be
        // asked for one to learn that it needs one.
        .route("/auth/mode", get(api::auth::mode))
        .route("/auth/logout", post(api::auth::logout))
        // The contract describes the software, not the installation, and a
        // client reads it before it holds a key.
        .route("/openapi.json", get(api::contract::serve));
    // Each way in exists in its own mode only. Elsewhere a request for the
    // provider's start logs an error on an install that has no provider, and
    // `/auth/login` checks a password a former `forms` mode left behind.
    let public = match config.auth_mode {
        AuthMode::Forms => public.route("/auth/login", post(api::auth::login)),
        // The browser leaves and comes back, so both ends are outside the
        // middleware: it has no session yet on the way out, and the provider's
        // redirect carries none on the way in.
        AuthMode::Oidc => public
            .route("/auth/oidc/start", get(api::auth::oidc_start))
            .route("/auth/oidc/callback", get(api::auth::oidc_callback)),
        // The browser's way in: the key sent once for a session cookie, so it
        // is kept nowhere a script on the page could read it.
        AuthMode::ApiKey => public.route("/auth/key-session", post(api::auth::key_session)),
        AuthMode::None | AuthMode::External => public,
    };

    let protected = Router::new()
        .route("/auth/me", get(api::auth::me))
        .route("/auth/password", put(api::account::change_password))
        .route(
            "/auth/api-key",
            post(api::account::rotate_api_key).delete(api::account::delete_api_key),
        )
        .route("/auth/sessions", get(api::account::sessions).delete(api::account::end_sessions))
        .route("/auth/sessions/{handle}", delete(api::account::end_session))
        // Keys for other applications, each held to its own scopes. Owner-only,
        // like every route `api::applications::GRANTS` does not name.
        .route("/applications", get(api::applications::list).post(api::applications::create))
        .route("/applications/{id}", delete(api::applications::revoke))
        .route("/status", get(api::health::status))
        .route("/health", get(api::health::health_check))
        // Behind the API key like everything else that describes the library:
        // instance and category names are in there. Prometheus reaches it with
        // `authorization: credentials`, which the middleware already accepts as
        // `Bearer`.
        .route("/metrics", get(api::metrics::metrics))
        .route("/instances", get(api::instances::list).post(api::instances::create))
        .route(
            "/instances/{id}",
            get(api::instances::get_one).put(api::instances::update).delete(api::instances::remove),
        )
        .route("/instances/sync", post(api::instances::sync_all))
        .route("/instances/test", post(api::instances::probe))
        .route("/instances/{id}/test", post(api::instances::test))
        .route("/instances/{id}/sync", post(api::instances::sync_now))
        .route("/instances/{id}/webhook-token", post(api::instances::rotate_webhook_token))
        .route("/root-folders", get(api::root_folders::list).post(api::root_folders::create))
        .route("/root-folders/{id}", axum::routing::delete(api::root_folders::delete))
        .route("/root-folders/conflicts", get(api::root_folders::conflicts))
        .route("/root-folders/{id}/category", put(api::root_folders::update_category))
        .route("/categories", get(api::categories::list).post(api::categories::create))
        .route("/categories/{id}", put(api::categories::rename).delete(api::categories::remove))
        .route("/rules", get(api::rules::list).post(api::rules::create))
        .route("/rules/export", get(api::rules::export))
        .route("/rules/import", post(api::rules::import))
        .route("/rules/preview", post(api::rules::preview))
        .route("/rules/validate", post(api::rules::validate))
        .route("/rules/conditions", get(api::conditions::condition_catalog))
        // Pinned expectations. The preview says what a change *would* do, and
        // these say what it must not do: with first-match-by-priority, inserting
        // one rule rebalances every rule below it.
        .route("/rule-tests", get(api::rule_tests::list).post(api::rule_tests::create))
        .route("/rule-tests/run", post(api::rule_tests::run))
        // Which rules never win. Validation looks inside a rule. This is the
        // only thing that looks between them, which is where first-match-by-
        // priority puts its one trap.
        .route("/rules/health", get(api::rules::health))
        // What the library actually holds, per axis a condition reads. Nothing
        // else groups `media`, and without it writing a rule means guessing
        // what is present and finding out by trial.
        .route("/media/facets", get(api::media::facets))
        .route("/rule-tests/{id}", delete(api::rule_tests::delete))
        .route("/rules/reorder", post(api::rules::reorder))
        .route(
            "/rules/{id}",
            get(api::rules::get_one).put(api::rules::update).delete(api::rules::remove),
        )
        .route("/rules/{id}/duplicate", post(api::rules::duplicate))
        .route("/media", get(api::media::list))
        .route("/media/{id}", get(api::media::get_one))
        .route("/media/{id}/explain", get(api::media::explain))
        // Where a title another service names would go, whether the library
        // holds it or only an Arr knows it. Stores nothing.
        .route("/route", get(api::media::place))
        .route("/simulate", post(api::simulation::run))
        .route("/decisions", get(api::decisions::list))
        .route("/decisions/apply", post(api::decisions::apply))
        .route("/decisions/apply-all", post(api::decisions::apply_all))
        .route("/decisions/revert", post(api::decisions::revert))
        .route("/overrides", get(api::overrides::list).post(api::overrides::create))
        // Pins by the id another service gives a title, for an application that
        // knows the title by its TMDB, TheTVDB or IMDb id and not by Routarr's.
        .route(
            "/overrides/external",
            put(api::overrides::pin_external).delete(api::overrides::unpin_external),
        )
        .route("/overrides/{id}", delete(api::overrides::remove))
        .route("/jobs", get(api::jobs::list))
        .route("/jobs/{id}", get(api::jobs::get_one))
        .route("/jobs/{id}/cancel", post(api::jobs::cancel))
        .route("/logs", get(api::logs::list))
        .route("/logs/export", get(api::logs::export))
        .route("/security-log", get(api::security_log::list))
        .route("/security-log/export", get(api::security_log::export))
        .route("/maintenance/purge", post(api::maintenance::purge))
        .route("/backups", get(api::backup::list).post(api::backup::create))
        // The name is validated against a generated shape before it ever
        // becomes a path: these are the only routes where a caller picks a file
        // in the directory that also holds the master key.
        .route("/backups/{name}", get(api::backup::download).delete(api::backup::remove))
        .route("/backups/{name}/restore", post(api::backup::restore))
        .route("/backups/passphrase", put(api::backup::set_passphrase))
        .route("/settings", get(api::settings::get_all).put(api::settings::update))
        .route(
            "/notifications/webhook-secret",
            get(api::notifications::signing)
                .post(api::notifications::rotate_signing)
                .delete(api::notifications::remove_signing),
        )
        .route("/notifications/test", post(api::notifications::test))
        .route("/onboarding", get(api::onboarding::get).put(api::onboarding::update))
        .route("/metadata/providers", get(api::metadata::list))
        .route("/metadata/refresh", post(api::metadata::refresh))
        .route("/config/export", get(api::config::export))
        .route("/config/import", post(api::config::import))
        .route_layer(middleware::from_fn_with_state(state.clone(), api::auth::authenticate));

    // Webhooks authenticate with their own per-instance token, in a header or
    // in the path, so they sit outside the API-key middleware.
    let webhooks = Router::new()
        .route(WEBHOOK_ROUTE, post(api::webhook::receive))
        .route("/webhook/{instance_id}", post(api::webhook::receive_with_header));

    // A miss under the API prefix is a JSON 404, whatever the method: left to
    // the application's fallback it would answer `index.html` with a 200, and
    // a script with a typo in its path would parse HTML as JSON.
    let api_routes = public.merge(protected).merge(webhooks).fallback(api_not_found);

    let api = Router::new().nest(&format!("{}/api/v1", config.base_path), api_routes);
    let app = request_layers(api)
        // Rules and import bundles are the only large bodies. 2 MiB is generous
        // for them and stops an unauthenticated request from buffering
        // unbounded input.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(cors_layer(&config))
        .with_state(state);

    // Compression wraps the finished router so it also covers the static
    // frontend bundle attach_frontend adds: a layer attached earlier only
    // applies to the routes registered before it. The security headers go
    // outermost for the same reason: they have to reach the served HTML, not
    // just the API.
    attach_frontend(app, &config)
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(security_headers))
}

/// How long a request body may take to arrive, whole. A body sent a byte at a
/// time would otherwise hold its connection open as long as the sender likes.
/// The largest body taken is 2 MiB, seconds on the slowest link.
pub(crate) const BODY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The layers every API request passes through, around `routes`.
///
/// One function for the router and for the tests that make a handler panic: a
/// route added to the assembled router would sit outside every layer, since a
/// `.layer()` wraps only the routes registered before it.
pub(crate) fn request_layers<S>(routes: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    routes
        // Every request gets an id, carried on the response and in the span of
        // every line logged while serving it: a user pasting one
        // `X-Request-Id` from the browser's network panel is how a failure in
        // the log is matched to the click that caused it. Set outermost, so the
        // trace layer inside already sees it.
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        // Inside the request-id layers, so a panic still answers with the
        // `X-Request-Id` the trace span recorded, which is the whole point of
        // having one. Without this layer a panicking handler drops the
        // connection: no status, no body, nothing to match against the log, and
        // a client that cannot tell a bug from a cut cable.
        //
        // It hides nothing: `log_panics` logs the panic in the request span,
        // and the caller learns something too.
        .layer(CatchPanicLayer::custom(panic_response))
        .layer(tower_http::timeout::RequestBodyDeadlineLayer::new(BODY_DEADLINE))
        .layer(middleware::map_response(not_stored))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// An API answer may carry a key, a webhook token or an archive holding the
/// master key, and a shared browser profile or a proxy cache must keep none of
/// them. A handler that states its own caching keeps it.
async fn not_stored(mut response: Response) -> Response {
    response
        .headers_mut()
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-store"));
    response
}

/// The span every line logged while serving a request sits in, and the one a
/// handler's panic is logged in: its `id` is the `X-Request-Id` the response
/// carries.
pub(crate) fn request_span(request: &axum::extract::Request) -> tracing::Span {
    tracing::info_span!(
        "request",
        method = %request.method(),
        uri = %loggable_path(request),
        id = request.headers().get("x-request-id").and_then(|v| v.to_str().ok()),
    )
}

/// Log a panic through `tracing`, in the span entered where it happened.
///
/// The standard hook prints plain text to stderr: no request id, and a line
/// that is not JSON under `ROUTARR_LOG_FORMAT=json`. A handler's panic is
/// logged inside its request span instead, with a backtrace when
/// `RUST_BACKTRACE` asks for one. With no subscriber to take the line, as in a
/// test that never set one, the hook in place before this one prints it.
pub(crate) fn log_panics() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let logged = tracing::dispatcher::get_default(|dispatch| {
                !dispatch.is::<tracing::subscriber::NoSubscriber>()
            });
            if !logged {
                return previous(info);
            }
            let cause = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("a panic with no message");
            let location = info.location().map(ToString::to_string).unwrap_or_default();
            let backtrace = std::backtrace::Backtrace::capture();
            if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
                log_error!(%location, %backtrace, "Panicked: {cause}");
            } else {
                log_error!(%location, "Panicked: {cause}");
            }
        }));
    });
}

/// What a panicking handler answers.
///
/// The same shape as every other error this API returns, so the interface's
/// client unwraps it like any other: `describeError` appends the request id to
/// a 5xx, which is exactly the case this is.
pub(crate) fn panic_response(_: Box<dyn std::any::Any + Send + 'static>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "error": "internal",
            "message": "Something went wrong. The request id is in the response headers.",
        })),
    )
        .into_response()
}

/// Headers every response carries.
///
/// The API key lives in the browser's `localStorage`, so script injection is
/// the vector that would hand it to someone else: `script-src 'self'` is what
/// closes it, whatever ends up in the DOM.
///
/// `style-src` keeps `'unsafe-inline'`, a stated weakening. A few elements take
/// a size or a colour mix computed from data as an inline `style` attribute (the
/// confidence meter and its dot, the facet bars, a task's progress, the table
/// skeleton, the size a screen hands a dialog), and CSP does not distinguish one
/// from an injected `<style>` block. Styles cannot read `localStorage`, scripts
/// can, and those are locked down. A per-response nonce on those elements is what
/// would close it.
///
/// Everything else is same-origin: the application makes no external request.
/// `Cross-Origin-Opener-Policy` severs `window.opener`, which costs nothing
/// while nothing calls `window.open` and holds if that changes.
async fn security_headers(request: axum::extract::Request, next: middleware::Next) -> Response {
    use axum::http::header::{HeaderName, HeaderValue};

    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    for (name, value) in [
        (
            axum::http::header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; \
             base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
        ),
        (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (axum::http::header::REFERRER_POLICY, "same-origin"),
        (HeaderName::from_static("cross-origin-opener-policy"), "same-origin"),
        (
            HeaderName::from_static("permissions-policy"),
            "accelerometer=(), camera=(), geolocation=(), microphone=(), payment=(), usb=()",
        ),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }

    response
}

/// Serve the built SPA, falling back to `index.html` for client-side routes.
fn attach_frontend(app: Router, config: &Config) -> Router {
    if !config.frontend_dir.exists() {
        info!("Frontend dir {} not found, API-only mode active", config.frontend_dir.display());
        return app;
    }

    info!("Serving frontend assets from: {}", config.frontend_dir.display());
    let index_html = index_html(config);

    // The index is served from memory rather than from disk because it is
    // rewritten (see `index_html`), and rewriting it per request would be waste.
    let fallback = get(move || {
        let body = index_html.clone();
        async move { Html(body) }
    });

    // `append_index_html_on_directories(false)` matters: left on, ServeDir
    // answers `/` with index.html straight off disk and the rewrite below never
    // runs. Off, `/` misses and falls through to the handler like any other
    // client-side route.
    let service = ServeDir::new(&config.frontend_dir)
        .append_index_html_on_directories(false)
        .fallback(fallback);

    if config.base_path.is_empty() {
        app.fallback_service(service)
    } else {
        // Mounted under a sub-path, anything outside it is not ours to answer.
        app.nest_service(&config.base_path, service)
    }
}

/// Read `index.html` and point its relative asset URLs at the mount point.
///
/// The frontend is built with `base: './'`, so it asks for `./assets/…`. Loaded
/// from `/routarr/` that resolves correctly, but a deep link like
/// `/routarr/rules` would resolve it to `/routarr/rules/assets/…` and 404, and
/// the page would come up blank behind the proxy and only there. A `<base href>`
/// pins the resolution to the mount point whatever the URL depth, and doubles
/// as how the frontend discovers where it is mounted (`document.baseURI`).
fn index_html(config: &Config) -> String {
    let path = config.frontend_dir.join("index.html");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        log_error!("Cannot read {}: {e}", path.display());
        String::new()
    });

    let href = format!("{}/", config.base_path);
    match raw.split_once("<head>") {
        Some((head, tail)) => format!("{head}<head>\n    <base href=\"{href}\">{tail}"),
        // No <head> means a file we do not recognise. Serving it untouched is
        // better than serving a mangled one.
        None => raw,
    }
}

/// CORS is opt-in.
///
/// Any origin allowed with any header, on an API without authentication, lets
/// any page the user visits drive the API. Same-origin by default.
/// `ROUTARR_CORS_ORIGINS` names any other origin allowed.
fn cors_layer(config: &Config) -> CorsLayer {
    if config.cors_origins.is_empty() {
        return CorsLayer::new();
    }

    let origins: Vec<_> = config
        .cors_origins
        .iter()
        .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
        .collect();

    info!("CORS enabled for {} origin(s)", origins.len());

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderName::from_static("x-api-key"),
            // `respond-async`, which the contract documents for a long call.
            axum::http::HeaderName::from_static("prefer"),
        ])
        // What a page needs to follow a 202 and to report a failure: the task
        // `Location` names, how long to wait, and the request's id for the log.
        .expose_headers([
            axum::http::header::LOCATION,
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderName::from_static("preference-applied"),
            axum::http::HeaderName::from_static("x-request-id"),
        ])
        .allow_credentials(true)
}

fn init_tracing(config: &Config) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        format!("routarr={},tower_http=warn,sqlx=warn", config.log_level).into()
    });

    let registry = tracing_subscriber::registry().with(filter);

    if config.log_format.eq_ignore_ascii_case("json") {
        registry.with(tracing_subscriber::fmt::layer().json()).init();
    } else {
        // Colours only on a terminal: in `docker logs` and in a file the
        // escape codes stand between a fail2ban filter and the address.
        let colours = std::io::IsTerminal::is_terminal(&std::io::stdout());
        registry.with(tracing_subscriber::fmt::layer().with_ansi(colours)).init();
    }
}

/// Make the security posture explicit at startup rather than a surprise later.
fn warn_on_insecure_defaults(state: &AppState) {
    if state.config.auth_mode == AuthMode::External {
        warn!(
            "ROUTARR_AUTH=external: Routarr asks for no credential and trusts the reverse \
             proxy in front of it. Anything that reaches this port bypasses that proxy, so \
             bind it to the proxy's network and nowhere else."
        );
    }
    if state.config.auth_mode == AuthMode::Oidc && state.config.oidc_allow_anyone {
        warn!(
            "ROUTARR_OIDC_ALLOW_ANYONE=true: every account the OpenID Connect provider \
             authenticates signs in with full access. Name the people or the groups who may in \
             ROUTARR_OIDC_ALLOWED_SUBJECTS or ROUTARR_OIDC_ALLOWED_GROUPS instead."
        );
    }
    if state.config.auth_mode == AuthMode::None {
        // Only reachable by asking for it, so this states a decision back to
        // whoever made it rather than reporting an omission.
        warn!(
            "ROUTARR_AUTH=none: the API is unauthenticated. Make sure Routarr is only \
             reachable from a trusted network, or from a proxy that authenticates for it."
        );
    }
}

/// Resolve on Ctrl-C or SIGTERM so in-flight requests finish before exit.
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
