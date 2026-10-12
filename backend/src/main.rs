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
    // Before the pool opens, since a restore swaps the database file itself,
    // which cannot be done safely underneath live connections. And before the
    // API key is read, since the archive can carry another one: read first,
    // the old key is served until the restart after, when it changes under
    // every client without a word.
    db::prepare_data_dir(config)?;
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
    let api_routes = public
        .merge(protected)
        .merge(webhooks)
        .method_not_allowed_fallback(api_method_not_allowed)
        .fallback(api_not_found);

    let mut api = Router::new().nest(&format!("{}/api/v1", config.base_path), api_routes);
    // Mounted under a sub-path, the root, where someone typing the bare
    // address lands, leads in rather than answering an empty 404.
    if !config.base_path.is_empty() {
        let mounted = format!("{}/", config.base_path);
        let lead_in = move || async move { axum::response::Redirect::temporary(&mounted) };
        api = api.route("/", get(lead_in));
    }
    let app = request_layers(api)
        // Rules and import bundles are the only large bodies. 2 MiB is generous
        // for them and stops an unauthenticated request from buffering
        // unbounded input.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(cors_layer(&config))
        .with_state(state.clone());

    // Compression wraps the finished router so it also covers the static
    // frontend bundle attach_frontend adds: a layer attached earlier only
    // applies to the routes registered before it. The security headers go
    // outermost for the same reason: they have to reach the served HTML, not
    // just the API.
    attach_frontend(app, &config, state)
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
            "error": "internal_error",
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
fn attach_frontend(app: Router, config: &Config, state: AppState) -> Router {
    if !config.frontend_dir.exists() {
        info!("Frontend dir {} not found, API-only mode active", config.frontend_dir.display());
        return app;
    }

    info!("Serving frontend assets from: {}", config.frontend_dir.display());
    let page = index_html(config);

    // Read once and served from memory with its mount point set once (see
    // `index_html`). Its strings are added per request: the language is a
    // setting that changes while the server runs. Not stored by the browser,
    // which would keep the strings of the language it was last served in.
    let fallback = get(move || {
        let page = page.clone();
        let state = state.clone();
        async move {
            let html = with_dictionary(&page, &api::localization::answer(&state).await);
            ([(axum::http::header::CACHE_CONTROL, "no-cache")], Html(html))
        }
    });

    // `append_index_html_on_directories(false)` matters: left on, ServeDir
    // answers `/` with index.html straight off disk and the rewrite below never
    // runs. Off, `/` misses and falls through to the handler like any other
    // client-side route.
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

/// A file under `assets/` is named after a hash of what it holds, so a build
/// that changes it gives it another name, and a browser may keep one as long
/// as it likes rather than asking for every chunk again on every load. The
/// page names them and is never kept (`attach_frontend`), and a miss there,
/// answered with the page, keeps the page's own `no-cache`.
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

/// Every text file the frontend build writes is UTF-8, and `ServeDir` names
/// the type without its charset. A browser left to guess may read
/// `third-party-notices.txt` in a legacy encoding and garble every author's
/// name outside ASCII.
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

/// The page with `payload`, the `/localization` answer, before its `</head>`,
/// where `frontend/src/main.ts` reads it before anything renders: the first
/// paint asks for nothing more, and a dictionary that cannot be fetched is
/// never a blank page followed by raw keys. Every `<` is escaped, so no string
/// can close the element and run what follows. Never executed, the element is
/// no script for the CSP to refuse.
pub(crate) fn with_dictionary(page: &str, payload: &impl serde::Serialize) -> String {
    let json = serde_json::to_string(payload).unwrap_or_default().replace('<', "\\u003c");
    match page.split_once("</head>") {
        Some((head, tail)) => format!(
            "{head}<script type=\"application/json\" id=\"dictionary\">{json}</script>\n</head>{tail}"
        ),
        None => page.to_string(),
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
