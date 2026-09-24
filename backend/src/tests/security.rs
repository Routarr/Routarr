//! Dynamic penetration tests against the real router.
//!
//! Every case drives the assembled `Router` — middleware, extractors and
//! handlers included — through `oneshot`, exactly as a remote client would. The
//! point is not to test a function in isolation but to prove the *deployed*
//! surface holds: that authentication cannot be walked around, that user text
//! is data and never SQL, that a guessed webhook token fails closed, and that
//! no response ever carries a decrypted secret.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;

use super::TestApp;

/// Send with an arbitrary `X-Api-Key`, returning the status.
async fn get_with_key(app: &TestApp, path: &str, key: &str) -> StatusCode {
    app.send(Request::get(path).header("x-api-key", key).body(Body::empty()).unwrap()).await.status
}

/// Log in and return the cookie, for the tests that need a live session.
async fn open_session(app: &TestApp) -> String {
    use axum::http::header;
    use tower::ServiceExt;
    let login = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "username": "admin", "password": generated_password(app) })
                .to_string(),
        ))
        .unwrap();
    let response = app.router.clone().oneshot(login).await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    cookie.split(';').next().unwrap().to_string()
}

/// The same, for the two routes that change the key and are themselves behind
/// it: `TestApp::post` sends no credential, so a mode that demands one would
/// answer 401 and the test would prove nothing about the handler.
async fn post_with_key(app: &TestApp, path: &str, key: &str) -> super::TestResponse {
    app.send(
        Request::post(path)
            .header("x-api-key", key)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await
}

async fn delete_with_key(app: &TestApp, path: &str, key: &str) -> super::TestResponse {
    app.send(Request::delete(path).header("x-api-key", key).body(Body::empty()).unwrap()).await
}

// ----------------------------------------------------- authentication

/// Every route the API declares, read out of `main.rs` itself.
///
/// Both routers, not only `protected`: reading that one alone, a route *moved*
/// into `public` leaves the enumeration with it and the walk below never
/// notices. What has to be pinned is the small set that answers without a key,
/// so the check is "everything refuses except these", and moving a route out of
/// the protected block fails here until somebody adds it to the list on
/// purpose — which is the review this exists to force.
///
/// A hand-written list of probes is the shape this repository has condemned
/// twice: the e2e specs are selected by tag and the modal sweep is checked
/// against the files rendering a `<Modal>`, both because a list kept by hand
/// goes stale in silence.
fn declared_routes() -> Vec<(&'static str, String)> {
    // Compile-time, so the test cannot pass by reading nothing: a path that
    // does not resolve fails the build rather than the assertion.
    const MAIN: &str = include_str!("../main.rs");

    let block = MAIN
        .split_once("let public = Router::new()")
        .expect("main.rs declares a `public` router")
        .1;
    // The webhook router authenticates by a token in the path, which is a
    // different mechanism with its own tests.
    let block = block.split_once("let webhooks").expect("`webhooks` follows the two").0;

    let mut routes = Vec::new();
    for line in block.lines() {
        let Some(rest) = line.trim().strip_prefix(".route(\"") else { continue };
        let Some((path, handlers)) = rest.split_once('"') else { continue };
        for (needle, method) in [
            ("get(", "GET"),
            ("post(", "POST"),
            ("put(", "PUT"),
            ("delete(", "DELETE"),
            ("patch(", "PATCH"),
        ] {
            if handlers.contains(needle) {
                // `{id}` is a real segment to axum; any value routes the same,
                // and none of these is reached without a key anyway.
                routes.push((method, format!("/api/v1{}", path.replace("{id}", "probe"))));
            }
        }
    }
    routes
}

/// What answers without a key, and why.
///
/// `/ping` is the liveness probe. The shell needs its strings before it can
/// render an authentication error, and it needs to know which gate to show
/// before it has anything to show it with. The OIDC pair is outside by
/// necessity: the browser has no session on the way out and the provider's
/// redirect carries none on the way back.
const OPEN: &[(&str, &str)] = &[
    ("GET", "/api/v1/ping"),
    ("GET", "/api/v1/localization"),
    ("GET", "/api/v1/localization/languages"),
    ("GET", "/api/v1/auth/mode"),
    ("POST", "/api/v1/auth/login"),
    ("POST", "/api/v1/auth/logout"),
    ("GET", "/api/v1/auth/oidc/start"),
    ("GET", "/api/v1/auth/oidc/callback"),
];

/// The whole point of the API key is that *every* library-touching route is
/// behind it. A route reachable without one would be the worst kind of
/// regression, whichever router it was declared in.
#[tokio::test]
async fn every_route_but_the_declared_few_refuses_a_request_with_no_key() {
    let app = TestApp::with_api_key("s3cret").await;
    let routes = declared_routes();

    // The counts are the guard on the guard: a parser that stops matching
    // returns an empty list, and every assertion below then passes without
    // probing anything at all.
    assert!(routes.len() > 40, "only {} route(s) parsed out of main.rs", routes.len());
    assert!(
        routes.iter().filter(|(method, _)| *method != "GET").count() > 10,
        "too few write verbs among {} route(s): the walk is not representative",
        routes.len()
    );

    for (method, path) in routes {
        if OPEN.contains(&(method, path.as_str())) {
            continue;
        }
        let request = Request::builder()
            .method(method)
            .uri(&path)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let status = app.send(request).await.status;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path} was reachable without a key");
    }
}

/// And the open few are open: a list nothing checks would let one of them be
/// quietly closed, which locks the shell out of the screen that says why.
#[tokio::test]
async fn the_open_routes_answer_without_a_key() {
    let app = TestApp::with_api_key("s3cret").await;

    for (method, path) in OPEN {
        let request = Request::builder()
            .method(*method)
            .uri(*path)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let status = app.send(request).await.status;
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{method} {path} needs a key it cannot have");
    }
}

/// The keys are compared in constant time, but the comparison must still reject
/// every near-miss a brute-force or a copy-paste error would produce.
#[tokio::test]
async fn no_near_miss_key_is_accepted() {
    let app = TestApp::with_api_key("s3cret").await;

    for wrong in [
        "",        // empty
        " ",       // whitespace
        "s3cre",   // prefix
        "s3cret ", // trailing space (trimmed, so this actually matches — see below)
        "s3crets", // suffix
        "S3CRET",  // wrong case
        "s3cret.", // trailing punctuation
        "0s3cret", // leading digit
    ] {
        let status = get_with_key(&app, "/api/v1/settings", wrong).await;
        // "s3cret " trims to "s3cret" and is a legitimate accept; every other
        // entry must be rejected.
        if wrong.trim() == "s3cret" {
            assert_eq!(status, StatusCode::OK, "{wrong:?} should trim to a match");
        } else {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{wrong:?} was wrongly accepted");
        }
    }
}

/// A `Bearer` token is accepted, but only under that exact scheme — `Basic`,
/// a bare token, or a lowercased scheme must not slip through.
#[tokio::test]
async fn only_the_bearer_scheme_is_honoured() {
    let app = TestApp::with_api_key("s3cret").await;

    let cases = [
        ("Bearer s3cret", StatusCode::OK),
        ("bearer s3cret", StatusCode::UNAUTHORIZED),
        ("Basic s3cret", StatusCode::UNAUTHORIZED),
        ("s3cret", StatusCode::UNAUTHORIZED),
        ("Bearer  s3cret", StatusCode::OK), // extra space is trimmed
    ];

    for (header, expected) in cases {
        let request = Request::get("/api/v1/settings")
            .header("authorization", header)
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.send(request).await.status, expected, "authorization: {header:?}");
    }
}

/// The header name is case-insensitive per HTTP, and the middleware must honour
/// that or a proxy that normalises casing would lock everyone out.
#[tokio::test]
async fn the_key_header_name_is_case_insensitive() {
    let app = TestApp::with_api_key("s3cret").await;
    for name in ["x-api-key", "X-Api-Key", "X-API-KEY"] {
        let request =
            Request::get("/api/v1/settings").header(name, "s3cret").body(Body::empty()).unwrap();
        assert_eq!(app.send(request).await.status, StatusCode::OK, "header {name}");
    }
}

// -------------------------------------------------------- injection

/// The search term flows into a `LIKE ? ESCAPE '\'`. A classic injection, a
/// wildcard, and the escape character itself must all be treated as data: the
/// query must succeed and match literally, never error and never widen.
#[tokio::test]
async fn the_search_parameter_cannot_inject_or_widen() {
    let app = TestApp::new().await;
    app.seed_library().await; // one movie: "My Neighbor Totoro"

    let hostile = [
        "'; DROP TABLE media;--",
        "' OR '1'='1",
        "%",  // LIKE wildcard: must not match everything
        "_",  // single-char wildcard
        "\\", // the escape character
        "Totoro' UNION SELECT * FROM instances--",
    ];

    for term in hostile {
        let encoded = urlencode(term);
        let response = app.get(&format!("/api/v1/media?search={encoded}")).await;
        response.assert_ok();
        // The table is intact and nothing was dropped: a follow-up query works.
        let count = response.json["pagination"]["total"].as_i64().unwrap();
        assert_eq!(count, 0, "{term:?} matched something it should not have");
    }

    // And the table really still exists and still holds its row.
    let all = app.get("/api/v1/media").await;
    assert_eq!(all.json["pagination"]["total"], 1);
}

/// A category name is compared by value against rules and root folders, so it
/// is the kind of field an injection would target. The handler does better than
/// bind it: an allowlist refuses anything but letters, digits, `-` and `_`, so
/// the hostile string never reaches storage in the first place.
#[tokio::test]
async fn a_hostile_category_name_is_refused_by_the_allowlist() {
    let app = TestApp::new().await;

    for name in [
        "anime'); DROP TABLE categories;--",
        "a b", // a space is not on the allowlist
        "a/b",
        "a\'b",
        "../etc",
    ] {
        let created = app.post("/api/v1/categories", serde_json::json!({ "name": name })).await;
        assert_eq!(created.status, StatusCode::BAD_REQUEST, "{name:?} should be refused");
    }

    // No partial row was written: none of the hostile names is present.
    let list = app.get("/api/v1/categories").await;
    let names: Vec<String> = list
        .json
        .as_array()
        .or(list.json["data"].as_array())
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        !names.iter().any(|n| n.contains("drop") || n.contains('/') || n.contains(' ')),
        "a hostile category name reached storage: {names:?}"
    );
}

/// A rule's condition values are JSON, stored and read back. A value crafted to
/// break out of the JSON or the SQL underneath must survive as an ordinary
/// string.
#[tokio::test]
async fn rule_condition_values_are_inert() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let rule = serde_json::json!({
        "name": "x'); DROP TABLE rules;--",
        "priority": 10,
        "media_type": "movie",
        "match_mode": "all",
        "target_category": "anime",
        "conditions": [{ "type": "genre_contains", "value": ["'; DELETE FROM media;--"] }],
        "exclusions": []
    });
    app.post("/api/v1/rules", rule).await.assert_ok();

    // Both tables are intact.
    assert_eq!(app.get("/api/v1/rules").await.json.as_array().map(|a| a.len()), Some(1));
    assert_eq!(app.get("/api/v1/media").await.json["pagination"]["total"], 1);
}

// ------------------------------------------------------ secret leakage

/// An instance's API key is encrypted at rest and must never appear in any
/// response — not the list, not the detail, not an error.
#[tokio::test]
async fn an_arr_key_never_appears_in_a_response() {
    let app = TestApp::new().await;
    let secret = "super-secret-arr-key-8f3a";

    app.post(
        "/api/v1/instances",
        serde_json::json!({
            "name": "Radarr", "instance_type": "radarr",
            "base_url": "http://radarr:7878", "api_key": secret,
            "enabled": true, "sync_interval_minutes": 15
        }),
    )
    .await
    .assert_ok();

    // Neither the list, the detail, nor a config export may carry it.
    for path in ["/api/v1/instances", "/api/v1/config/export"] {
        let body = app.text(path).await;
        assert!(!body.contains(secret), "the Arr key leaked through {path}");
    }
}

/// A stored secret is sealed, so even direct database inspection shows
/// ciphertext — the property the whole `crypto` module exists to guarantee, held
/// end to end through the create handler.
#[tokio::test]
async fn a_stored_arr_key_is_ciphertext_not_plaintext() {
    let app = TestApp::new().await;
    let secret = "plaintext-should-never-persist";

    app.post(
        "/api/v1/instances",
        serde_json::json!({
            "name": "Radarr", "instance_type": "radarr",
            "base_url": "http://radarr:7878", "api_key": secret,
            "enabled": true, "sync_interval_minutes": 15
        }),
    )
    .await
    .assert_ok();

    let stored: String = sqlx::query_scalar("SELECT api_key FROM instances")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("enc:v1:"), "the key was not sealed");
    assert!(!stored.contains(secret), "plaintext survived in the column");
}

// -------------------------------------------------------- body limit

/// The 2 MiB body cap stops an unauthenticated party from making the server
/// buffer unbounded input. Above the cap the request is refused, not OOM'd.
#[tokio::test]
async fn an_oversized_body_is_refused() {
    let app = TestApp::new().await;

    let huge = serde_json::json!({ "settings": { "x": "A".repeat(3 * 1024 * 1024) } });
    let body = serde_json::to_vec(&huge).unwrap();
    let request = Request::put("/api/v1/settings")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let status = app.send(request).await.status;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

// ------------------------------------------------------- modes

/// The value this setting shipped with. An image people pull without reading
/// release notes has to keep starting, and starting *open* is the one outcome
/// a typo must never produce.
#[test]
fn the_old_spelling_still_opts_out_and_a_typo_never_does() {
    use crate::config::AuthMode;

    assert_eq!(AuthMode::from_env("none"), AuthMode::None);
    assert_eq!(AuthMode::from_env("disabled"), AuthMode::None);
    assert_eq!(AuthMode::from_env("DISABLED"), AuthMode::None);

    assert_eq!(AuthMode::from_env("required"), AuthMode::ApiKey);
    assert_eq!(AuthMode::from_env("apikey"), AuthMode::ApiKey);
    assert_eq!(AuthMode::from_env(""), AuthMode::ApiKey);
    // A typo closes the door rather than opening it.
    assert_eq!(AuthMode::from_env("nome"), AuthMode::ApiKey);
    assert_eq!(AuthMode::from_env("off"), AuthMode::ApiKey);
}

/// Radarr wires its own `External` to the same handler as `None`: it reads no
/// identity header, so there is none to forge. What it asks in exchange is
/// that nothing reach the port except through the proxy, which is why it says
/// so at startup and in the diagnostics.
#[tokio::test]
async fn external_asks_for_nothing_and_says_so() {
    use crate::config::AuthMode;

    assert_eq!(AuthMode::from_env("external"), AuthMode::External);
    assert_eq!(AuthMode::from_env("proxy"), AuthMode::External);

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = AuthMode::External;
    // A key is present and still not demanded: the mode decides, not the key.
    config.api_key = Some("s3cret".into());

    let state = crate::state::AppState::for_tests().await.with_config(config);
    let app = TestApp { router: crate::build_router(state.clone()), state };

    app.get("/api/v1/status").await.assert_ok();

    let warnings = app.get("/api/v1/status").await.assert_ok()["warnings"].to_string();
    assert!(warnings.contains("reverse proxy"), "{warnings}");
}

// ------------------------------------------------------- forms

/// Each one gets its own data directory: the generated password is written to
/// `data_dir`, and two of these sharing one would each overwrite the other's
/// file while keeping their own account, so whichever read second would read a
/// password that opens nothing.
async fn forms_app(label: &str) -> (TestApp, super::TempDir) {
    use crate::config::AuthMode;

    let dir = super::TempDir(
        std::env::temp_dir().join(format!("routarr-forms-{label}-{}", std::process::id())),
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = AuthMode::Forms;
    config.data_dir = dir.to_path_buf();

    let state = crate::state::AppState::for_tests().await.with_config(config);
    crate::services::accounts::ensure_account(&state.pool, &state.config.password_path())
        .await
        .unwrap();
    // The generated password is written beside the database; the file is the
    // only copy a test can read, as it is for an operator who missed the log.
    (TestApp { router: crate::build_router(state.clone()), state }, dir)
}

fn generated_password(app: &TestApp) -> String {
    std::fs::read_to_string(app.state.config.password_path()).unwrap()
}

/// Sign in and return the session cookie, as a browser would carry it.
async fn sign_in(app: &TestApp, password: &str) -> String {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let body = serde_json::json!({ "username": "admin", "password": password });
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "sign-in failed");
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

/// A request carrying the session, since `TestApp` sends no cookies.
async fn with_session(
    app: &TestApp,
    method: &str,
    path: &str,
    cookie: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn forms_refuses_until_a_session_is_opened() {
    let (app, _dir) = forms_app("refuses").await;
    app.get("/api/v1/status").await.assert_status(StatusCode::UNAUTHORIZED);

    let bad = app
        .post(
            "/api/v1/auth/login",
            serde_json::json!({ "username": "admin", "password": "not it" }),
        )
        .await;
    bad.assert_status(StatusCode::UNAUTHORIZED);
    // One message for a wrong name and a wrong password alike.
    assert_eq!(bad.message(), "Wrong username or password.");

    let unknown = app
        .post(
            "/api/v1/auth/login",
            serde_json::json!({ "username": "root", "password": generated_password(&app) }),
        )
        .await;
    assert_eq!(unknown.message(), "Wrong username or password.");
}

/// A password shorter than the minimum cannot be the stored one, so hashing it
/// would spend 355 ms proving what its length already says. The test writes a
/// short password's hash straight into the table: refused anyway is what shows
/// the check was skipped rather than merely failed.
#[tokio::test]
async fn a_password_too_short_to_have_been_set_is_refused_without_hashing() {
    let (app, _dir) = forms_app("short").await;

    let hash = crate::services::accounts::hash_password("short").unwrap();
    sqlx::query("UPDATE users SET password_hash = ?")
        .bind(&hash)
        .execute(&app.state.pool)
        .await
        .unwrap();

    // It *is* the stored password, and it is still refused.
    app.post("/api/v1/auth/login", serde_json::json!({ "username": "admin", "password": "short" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// A flood of simultaneous sign-ins must not take the application down with it,
/// and must not stop the operator signing in either. Unbounded, twenty at once
/// ran twenty argon2 hashes in parallel — a core and about 19 MiB each — and a
/// lockout would not have bounded them, since the failures it counts are
/// recorded after the hashes they were meant to prevent.
#[tokio::test]
async fn a_burst_of_sign_ins_is_bounded_and_still_lets_the_password_through() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let (app, _dir) = forms_app("burst").await;
    let password = generated_password(&app);

    let attempt = |body: String| {
        let router = app.router.clone();
        async move {
            router
                .oneshot(
                    Request::post("/api/v1/auth/login")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };

    let wrong =
        serde_json::json!({ "username": "admin", "password": "not the password" }).to_string();
    let statuses = futures::future::join_all((0..24).map(|_| attempt(wrong.clone()))).await;

    // Every one is answered: refused, or told to come back in a moment. None is
    // dropped, and none locks anybody out.
    for status in &statuses {
        assert!(
            *status == StatusCode::UNAUTHORIZED || *status == StatusCode::SERVICE_UNAVAILABLE,
            "unexpected {status} under load"
        );
    }

    // And the operator still gets in — the property a lockout gives away.
    let right = serde_json::json!({ "username": "admin", "password": password }).to_string();
    assert_eq!(attempt(right).await, StatusCode::OK, "the burst locked the account out");
}

/// The cookie has to carry the three attributes that make it survivable: out of
/// reach of script, refused on a cross-site form, and scoped to the mount point
/// rather than to every application behind the same proxy.
#[tokio::test]
async fn a_session_cookie_is_httponly_lax_and_scoped() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let (app, _dir) = forms_app("cookie").await;
    let body = serde_json::json!({ "username": "admin", "password": generated_password(&app) });
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(cookie.starts_with("routarr_session="), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
}

/// An operator who moves from `forms` to `oidc` does so to change who may enter,
/// and the database — sessions included — survives the restart. A session the
/// old mode opened and the new one still honours is the old door left open,
/// sliding for as long as someone keeps using it.
#[tokio::test]
async fn a_session_is_honoured_only_by_the_mode_that_opened_it() {
    use crate::config::AuthMode;

    let (forms, _dir) = forms_app("mode-change").await;
    let cookie = sign_in(&forms, &generated_password(&forms)).await;
    let (status, _) =
        with_session(&forms, "GET", "/api/v1/status", &cookie, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "the session must work under the mode that opened it");

    // The same database after a restart with ROUTARR_AUTH=oidc.
    let mut config = (*forms.state.config).clone();
    config.auth_mode = AuthMode::Oidc;
    let state = forms.state.clone().with_config(config);
    let oidc = TestApp { router: crate::build_router(state.clone()), state };

    let (status, _) =
        with_session(&oidc, "GET", "/api/v1/status", &cookie, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a forms session got through under oidc");

    // And the other way: a provider's session presented once the operator has
    // gone back to the local account.
    sqlx::query(
        "INSERT INTO sessions (id, subject, source, expires_at)
         VALUES ('from-the-provider', 'someone', 'oidc', datetime('now', '+1 day'))",
    )
    .execute(&oidc.state.pool)
    .await
    .unwrap();
    let provider_cookie = "routarr_session=from-the-provider";
    let (status, _) =
        with_session(&oidc, "GET", "/api/v1/status", provider_cookie, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "the provider's session must work under oidc");
    let (status, _) =
        with_session(&forms, "GET", "/api/v1/status", provider_cookie, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "an oidc session got through under forms");
}

/// A cookie travels on a cross-site request whether or not the page meant to
/// send it. SameSite refuses the dangerous shapes and the JSON extractor
/// refuses a form's content type; this is the third of the three.
#[tokio::test]
async fn a_write_from_another_origin_is_refused_even_with_the_cookie() {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    let (app, _dir) = forms_app("origin").await;
    let body = serde_json::json!({ "username": "admin", "password": generated_password(&app) });
    let login = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.router.clone().oneshot(login).await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    let session = cookie.split(';').next().unwrap().to_string();

    // The same cookie, read: allowed, because a read forges nothing.
    let read = Request::get("/api/v1/status")
        .header(header::COOKIE, &session)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::HOST, "routarr.local")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.router.clone().oneshot(read).await.unwrap().status(), StatusCode::OK);

    // The same cookie, writing, from somewhere else: refused.
    let write = Request::post("/api/v1/simulate")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &session)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::HOST, "routarr.local")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(app.router.clone().oneshot(write).await.unwrap().status(), StatusCode::FORBIDDEN);
}

/// Sends a write with the cookie and the given browser and proxy headers.
async fn write_from(
    app: &TestApp,
    session: &str,
    origin: &str,
    host: &str,
    forwarded_host: Option<&str>,
) -> StatusCode {
    use axum::http::header;
    let mut request = Request::post("/api/v1/simulate")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, session)
        .header(header::ORIGIN, origin)
        .header(header::HOST, host);
    if let Some(public) = forwarded_host {
        request = request.header("x-forwarded-host", public);
    }
    app.send(request.body(Body::from("{}")).unwrap()).await.status
}

/// The mirror of the refusal above, without which "every write with an Origin
/// is refused" would pass it. A browser on this application says so through
/// `Origin`, and the write goes through — including behind the proxies people
/// actually run: nginx sends the upstream's name as `Host` and the public one
/// as `X-Forwarded-Host`, and a browser omits the default port that a proxy
/// may write out.
#[tokio::test]
async fn a_write_from_this_origin_goes_through_with_the_cookie() {
    let (app, _dir) = forms_app("origin-ok").await;
    let session = open_session(&app).await;

    for (origin, host, forwarded) in [
        ("http://routarr.local", "routarr.local", None),
        ("https://routarr.example.com", "routarr.example.com", None),
        ("https://routarr.example.com", "routarr:9876", Some("routarr.example.com")),
        ("https://routarr.example.com", "routarr.example.com:443", None),
        ("http://routarr.local:80", "routarr.local", None),
        ("http://[::1]:9876", "[::1]:9876", None),
    ] {
        let status = write_from(&app, &session, origin, host, forwarded).await;
        assert_eq!(status, StatusCode::OK, "{origin} for {host} ({forwarded:?}) was refused");
    }
}

/// A proxy header does not widen anything: the origin still has to be the
/// public host, an opaque origin names nobody, and a port is part of it.
#[tokio::test]
async fn a_foreign_or_opaque_origin_is_refused_through_a_proxy_too() {
    let (app, _dir) = forms_app("origin-proxy").await;
    let session = open_session(&app).await;

    for (origin, host, forwarded) in [
        ("null", "routarr.local", None),
        ("https://evil.example", "routarr:9876", Some("routarr.example.com")),
        ("https://routarr.example.com:8443", "routarr.example.com", None),
        ("https://routarr.example.com.evil.example", "routarr.example.com", None),
    ] {
        let status = write_from(&app, &session, origin, host, forwarded).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{origin} for {host} ({forwarded:?}) went through"
        );
    }
}

/// Behind a proxy that terminates TLS, a cookie without `Secure` also travels
/// on a plain `http://` request to the same host. The proxy says which scheme
/// the browser used through `X-Forwarded-Proto`, and the cookie follows it —
/// on the way in and on the way out, since a browser will not let an insecure
/// response clear a secure cookie.
#[tokio::test]
async fn the_session_cookie_is_secure_when_the_browser_came_over_https() {
    use axum::http::header;
    use tower::ServiceExt;

    let (app, _dir) = forms_app("secure").await;
    let body = serde_json::json!({ "username": "admin", "password": generated_password(&app) });

    let plain = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.router.clone().oneshot(plain).await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(!cookie.contains("Secure"), "a plain http sign-in got a Secure cookie: {cookie}");

    let tls = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-proto", "https")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.router.clone().oneshot(tls).await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(
        cookie.contains("; Secure"),
        "a sign-in over https got a cookie without Secure: {cookie}"
    );
    let session = cookie.split(';').next().unwrap().to_string();

    let logout = Request::post("/api/v1/auth/logout")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &session)
        .header("x-forwarded-proto", "https")
        .body(Body::from("{}"))
        .unwrap();
    let response = app.router.clone().oneshot(logout).await.unwrap();
    let cleared = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(cleared.contains("; Secure"), "the clearing cookie lost Secure: {cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

/// A decision has to name whoever asked for it, or "who moved my files" has no
/// answer past "somebody, manually" — which is the answer a session mode exists
/// to improve on.
#[tokio::test]
async fn a_decision_names_the_account_that_asked_for_it() {
    use axum::http::header;

    let (app, _dir) = forms_app("attribution").await;
    app.seed_library().await;

    let session = open_session(&app).await;

    let simulate = Request::post("/api/v1/simulate")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &session)
        .body(Body::from(r#"{"persist": true, "persist_unchanged": true}"#))
        .unwrap();
    app.send(simulate).await.assert_status(StatusCode::OK);

    let stored: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT actor, subject FROM decisions")
            .fetch_all(&app.state.pool)
            .await
            .unwrap();
    assert!(!stored.is_empty(), "the run stored nothing to attribute");
    for (actor, subject) in stored {
        assert_eq!(actor, "manual", "the trigger still says what caused the run");
        assert_eq!(subject.as_deref(), Some("admin"), "the account is not recorded");
    }
}

/// The other half of the same contract: a mode where everybody shares one
/// anonymous subject must store no name at all. A word that names nobody reads,
/// on the History screen, exactly like an attribution.
#[tokio::test]
async fn a_mode_that_names_nobody_stores_nobody() {
    let app = TestApp::new().await;
    app.seed_library().await;

    app.post("/api/v1/simulate", serde_json::json!({ "persist": true, "persist_unchanged": true }))
        .await
        .assert_ok();

    let subjects: Vec<Option<String>> = sqlx::query_scalar("SELECT subject FROM decisions")
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert!(!subjects.is_empty(), "the run stored nothing to attribute");
    assert!(subjects.iter().all(Option::is_none), "an anonymous caller was recorded as one");
}

/// A machine client cannot hold a cookie, so the key keeps working beside the
/// session — which is also how Servarr separates the two.
#[tokio::test]
async fn the_api_key_still_opens_the_door_in_forms_mode() {
    use crate::config::AuthMode;

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = AuthMode::Forms;
    config.api_key = Some("s3cret".into());
    let state = crate::state::AppState::for_tests().await.with_config(config);
    let app = TestApp { router: crate::build_router(state.clone()), state };

    assert_eq!(get_with_key(&app, "/api/v1/status", "s3cret").await, StatusCode::OK);
    assert_eq!(get_with_key(&app, "/api/v1/status", "wrong").await, StatusCode::UNAUTHORIZED);
}

/// A generated password that can never be changed is not a credential anybody
/// can live with. Proving the current one matters even behind a session: a
/// browser left open on a shared machine is the case this exists for.
#[tokio::test]
async fn the_password_changes_only_against_the_current_one() {
    let (app, _dir) = forms_app("password").await;
    let current = generated_password(&app);
    let session = sign_in(&app, &current).await;

    let (status, _) = with_session(
        &app,
        "PUT",
        "/api/v1/auth/password",
        &session,
        serde_json::json!({ "current": "not it", "new_password": "a much longer one" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a wrong current password was accepted");

    let (status, _) = with_session(
        &app,
        "PUT",
        "/api/v1/auth/password",
        &session,
        serde_json::json!({ "current": current.clone(), "new_password": "short" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a five-character password was accepted");

    let (status, _) = with_session(
        &app,
        "PUT",
        "/api/v1/auth/password",
        &session,
        serde_json::json!({ "current": current.clone(), "new_password": "a much longer one" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The session the old password opened is gone with it.
    let (status, _) =
        with_session(&app, "GET", "/api/v1/status", &session, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old session survived the change");

    // The old password no longer opens anything, the new one does.
    app.post("/api/v1/auth/login", serde_json::json!({ "username": "admin", "password": current }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.post(
        "/api/v1/auth/login",
        serde_json::json!({ "username": "admin", "password": "a much longer one" }),
    )
    .await
    .assert_ok();
}

/// The shell has to learn which gate to show before it can show one.
#[tokio::test]
async fn the_mode_is_readable_without_a_session() {
    let (app, _dir) = forms_app("mode").await;
    assert_eq!(app.get("/api/v1/auth/mode").await.assert_ok()["mode"], "forms");
}

/// In `forms` and `oidc` a key exists only if somebody set one, and the screen
/// that offers to store one has no other way to know. Answering `true` where
/// there is nothing to present is what puts a dead field on that screen.
#[tokio::test]
async fn the_mode_says_whether_a_key_exists_at_all() {
    let (app, _dir) = forms_app("mode-key").await;
    assert_eq!(app.get("/api/v1/auth/mode").await.assert_ok()["api_key_configured"], false);

    let mut config = (*app.state.config).clone();
    config.api_key = Some("k".into());
    let state = app.state.clone().with_config(config);
    let with_key = TestApp { router: crate::build_router(state.clone()), state };
    let response = with_key.get("/api/v1/auth/mode").await;
    let mode = response.assert_ok();
    assert_eq!(mode["api_key_configured"], true);
    // Set through the environment, so the interface must not offer to replace
    // it: the new one would last exactly until the next restart.
    assert_eq!(mode["api_key_pinned"], true);
}

// ------------------------------------------------------- panics

/// A handler that panics has to answer.
///
/// Without a catch layer the connection is dropped: no status, no body, and no
/// `X-Request-Id` — so the caller cannot tell a bug from a cut cable, and the
/// one identifier that ties a failure on screen to its line in the log never
/// reaches them. The panic is still logged; what changes is that somebody is
/// told.
///
/// The stack is rebuilt here rather than borrowed from `build_router`, because
/// a `.layer()` wraps only the routes registered before it — a `/boom` added to
/// the assembled router would sit outside every layer and prove nothing. What
/// this pins is the pair that matters: the catch layer *inside* the request-id
/// layers, so the id still reaches a response the handler never produced.
#[tokio::test]
async fn a_panicking_handler_answers_five_hundred_with_its_request_id() {
    use axum::routing::get;
    use tower::ServiceExt;
    use tower_http::catch_panic::CatchPanicLayer;
    use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};

    // A declared return type, or the handler's response type is `!` and axum
    // cannot infer one.
    async fn boom() -> String {
        panic!("a handler that panics is a bug, not a disconnection")
    }

    let router = axum::Router::new()
        .route("/boom", get(boom))
        .layer(CatchPanicLayer::custom(crate::panic_response))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid));

    let response = router
        .oneshot(Request::get("/boom").body(Body::empty()).unwrap())
        .await
        .expect("the connection must survive the panic");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        response.headers().contains_key("x-request-id"),
        "the id the log recorded has to reach the caller"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("a JSON body like any other error");
    assert_eq!(json["error"], "internal");
}

// --------------------------------------------------- rotation

/// An `apikey` installation whose key is the stored one rather than a pinned
/// variable — which is the ordinary case, and the only one that can rotate.
async fn stored_key_app(label: &str, key: &str) -> (TestApp, super::TempDir) {
    let dir = super::TempDir(
        std::env::temp_dir().join(format!("routarr-key-{label}-{}", std::process::id())),
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = crate::config::AuthMode::ApiKey;
    config.data_dir = dir.to_path_buf();
    crate::crypto::write_api_key(&config.api_key_path(), key).unwrap();

    let state = crate::state::AppState::for_tests().await.with_config(config);
    (TestApp { router: crate::build_router(state.clone()), state }, dir)
}

/// A key that cannot be replaced without stopping the service is a key that
/// stays valid for as long as it takes somebody to notice it leaked.
#[tokio::test]
async fn rotating_refuses_the_old_key_from_the_very_next_request() {
    let (app, _dir) = stored_key_app("rotate", "first").await;
    assert_eq!(get_with_key(&app, "/api/v1/status", "first").await, StatusCode::OK);

    let response = post_with_key(&app, "/api/v1/auth/api-key", "first").await;
    let body = response.assert_ok();
    let minted = body["api_key"].as_str().expect("the new key is returned once").to_string();
    assert_ne!(minted, "first");

    // No restart in between: the router and the state are the same objects.
    assert_eq!(get_with_key(&app, "/api/v1/status", "first").await, StatusCode::UNAUTHORIZED);
    assert_eq!(get_with_key(&app, "/api/v1/status", &minted).await, StatusCode::OK);
}

/// It has to survive the restart too, or the rotation is undone by the next
/// deploy and the old key comes back. And a session mode is where minting one
/// is a *feature*: a script gets a credential without being given the password.
#[tokio::test]
async fn a_key_minted_in_a_session_mode_is_the_one_on_disk() {
    use axum::http::header;

    let (app, _dir) = forms_app("mint").await;
    let session = open_session(&app).await;

    // Nothing to present yet: the mode does not generate one.
    assert_eq!(crate::crypto::read_api_key(&app.state.config.api_key_path()), None);

    let minted = app
        .send(
            Request::post("/api/v1/auth/api-key")
                .header(header::COOKIE, &session)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .assert_ok()["api_key"]
        .as_str()
        .unwrap()
        .to_string();

    let on_disk = crate::crypto::read_api_key(&app.state.config.api_key_path());
    assert_eq!(on_disk.as_deref(), Some(minted.as_str()), "the file holds another key");
    assert_eq!(get_with_key(&app, "/api/v1/status", &minted).await, StatusCode::OK);

    // And withdrawing it takes the file with it, since the session remains.
    let removed = app
        .send(
            Request::delete("/api/v1/auth/api-key")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    removed.assert_status(StatusCode::NO_CONTENT);
    assert_eq!(crate::crypto::read_api_key(&app.state.config.api_key_path()), None);
    assert_eq!(get_with_key(&app, "/api/v1/status", &minted).await, StatusCode::UNAUTHORIZED);
}

/// Removing the only credential locks everybody out, including whoever would
/// put it back. The refusal is the guardrail.
#[tokio::test]
async fn the_key_cannot_be_withdrawn_when_it_is_the_only_way_in() {
    let (app, _dir) = stored_key_app("withdraw", "only").await;

    let refused = delete_with_key(&app, "/api/v1/auth/api-key", "only").await;
    refused.assert_status(StatusCode::CONFLICT);
    assert_eq!(get_with_key(&app, "/api/v1/status", "only").await, StatusCode::OK);
}

/// A key the environment states cannot be changed here: the replacement would
/// live until the next restart and then be silently overwritten again.
#[tokio::test]
async fn a_key_the_environment_pins_is_not_rotated_here() {
    let app = TestApp::with_api_key("pinned").await;

    let refused = post_with_key(&app, "/api/v1/auth/api-key", "pinned").await;
    refused.assert_status(StatusCode::CONFLICT);
    assert_eq!(get_with_key(&app, "/api/v1/status", "pinned").await, StatusCode::OK);
}

// ------------------------------------------------------- oidc

/// The whole flow against a provider on an ephemeral port: discovery, the
/// authorization URL, the exchange, and the session it opens.
async fn oidc_app(idp: &crate::tests::fake_oidc::FakeOidc) -> TestApp {
    use crate::config::AuthMode;

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = AuthMode::Oidc;
    config.oidc_issuer = Some(idp.issuer.clone());
    config.oidc_client_id = Some("routarr".into());
    config.oidc_client_secret = Some("shhh".into());
    config.oidc_redirect_url = Some("http://routarr.local/api/v1/auth/oidc/callback".into());

    let state = crate::state::AppState::for_tests().await.with_config(config);
    TestApp { router: crate::build_router(state.clone()), state }
}

/// Not verifying the token's signature rests on the exchange happening over
/// TLS, so a provider that describes its token endpoint as plain `http://`
/// pulls that assumption out from under the flow. The issuer is checked at
/// startup; the endpoints are only known once the provider has been asked.
#[tokio::test]
async fn a_provider_whose_endpoints_are_in_the_clear_is_refused() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    idp.advertise_endpoints_at("http://idp.example");
    let app = oidc_app(&idp).await;

    let response = app.get("/api/v1/auth/oidc/start").await;
    // The caller is a browser following the sign-in link. It lands back on the
    // sign-in screen, which says the sign-in did not complete, rather than on
    // an error body, and the fault stays out of what an anonymous caller sees.
    assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.json);
    assert_eq!(response.location().as_deref(), Some("/?signin=failed"));
    // The log line the operator reads names what to fix.
    let message = match crate::services::oidc::start(&app.state).await {
        Ok(_) => String::from("the flow started"),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("token_endpoint") && message.contains("https"),
        "the refusal names the endpoint and the scheme: {message}"
    );
}

/// Start a sign-in as a browser would, and keep what it would keep: the
/// cookie that ties it to this browser, the `state` in the redirect, and the
/// nonce the provider is to echo — read from the row, as the test's stand-in
/// for what the provider learns from the authorization URL.
struct Flow {
    cookie: String,
    state: String,
    nonce: String,
    location: String,
}

async fn start_flow(app: &TestApp) -> Flow {
    use axum::http::header;
    use tower::ServiceExt;

    let request = Request::get("/api/v1/auth/oidc/start").body(Body::empty()).unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "the browser must be sent to the provider"
    );
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    let cookie = cookie.split(';').next().unwrap().to_string();
    let state = cookie.split_once('=').expect("name=value").1.to_string();
    let location = response.headers()[header::LOCATION].to_str().unwrap().to_string();
    let nonce: String = sqlx::query_scalar("SELECT nonce FROM oidc_flows WHERE state = ?")
        .bind(&state)
        .fetch_one(&app.state.pool)
        .await
        .expect("the attempt was recorded");
    Flow { cookie, state, nonce, location }
}

/// Come back from the provider as the browser that left — or as another one.
async fn callback(app: &TestApp, cookie: Option<&str>, path: &str) -> super::TestResponse {
    let mut request = Request::get(path);
    if let Some(cookie) = cookie {
        request = request.header(axum::http::header::COOKIE, cookie);
    }
    app.send(request.body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn a_sign_in_carries_pkce_and_the_nonce_to_the_provider() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;

    let Flow { state, nonce, location, .. } = start_flow(&app).await;
    assert!(location.starts_with(&format!("{}/authorize", idp.issuer)), "{location}");
    assert!(location.contains("code_challenge_method=S256"), "{location}");
    // The challenge, never the verifier: what travels through a browser must
    // not be the secret the exchange proves.
    assert!(!location.contains("code_verifier"), "{location}");
    assert!(location.contains(&format!("state={state}")), "{location}");
    assert!(location.contains(&format!("nonce={nonce}")), "{location}");
}

/// The provider is asked to describe itself once, not once per request.
///
/// `/auth/oidc/start` sits in the *public* router — a browser with no session
/// cannot be asked for one to learn it needs one — so anyone who reaches the
/// port reaches this. Refetching the discovery document per call turns one
/// cheap inbound request into one outbound request against the operator's own
/// identity provider, which rate-limits by address: the flood locks them out
/// of the thing they log in with, from their own host.
///
/// The document is static by specification. `SignInThrottle` already states
/// the rule this route was missing — a public endpoint that costs something
/// carries a bound.
#[tokio::test]
async fn the_provider_is_asked_to_describe_itself_once_however_many_sign_ins_begin() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;

    for _ in 0..12 {
        let response = app.get("/api/v1/auth/oidc/start").await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
    }

    assert_eq!(
        idp.discoveries(),
        1,
        "twelve sign-ins made {} requests to the provider",
        idp.discoveries()
    );
}

/// An unauthenticated flood cannot grow the table without end, and cannot
/// stop the operator signing in either.
///
/// The rows are pruned by the hourly maintenance pass and by nothing else, so
/// a public endpoint that inserts one per call is an hour of traffic on
/// somebody's disk. Bounding it by refusing callers would be worse: it hands
/// an attacker a way to deny sign-in to the one account there is. The oldest
/// attempts go instead, and the browser that just started one is the newest.
#[tokio::test]
async fn a_flood_of_sign_ins_is_bounded_without_denying_the_next_one() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;

    // The one the operator started, before the flood.
    let mine = start_flow(&app).await;
    for _ in 0..80 {
        let response = app.get("/api/v1/auth/oidc/start").await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
    }

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_flows")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert!(rows <= 4096, "the table is not bounded: {rows} rows");

    // Eighty anonymous requests must not have evicted the attempt somebody is
    // answering their provider for: at a bound of sixty-four, they did.
    idp.will_claim(serde_json::json!({ "nonce": mine.nonce, "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", mine.state);
    let response = callback(&app, Some(&mine.cookie), &path).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(
        response.location().as_deref(),
        Some("/"),
        "the operator's attempt was evicted by the flood"
    );
}

/// And finishing one does not ask again either.
///
/// `start` and `finish` both need the endpoints, so an uncached document is two
/// outbound requests per successful login rather than one.
#[tokio::test]
async fn finishing_a_sign_in_reuses_the_description_the_start_already_read() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;

    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce, "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    callback(&app, Some(&flow.cookie), &path).await;

    assert_eq!(idp.discoveries(), 1, "one sign-in cost {} discoveries", idp.discoveries());
}

#[tokio::test]
async fn a_sound_callback_opens_a_session_naming_the_subject() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce, "preferred_username": "alice" }));

    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let response = callback(&app, Some(&flow.cookie), &path).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location().as_deref(), Some("/"), "a success lands on the application");

    let subject: String = sqlx::query_scalar("SELECT subject FROM sessions WHERE source = 'oidc'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(subject, "alice");

    // The client proved the exchange with the verifier and its secret, and the
    // flow row is gone so the code cannot be presented twice.
    let exchange = idp.exchanges().pop().expect("one exchange");
    assert_eq!(exchange.get("client_secret").map(String::as_str), Some("shhh"));
    assert!(exchange.contains_key("code_verifier"), "{exchange:?}");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_flows")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// Each of these is a token that verifies cryptographically and still must not
/// be accepted: the wrong audience, a stale one, and one belonging to another
/// sign-in.
#[tokio::test]
async fn a_token_failing_any_claim_opens_nothing() {
    for bad in [
        serde_json::json!({ "aud": "someone else" }),
        serde_json::json!({ "exp": 1 }),
        serde_json::json!({ "nonce": "another attempt" }),
        serde_json::json!({ "iss": "https://elsewhere.example" }),
    ] {
        let idp = crate::tests::fake_oidc::FakeOidc::start().await;
        let app = oidc_app(&idp).await;
        let flow = start_flow(&app).await;

        let mut claims = serde_json::json!({ "nonce": flow.nonce });
        for (key, value) in bad.as_object().unwrap() {
            claims[key] = value.clone();
        }
        idp.will_claim(claims);

        let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
        let response = callback(&app, Some(&flow.cookie), &path).await;
        assert_eq!(response.location().as_deref(), Some("/?signin=failed"), "{bad} was accepted");
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "{bad} opened a session");
    }
}

/// The callback used to accept any known `code` and `state` pair from whoever
/// presented it: a link carrying somebody else's pair signed the reader in as
/// that somebody — login CSRF, with the attribution on every write theirs.
/// The browser that started the attempt carries its `state` in a cookie, and
/// only that browser may finish it.
#[tokio::test]
async fn a_callback_from_a_browser_that_did_not_start_the_attempt_opens_nothing() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce, "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);

    // No cookie at all, then a cookie from another attempt.
    for foreign in [None, Some("routarr_oidc=some-other-attempt")] {
        let response = callback(&app, foreign, &path).await;
        assert_eq!(response.location().as_deref(), Some("/?signin=failed"), "{foreign:?}");
    }
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "a foreign browser was signed in");

    // The refusals cost the attempt nothing: the browser that started it is
    // still let in, which is also the positive control on the cookie.
    let own = callback(&app, Some(&flow.cookie), &path).await;
    assert_eq!(own.location().as_deref(), Some("/"));
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 1);
}

/// An authorisation code is single-use, and the row is what enforces it.
#[tokio::test]
async fn a_replayed_callback_opens_nothing() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce }));

    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let first = callback(&app, Some(&flow.cookie), &path).await;
    assert_eq!(first.location().as_deref(), Some("/"));
    let again = callback(&app, Some(&flow.cookie), &path).await;
    assert_eq!(again.location().as_deref(), Some("/?signin=failed"));

    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 1, "the replay opened a second session");
}

// ------------------------------------------------------- webhook auth

/// The webhook token is the only guard on the one unauthenticated route. It is
/// compared in constant time; an empty stored token, a wrong token, and a token
/// for the wrong instance must all fail closed as a 404 — which also refuses to
/// confirm whether the instance exists.
#[tokio::test]
async fn a_webhook_token_fails_closed() {
    let app = TestApp::new().await;
    app.seed_library().await; // inst-1, token "tok"

    let body = serde_json::json!({ "eventType": "Download", "movie": { "id": 1 } });
    for (instance, token) in [
        ("inst-1", "wrong"),
        ("inst-1", ""),
        ("inst-1", "TOK"),      // wrong case
        ("inst-1", "tok%20"),   // decodes to "tok " — a trailing space
        ("nonexistent", "tok"), // right token shape, unknown instance
    ] {
        let path = format!("/api/v1/webhook/{instance}/{token}");
        let response = app.post(&path, body.clone()).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{instance}/{token} should fail closed");
        // The same body whether the id or the token is wrong: a different
        // sentence would confirm which instance ids exist. An empty token is
        // a path the route does not match at all, answered by the router.
        if !token.is_empty() {
            assert_eq!(response.message(), "Unknown webhook", "{instance}/{token}");
        }
    }
}

// ------------------------------------------------------------- helpers

/// Percent-encode a query value. Small and dependency-free: only the bytes that
/// actually break a query string are escaped.
fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `text()` on `TestApp` reads a GET body; a couple of tests here want the raw
/// bytes of an authenticated-or-not GET, which `text` already provides.
#[allow(dead_code)]
async fn drain(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A disabled instance is one the user switched off. The scheduler already skips
/// it; the webhook must not be the back door that keeps syncing it, re-routing
/// it and — with automatic application armed — writing to it.
#[tokio::test]
async fn a_webhook_for_a_disabled_instance_changes_nothing() {
    let arr = super::fake_arr::FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    sqlx::query("UPDATE instances SET enabled = 0 WHERE id = 'inst-1'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let body = serde_json::json!({ "eventType": "Download", "movie": { "id": 10 } });
    let response = app.post("/api/v1/webhook/inst-1/tok", body).await;

    // Acknowledged, so the Arr does not retry something it cannot fix — but
    // acted on in no way whatsoever.
    response.assert_ok();
    assert_eq!(response.json["ignored"], "instance is disabled");

    let media: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(media, 0, "a disabled instance was synced through its webhook");
    assert!(arr.recorded().api_keys.is_empty(), "a disabled instance was contacted");
}

// -------------------------------------------------------- response headers

/// The API key lives in the browser's `localStorage`, so an injected script is
/// the one thing that could hand it to someone else. These headers are what
/// stops that being a single point of failure.
#[tokio::test]
async fn every_response_carries_a_content_security_policy() {
    let app = TestApp::new().await;

    let response = app.raw("/api/v1/ping").await;
    let csp = response
        .headers()
        .get("content-security-policy")
        .expect("no CSP on the response")
        .to_str()
        .unwrap()
        .to_string();

    // The directive that actually protects the key.
    assert!(csp.contains("script-src 'self'"), "scripts are not locked to the origin: {csp}");
    // No framing, no plugin objects, no form posting elsewhere.
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("object-src 'none'"));
    assert!(csp.contains("form-action 'self'"));

    assert_eq!(response.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert!(response.headers().get("referrer-policy").is_some());
    // Severs `window.opener` both ways, so a page on either side of an
    // `open()` cannot reach into this one's window.
    assert_eq!(response.headers().get("cross-origin-opener-policy").unwrap(), "same-origin");
}

/// A self-hosted application that fetches anything from a third party tells that
/// third party the address of every homelab running it — and breaks on the
/// air-gapped NAS a Servarr stack often lives on. `connect-src 'self'` is only
/// honest if the interface really makes no outbound request.
#[tokio::test]
async fn the_policy_permits_no_external_origin() {
    let app = TestApp::new().await;
    let response = app.raw("/api/v1/ping").await;
    let csp = response.headers().get("content-security-policy").unwrap().to_str().unwrap();

    assert!(!csp.contains("http://"), "an external origin is allowed: {csp}");
    assert!(!csp.contains("https://"), "an external origin is allowed: {csp}");
}

// ------------------------------------------------------------------ logging

/// Collects what a `tracing` subscriber writes, so a test can read the log a
/// request produced rather than trust that nothing sensitive is in it.
#[derive(Clone, Default)]
struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogCapture {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The token is the only credential of the only unauthenticated route, and the
/// request span is on every line logged while a delivery is served — the error
/// line an operator pastes into a ticket included. With it, anyone reading the
/// log can trigger a sync, an enrichment and a simulation, and a write when
/// auto-apply is armed.
#[tokio::test]
async fn the_webhook_token_never_reaches_the_log() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let app = TestApp::new().await;
    app.seed_library().await;
    let token = "hook-secret-4f9c";
    sqlx::query("UPDATE instances SET webhook_token = ? WHERE id = 'inst-1'")
        .bind(token)
        .execute(&app.state.pool)
        .await
        .unwrap();

    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let dispatch = tracing::Dispatch::new(subscriber);

    let path = format!("/api/v1/webhook/inst-1/{token}");
    // The fixture's Radarr is unreachable, so the delivery fails: that is the
    // error line, and the one worth checking.
    let delivered = app
        .post(&path, serde_json::json!({ "eventType": "Download", "movie": { "id": 10 } }))
        .with_subscriber(dispatch.clone())
        .await;
    assert_eq!(delivered.status, StatusCode::BAD_GATEWAY);
    // A wrong method still names the path it refused.
    let refused = app.get(&path).with_subscriber(dispatch).await;
    assert_eq!(refused.status, StatusCode::METHOD_NOT_ALLOWED);

    let log = capture.contents();
    assert!(
        log.contains("webhook") && log.contains("502"),
        "positive control: the delivery and its failure were logged at all:\n{log}"
    );
    assert!(!log.contains(token), "the webhook token is in the log:\n{log}");
}
