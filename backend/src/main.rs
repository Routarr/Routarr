use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Router, middleware};
use std::sync::Arc;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::error as log_error;
use tracing::{info, warn};

mod api;
mod commands;
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
mod process;
mod race;
mod services;
mod state;
mod timestamp;

#[cfg(test)]
mod tests;

use config::{AuthMode, Config};
use state::AppState;

fn main() -> std::process::ExitCode {
    process::main()
}

/// What a start reads from disk, in the one order that works: a staged
/// restore first, since it swaps the database, the master key and the API key
/// files, then what those files hold.
pub(crate) async fn open_storage(
    config: &config::Config,
) -> error::AppResult<(Option<String>, sqlx::SqlitePool, crypto::SecretBox)> {
    // Before the pool opens and the key is read: a restore swaps the database and key files.
    db::prepare_data_dir(config)?;
    services::backup::sweep_leftovers(config);
    if services::backup::apply_pending_restore(config).await? {
        info!("A staged backup was restored");
    }

    // A declared ROUTARR_API_KEY outranks the generated file, which the other modes only read.
    let api_key = if config.api_key.is_none() && config.auth_mode == AuthMode::ApiKey {
        let path = config.api_key_path();
        let (key, generated) = crypto::load_or_generate_api_key(&path)?;
        if generated {
            // The path, never the key. `scripts/smoke-image.sh` looks for this exact line.
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

/// Refuse a start that would make a new master key while the database holds values sealed
/// with the old one, as when `routarr.db` was copied alone: a new key opens none of them.
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

/// A path that exists, asked with a method it does not take. The router adds
/// the `Allow` header that lists the ones it does.
async fn api_method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        axum::Json(serde_json::json!({
            "error": "method_not_allowed",
            "message": "This API route does not take this method.",
        })),
    )
        .into_response()
}

/// What the request span records as the path: the webhook route as its template, since its
/// token is a credential, and every other path as sent, so an id in it stays findable.
fn loggable_path(request: &axum::extract::Request) -> String {
    match request.extensions().get::<axum::extract::MatchedPath>() {
        Some(matched) if matched.as_str().ends_with(WEBHOOK_ROUTE) => matched.as_str().to_owned(),
        _ => request.uri().path().to_owned(),
    }
}

/// Assemble the API, middleware stack and static frontend.
fn build_router(state: AppState) -> Router {
    let config = Arc::clone(&state.config);

    // The unauthenticated liveness probe, which reveals nothing about the library.
    let public = Router::new()
        .route("/ping", get(api::health::ping))
        // The shell needs its strings before anything renders, an authentication error included.
        .route("/localization", get(api::localization::dictionary))
        .route("/localization/languages", get(api::localization::languages))
        // Outside the middleware, since a browser with no session learns here that it needs one.
        .route("/auth/mode", get(api::auth::mode))
        .route("/auth/logout", post(api::auth::logout))
        // The contract describes the software, and a client reads it before it holds a key.
        .route("/openapi.json", get(api::contract::serve));
    // Each way in exists in its own mode only, or `/auth/login` checks a stale `forms` password.
    let public = match config.auth_mode {
        AuthMode::Forms => public.route("/auth/login", post(api::auth::login)),
        // Both ends outside the middleware: no session on the way out, none on the redirect back.
        AuthMode::Oidc => public
            .route("/auth/oidc/start", get(api::auth::oidc_start))
            .route("/auth/oidc/callback", get(api::auth::oidc_callback)),
        // The key sent once for a session cookie, kept nowhere a page script could read it.
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
        // Keys for other applications. Owner-only, like every route `GRANTS` does not name.
        .route("/applications", get(api::applications::list).post(api::applications::create))
        .route("/applications/{id}", delete(api::applications::revoke))
        .route("/status", get(api::health::status))
        .route("/health", get(api::health::health_check))
        // Behind the key, since it names instances. Prometheus authenticates as a bearer token.
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
        .route("/root-folders/suggestions", get(api::root_folders::suggestions))
        .route(
            "/root-folders/{id}/dismissal",
            post(api::root_folders::dismiss).delete(api::root_folders::undismiss),
        )
        .route("/root-folders/{id}/category", put(api::root_folders::update_category))
        .route("/categories", get(api::categories::list).post(api::categories::create))
        .route("/categories/{id}", put(api::categories::rename).delete(api::categories::remove))
        .route("/rules", get(api::rules::list).post(api::rules::create))
        .route("/rules/export", get(api::rules::export))
        .route("/rules/import", post(api::rules::import))
        .route("/rules/preview", post(api::rules::preview))
        .route("/rules/validate", post(api::rules::validate))
        .route("/rules/conditions", get(api::conditions::condition_catalog))
        .route("/rules/templates", get(api::templates::list))
        .route("/rules/templates/{id}/preview", post(api::templates::preview))
        .route("/rules/templates/{id}/import", post(api::templates::import))
        // Pinned expectations: inserting one rule can change what every rule below it decides.
        .route("/rule-tests", get(api::rule_tests::list).post(api::rule_tests::create))
        .route("/rule-tests/run", post(api::rule_tests::run))
        // Which rules never win: the one check between rules, where first match sets its trap.
        .route("/rules/health", get(api::rules::health))
        // What the library holds per axis a condition reads, so a rule need not guess.
        .route("/library/facets", get(api::media::facets))
        .route("/rule-tests/{id}", delete(api::rule_tests::delete))
        .route("/rules/reorder", post(api::rules::reorder))
        .route(
            "/rules/{id}",
            get(api::rules::get_one).put(api::rules::update).delete(api::rules::remove),
        )
        .route("/rules/{id}/duplicate", post(api::rules::duplicate))
        .route("/library", get(api::media::list))
        .route("/library/{id}", get(api::media::get_one))
        .route("/library/{id}/explain", get(api::media::explain))
        .route("/library/{id}/keep", post(api::overrides::keep))
        // Where a title another service names would go, held or not. Stores nothing.
        .route("/route", get(api::media::place))
        .route("/simulate", post(api::simulation::run))
        .route("/decisions", get(api::decisions::list))
        .route("/decisions/apply", post(api::decisions::apply))
        .route("/decisions/apply-all", post(api::decisions::apply_all))
        .route("/decisions/revert", post(api::decisions::revert))
        .route("/decisions/revert-all", post(api::decisions::revert_all))
        .route("/exceptions", get(api::overrides::list).post(api::overrides::create))
        // Pins by the TMDB, TheTVDB or IMDb id another service gives a title.
        .route(
            "/exceptions/external",
            put(api::overrides::pin_external).delete(api::overrides::unpin_external),
        )
        .route("/exceptions/{id}", delete(api::overrides::remove))
        .route("/tasks", get(api::jobs::list))
        .route("/tasks/{id}", get(api::jobs::get_one))
        .route("/tasks/{id}/cancel", post(api::jobs::cancel))
        .route("/move-log", get(api::logs::list))
        .route("/move-log/export", get(api::logs::export))
        .route("/security-log", get(api::security_log::list))
        .route("/security-log/export", get(api::security_log::export))
        .route("/maintenance/purge", post(api::maintenance::purge))
        .route("/backups", get(api::backup::list).post(api::backup::create))
        // The name is checked against the generated shape: the folder also holds the master key.
        .route("/backups/{name}", get(api::backup::download).delete(api::backup::remove))
        .route("/backups/{name}/restore", post(api::backup::restore))
        .route("/backups/passphrase", put(api::backup::set_passphrase))
        .route("/backups/copy/adopt", post(api::backup::adopt_copy_folder))
        .route("/settings", get(api::settings::get_all).put(api::settings::update))
        .route(
            "/notifications/webhook-secret",
            get(api::notifications::signing)
                .post(api::notifications::rotate_signing)
                .delete(api::notifications::remove_signing),
        )
        .route("/notifications/test", post(api::notifications::test))
        .route("/onboarding", get(api::onboarding::get).put(api::onboarding::update))
        .route("/sources", get(api::metadata::list))
        .route("/sources/refresh", post(api::metadata::refresh))
        .route("/sources/anime-ids", get(api::metadata::anime_ids))
        .route("/sources/anime-ids/download", post(api::metadata::download_anime_ids))
        .route(
            "/sources/anime-ids/import",
            post(api::metadata::import_anime_ids)
                .layer(DefaultBodyLimit::max(services::anime_ids::MAX_BYTES)),
        )
        .route("/config/export", get(api::config::export))
        .route("/config/import", post(api::config::import))
        // The old paths of the routes above (`api::deprecated::DEPRECATED_PATHS`), each with its
        // new path's handlers, answered until their removal before 1.0.
        .route("/overrides", get(api::overrides::list).post(api::overrides::create))
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
        .route("/media", get(api::media::list))
        .route("/media/facets", get(api::media::facets))
        .route("/media/{id}", get(api::media::get_one))
        .route("/media/{id}/explain", get(api::media::explain))
        .route("/media/{id}/keep", post(api::overrides::keep))
        .route("/metadata/providers", get(api::metadata::list))
        .route("/metadata/refresh", post(api::metadata::refresh))
        .route_layer(middleware::from_fn_with_state(state.clone(), api::deprecated::signal))
        .route_layer(middleware::from_fn_with_state(state.clone(), api::auth::authenticate));

    // Authenticated by their own per-instance token, outside the API-key middleware.
    let webhooks = Router::new()
        .route(WEBHOOK_ROUTE, post(api::webhook::receive))
        .route("/webhook/{instance_id}", post(api::webhook::receive_with_header));

    // A JSON 404 for any miss under the API prefix, or a typo answers `index.html` with a 200.
    let api_routes = public
        .merge(protected)
        .merge(webhooks)
        .method_not_allowed_fallback(api_method_not_allowed)
        .fallback(api_not_found);

    let mut api = Router::new().nest(&format!("{}/api/v1", config.base_path), api_routes);
    // Under a sub-path, the bare root leads in instead of answering an empty 404.
    if !config.base_path.is_empty() {
        let mounted = format!("{}/", config.base_path);
        let lead_in = move || async move { axum::response::Redirect::temporary(&mounted) };
        api = api.route("/", get(lead_in));
    }
    let app = request_layers(api)
        // 2 MiB fits rules and import bundles, and bounds what an unauthenticated request buffers.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(cors_layer(&config))
        .with_state(state.clone());

    // Last, to cover the frontend. An archive stays uncompressed, keeping its download length.
    let compressed = DefaultPredicate::new()
        .and(NotForContentType::const_new("application/zip"))
        .and(NotForContentType::const_new("application/octet-stream"));
    attach_frontend(app, &config, state)
        .layer(CompressionLayer::new().compress_when(compressed))
        .layer(middleware::from_fn(security_headers))
}

/// How long a request body may take to arrive, whole. A body sent a byte at a
/// time would otherwise hold its connection open as long as the sender likes.
/// The largest body taken is 2 MiB, seconds on the slowest link.
pub(crate) const BODY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The layers every API request passes through, around `routes`, shared with the tests
/// that make a handler panic, since a `.layer()` wraps only the routes registered before it.
pub(crate) fn request_layers<S>(routes: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    routes
        // The span carries the request id, set outermost, so a logged failure matches its click.
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        // Inside the request-id layers, so a panic's answer still carries its `X-Request-Id`.
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

/// Log a panic through `tracing`, in the span entered where it happened, with a backtrace
/// when `RUST_BACKTRACE` asks. With no subscriber, as in a test, the previous hook prints it.
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

/// What a panicking handler answers, in the envelope every error shares, so the interface
/// appends the request id to it as to any 5xx.
pub(crate) fn panic_response(_: Box<dyn std::any::Any + Send + 'static>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "error": "internal_error",
            "message": "Something went wrong. The request id is in the response headers.",
        })),
    )
        .into_response()
}

/// Headers every response carries. `script-src 'self'` keeps injected markup from running.
/// `style-src` keeps `'unsafe-inline'` for the `style` attributes computed from data, a
/// weakening a nonce per response would close. Everything else is same-origin.
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
fn attach_frontend(app: Router, config: &Config, state: AppState) -> Router {
    if !config.frontend_dir.exists() {
        info!("Frontend dir {} not found, API-only mode active", config.frontend_dir.display());
        return app;
    }

    info!("Serving frontend assets from: {}", config.frontend_dir.display());
    let page = index_html(config);

    // Its strings are added per request and never cached, since the language can change.
    let fallback = get(move || {
        let page = page.clone();
        let state = state.clone();
        async move {
            let html = with_dictionary(&page, &api::localization::answer(&state).await);
            ([(axum::http::header::CACHE_CONTROL, "no-cache")], Html(html))
        }
    });

    // Off, or ServeDir answers `/` with index.html off disk and the rewrite never runs.
    let service = ServeDir::new(&config.frontend_dir)
        .append_index_html_on_directories(false)
        .fallback(fallback);
    let service = Router::new()
        .fallback_service(service)
        .layer(middleware::from_fn(keep_hashed_files))
        .layer(middleware::from_fn(say_utf8));

    if config.base_path.is_empty() {
        app.fallback_service(service)
    } else {
        // Mounted under a sub-path, anything outside it is not ours to answer.
        app.nest_service(&config.base_path, service)
    }
}

/// A file under `assets/` is named after a hash of its content, so a browser may keep it
/// for good. The page naming them is never kept, nor a miss answered with the page.
async fn keep_hashed_files(request: axum::extract::Request, next: middleware::Next) -> Response {
    let hashed = request.uri().path().starts_with("/assets/");
    let mut response = next.run(request).await;
    if hashed && response.status().is_success() {
        response
            .headers_mut()
            .entry(axum::http::header::CACHE_CONTROL)
            .or_insert(axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    response
}

/// Every text file the frontend build writes is UTF-8, and `ServeDir` names the type
/// without its charset, so a guessing browser may garble a name outside ASCII.
async fn say_utf8(request: axum::extract::Request, next: middleware::Next) -> Response {
    use axum::http::{HeaderValue, header::CONTENT_TYPE};

    let mut response = next.run(request).await;
    let named = response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok());
    if let Some(text) = named.filter(|named| named.starts_with("text/") && !named.contains(';'))
        && let Ok(value) = HeaderValue::from_str(&format!("{text}; charset=utf-8"))
    {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    response
}

/// Read `index.html` and pin its relative asset URLs to the mount point with a `<base href>`,
/// or a deep link behind a proxy asks for `./assets/` under its own path and gets a 404.
/// The frontend reads its mount point back from `document.baseURI`.
fn index_html(config: &Config) -> String {
    let path = config.frontend_dir.join("index.html");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        log_error!("Cannot read {}: {e}", path.display());
        String::new()
    });

    let href = format!("{}/", config.base_path);
    match raw.split_once("<head>") {
        Some((head, tail)) => format!("{head}<head>\n    <base href=\"{href}\">{tail}"),
        // No <head> means a file we do not recognise: served untouched, never mangled.
        None => raw,
    }
}

/// The page with `payload`, the `/localization` answer, before its `</head>`, so the first
/// paint fetches nothing. Every `<` is escaped, so no string can close the element, and a
/// JSON element is no script for the CSP to refuse.
pub(crate) fn with_dictionary(page: &str, payload: &impl serde::Serialize) -> String {
    let json = serde_json::to_string(payload).unwrap_or_default().replace('<', "\\u003c");
    match page.split_once("</head>") {
        Some((head, tail)) => format!(
            "{head}<script type=\"application/json\" id=\"dictionary\">{json}</script>\n</head>{tail}"
        ),
        None => page.to_string(),
    }
}

/// CORS is opt-in, same-origin by default, since any origin with any header lets every page
/// the user visits drive the API. `ROUTARR_CORS_ORIGINS` names the others allowed.
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
        // What a page needs to follow a 202 and to report a failure, the request id included.
        .expose_headers([
            axum::http::header::LOCATION,
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderName::from_static("preference-applied"),
            axum::http::HeaderName::from_static("x-request-id"),
        ])
        .allow_credentials(true)
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
        // Only reachable by asking for it, so this states a decision back to whoever made it.
        warn!(
            "ROUTARR_AUTH=none: the API is unauthenticated. Make sure Routarr is only \
             reachable from a trusted network, or from a proxy that authenticates for it."
        );
    }
}
