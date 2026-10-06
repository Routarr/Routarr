//! OpenID Connect against a provider on an ephemeral port: discovery, the
//! attempt a browser carries, the exchange, who is let in, and the session
//! it opens.

use axum::body::Body;
use axum::http::{Request, StatusCode};

use super::TestApp;

/// The whole flow against a provider on an ephemeral port: discovery, the
/// authorization URL, the exchange, and the session it opens.
async fn oidc_app(idp: &crate::tests::fake_oidc::FakeOidc) -> TestApp {
    oidc_app_with_secret(idp, "shhh").await
}

async fn oidc_app_with_secret(idp: &crate::tests::fake_oidc::FakeOidc, secret: &str) -> TestApp {
    use crate::config::AuthMode;

    let mut config = crate::config::Config::for_tests();
    config.auth_mode = AuthMode::Oidc;
    config.oidc_issuer = Some(idp.issuer.clone());
    config.oidc_client_id = Some("routarr".into());
    config.oidc_client_secret = Some(secret.into());
    config.oidc_redirect_url = Some("http://routarr.local/api/v1/auth/oidc/callback".into());
    // The `sub` the provider signs in unless a test says otherwise.
    config.oidc_allowed_subjects = vec!["user-42".into()];

    let state = crate::state::AppState::for_tests().await.with_config(config);
    TestApp::around(state)
}

/// The same, with the configuration `adjust` leaves.
async fn oidc_app_configured(
    idp: &crate::tests::fake_oidc::FakeOidc,
    adjust: impl FnOnce(&mut crate::config::Config),
) -> TestApp {
    let app = oidc_app(idp).await;
    let mut config = (*app.state.config).clone();
    adjust(&mut config);
    TestApp::around(app.state.clone().with_config(config))
}

/// Not verifying the token's signature rests on the exchange happening over
/// TLS, so a provider that describes its token endpoint as plain `http://`
/// pulls that assumption out from under the flow. The issuer is checked at
/// startup, and the endpoints are only known once the provider has been asked.
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

/// The authorization endpoint is the one a browser is sent to: served in the
/// clear, the code and the state travel unencrypted, so it is refused as the
/// token endpoint is. A document describing another issuer than the one it
/// was fetched from is refused too: a redirect has moved the conversation.
#[tokio::test]
async fn a_provider_misdescribing_itself_is_refused_naming_what_to_fix() {
    let clear = crate::tests::fake_oidc::FakeOidc::start().await;
    clear.advertise_authorization_at("http://idp.example");
    let app = oidc_app(&clear).await;
    let refused = crate::services::oidc::start(&app.state).await.err().map(|e| e.to_string());
    let refused = refused.expect("an authorization endpoint in the clear was accepted");
    assert!(refused.contains("authorization_endpoint"), "{refused}");

    let renamed = crate::tests::fake_oidc::FakeOidc::start().await;
    renamed.call_itself("https://elsewhere.example");
    let app = oidc_app(&renamed).await;
    let refused = crate::services::oidc::start(&app.state).await.err().map(|e| e.to_string());
    let refused = refused.expect("a document naming another issuer was accepted");
    assert!(refused.contains("elsewhere.example"), "{refused}");
}

/// A redirect address registered as `https://` says the browser comes over
/// TLS, for a proxy that forwards no scheme: the session cookie is `Secure`,
/// and over plain HTTP it is not.
#[tokio::test]
async fn an_https_redirect_address_makes_the_session_cookie_secure() {
    for (redirect, secure) in [
        ("https://routarr.example/api/v1/auth/oidc/callback", true),
        ("http://routarr.local/api/v1/auth/oidc/callback", false),
    ] {
        let idp = crate::tests::fake_oidc::FakeOidc::start().await;
        let app = oidc_app(&idp).await;
        let config = crate::config::Config {
            oidc_redirect_url: Some(redirect.into()),
            ..(*app.state.config).clone()
        };
        let app = TestApp::around(app.state.clone().with_config(config));
        let flow = start_flow(&app).await;
        idp.will_claim(serde_json::json!({ "nonce": flow.nonce }));

        let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
        let request = Request::get(&path)
            .header(axum::http::header::COOKIE, &flow.cookie)
            .body(Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(app.router.clone(), request).await.unwrap();
        let session = response
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .find(|cookie| cookie.starts_with("routarr_session="))
            .expect("no session cookie");
        assert_eq!(session.contains("; Secure"), secure, "{redirect}: {session}");
    }
}

/// An attempt lives ten minutes in the browser that started it, the time to
/// type a password and answer a second factor. Past them it opens nothing
/// (`services::oidc`).
#[tokio::test]
async fn a_sign_in_attempt_lives_ten_minutes() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    assert!(flow.set_cookie.contains("Max-Age=600"), "{}", flow.set_cookie);
    assert!(flow.set_cookie.contains("HttpOnly"), "{}", flow.set_cookie);
}

/// Start a sign-in as a browser would, and keep what it would keep: the
/// cookie that ties it to this browser, and the redirect with the `state` and
/// the nonce the provider is to echo.
struct Flow {
    cookie: String,
    set_cookie: String,
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
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    let location = response.headers()[header::LOCATION].to_str().unwrap().to_string();
    let asked: std::collections::HashMap<_, _> =
        reqwest::Url::parse(&location).unwrap().query_pairs().into_owned().collect();
    let state = asked["state"].clone();
    assert!(
        cookie.starts_with(&format!("routarr_oidc_{}=", &state[..8])),
        "the attempt's cookie is not named after its state: {cookie}"
    );
    Flow { cookie, set_cookie, state, nonce: asked["nonce"].clone(), location }
}

/// Come back from the provider as the browser that left, or as another one.
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
    // The authorization code flow, and an ID token: a provider answers
    // anything else with an error page.
    let url = reqwest::Url::parse(&location).unwrap();
    let asked: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(asked.get("response_type").map(String::as_str), Some("code"), "{location}");
    let scopes = asked.get("scope").map(String::as_str).unwrap_or_default();
    assert!(scopes.split(' ').any(|scope| scope == "openid"), "{location}");
}

/// The provider is asked to describe itself once, not once per request.
///
/// `/auth/oidc/start` sits in the *public* router (a browser with no session
/// cannot be asked for one to learn it needs one), so anyone who reaches the
/// port reaches this. Refetching the discovery document per call turns one
/// cheap inbound request into one outbound request against the operator's own
/// identity provider, which rate-limits by address: the flood locks them out
/// of the thing they log in with, from their own host.
///
/// The document is static by specification, and `SignInThrottle` states the
/// same rule: a public endpoint that costs something carries a bound.
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

/// The route that starts an attempt is public, and anyone who reaches the
/// port can call it in a loop. It writes nothing: it answers with the
/// database closed.
#[tokio::test]
async fn a_flood_of_starts_writes_nothing() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    app.state.pool.close().await;

    for _ in 0..20 {
        let response = app.get("/api/v1/auth/oidc/start").await;
        assert_eq!(response.status, StatusCode::SEE_OTHER);
        let location = response.location().unwrap_or_default();
        assert!(location.starts_with(&idp.issuer), "the start failed: {location}");
    }
}

/// However many attempts others start meanwhile, the one somebody is
/// answering their provider for still finishes: nothing is evicted, since
/// nothing is kept.
#[tokio::test]
async fn a_sign_in_survives_ten_thousand_starts_from_elsewhere() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;

    let mine = start_flow(&app).await;
    for _ in 0..10_000 {
        app.get("/api/v1/auth/oidc/start").await.assert_status(StatusCode::SEE_OTHER);
    }

    idp.will_claim(serde_json::json!({ "nonce": mine.nonce }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", mine.state);
    let response = callback(&app, Some(&mine.cookie), &path).await;
    assert_eq!(response.location().as_deref(), Some("/"), "the attempt was lost");
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
    let request = Request::get(&path)
        .header(axum::http::header::COOKIE, &flow.cookie)
        .body(Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app.router.clone(), request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/", "a success lands on the application");

    let (id, subject): (String, String) =
        sqlx::query_as("SELECT id, subject FROM sessions WHERE source = 'oidc'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(subject, "alice (user-42)");
    // A session the browser is never handed is a sign-in that lands back on the
    // gate, and the attempt's cookie is cleared in the same answer.
    let cookies: Vec<&str> = response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    let handed = cookies
        .iter()
        .find_map(|c| c.strip_prefix("routarr_session=")?.split(';').next())
        .expect("no session cookie was handed");
    assert_eq!(crate::services::accounts::stored(handed), id, "{cookies:?}");
    let attempt = flow.cookie.split_once('=').unwrap().0;
    assert!(cookies.iter().any(|c| c.starts_with(&format!("{attempt}=;"))), "{cookies:?}");

    // The client proved the exchange with the verifier and its secret.
    let exchange = idp.exchanges().pop().expect("one exchange");
    assert_eq!(exchange.basic, Some(("routarr".into(), "shhh".into())), "{exchange:?}");
    assert!(!exchange.form.contains_key("client_secret"), "{exchange:?}");
    assert!(exchange.form.contains_key("code_verifier"), "{exchange:?}");
}

/// Signs in once through `idp`, and answers where the callback sent the
/// browser.
async fn signed_in_through(idp: &crate::tests::fake_oidc::FakeOidc, app: &TestApp) -> String {
    let flow = start_flow(app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let request = Request::get(&path)
        .header(axum::http::header::COOKIE, &flow.cookie)
        .body(Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app.router.clone(), request).await.unwrap();
    response.headers()["location"].to_str().unwrap().to_string()
}

/// A provider that lists `client_secret_post` alone takes the credentials in
/// the body, the one place it reads them.
#[tokio::test]
async fn a_provider_taking_the_secret_in_the_body_gets_it_there() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    idp.accept_only_post();
    let app = oidc_app(&idp).await;

    assert_eq!(signed_in_through(&idp, &app).await, "/");

    let exchange = idp.exchanges().pop().expect("one exchange");
    assert_eq!(exchange.basic, None, "{exchange:?}");
    assert_eq!(exchange.form.get("client_id").map(String::as_str), Some("routarr"));
    assert_eq!(exchange.form.get("client_secret").map(String::as_str), Some("shhh"));
}

/// A secret is any string a provider generates, and HTTP Basic joins the id
/// and the secret with a colon: each is form-encoded first, or a secret
/// holding a colon, a space or a percent sign reaches the provider altered.
#[tokio::test]
async fn a_secret_with_a_colon_reaches_the_provider_whole() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app_with_secret(&idp, "s:h h%2B+").await;

    assert_eq!(signed_in_through(&idp, &app).await, "/");

    let exchange = idp.exchanges().pop().expect("one exchange");
    assert_eq!(exchange.basic, Some(("routarr".into(), "s:h h%2B+".into())), "{exchange:?}");
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
        // No nonce at all ties the token to no attempt.
        serde_json::json!({ "nonce": null }),
        // Issued to other clients only, or to this one on another's behalf.
        serde_json::json!({ "aud": ["other-a", "other-b"] }),
        serde_json::json!({ "aud": ["routarr", "other-a"], "azp": "other-a" }),
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

/// A token issued to this client among others, its `azp` naming this one, is
/// as good as one issued to it alone. With no `preferred_username` the
/// session is named after the token's `sub`.
#[tokio::test]
async fn a_token_naming_this_client_among_others_opens_a_session_named_after_its_subject() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({
        "nonce": flow.nonce, "aud": ["routarr", "other-a"], "azp": "routarr"
    }));

    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let response = callback(&app, Some(&flow.cookie), &path).await;

    assert_eq!(response.location().as_deref(), Some("/"));
    let subject: String = sqlx::query_scalar("SELECT subject FROM sessions")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(subject, "user-42");
}

/// The browser that started the attempt carries its `state` in a cookie, and
/// only that browser may finish it. A callback accepting any known `code` and
/// `state` pair from whoever presents it would let a link carrying somebody
/// else's pair sign the reader in as that somebody: login CSRF, with the
/// attribution on every write theirs.
#[tokio::test]
async fn a_callback_from_a_browser_that_did_not_start_the_attempt_opens_nothing() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce, "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);

    // No cookie at all, another attempt's, and one forged under this
    // attempt's name.
    let other = start_flow(&app).await;
    let name = flow.cookie.split_once('=').unwrap().0;
    let forged = format!("{name}=enc:v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    for foreign in [None, Some(other.cookie.as_str()), Some(forged.as_str())] {
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

/// An authorisation code is single-use: the provider spends it at the first
/// exchange, and the callback clears the attempt's cookie, so a replay with
/// a copy of that cookie opens nothing either.
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

/// Whoever the provider authenticates is not whoever may enter: a family
/// member with an account for another service is refused, and the log names
/// the `sub` it refused, never the token.
#[tokio::test]
async fn a_subject_outside_the_allowed_list_opens_nothing() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce, "sub": "u2" }));

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let response = callback(&app, Some(&flow.cookie), &path)
        .with_subscriber(tracing::Dispatch::new(subscriber))
        .await;

    assert_eq!(response.location().as_deref(), Some("/?signin=failed"));
    assert_eq!(app.count("SELECT COUNT(*) FROM sessions").await, 0);
    let log = capture.contents();
    assert!(log.contains("WARN") && log.contains("'u2'"), "{log}");
    assert!(!log.contains("header."), "the token is in the log:\n{log}");
}

/// A group the operator names lets its members in, read from the claim they
/// named, and the provider is asked for the scope that carries it.
#[tokio::test]
async fn a_member_of_an_allowed_group_signs_in() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app_configured(&idp, |config| {
        config.oidc_allowed_subjects.clear();
        config.oidc_allowed_groups = vec!["media".into()];
        config.oidc_groups_claim = "roles".into();
    })
    .await;

    let member = start_flow(&app).await;
    assert!(member.location.contains("scope=openid+profile+groups"), "{}", member.location);
    idp.will_claim(
        serde_json::json!({ "nonce": member.nonce, "sub": "u7", "roles": ["family", "media"] }),
    );
    let path = format!("/api/v1/auth/oidc/callback?code=one&state={}", member.state);
    assert_eq!(callback(&app, Some(&member.cookie), &path).await.location().as_deref(), Some("/"));

    let outsider = start_flow(&app).await;
    idp.will_claim(
        serde_json::json!({ "nonce": outsider.nonce, "sub": "u8", "roles": ["family"] }),
    );
    let path = format!("/api/v1/auth/oidc/callback?code=two&state={}", outsider.state);
    let refused = callback(&app, Some(&outsider.cookie), &path).await;
    assert_eq!(refused.location().as_deref(), Some("/?signin=failed"));
    assert_eq!(app.count("SELECT COUNT(*) FROM sessions").await, 1);
}

/// Letting every account of the provider in is the operator's to say, and
/// once said it stays on screen.
#[tokio::test]
async fn anyone_let_in_is_said_in_the_status() {
    let key = "a-key-pinned-for-this-test-and-long-enough";
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app_configured(&idp, |config| {
        config.oidc_allowed_subjects.clear();
        config.oidc_allow_anyone = true;
        config.api_key = Some(key.into());
    })
    .await;
    assert_eq!(signed_in_through(&idp, &app).await, "/");

    let status = app
        .send(Request::get("/api/v1/status").header("x-api-key", key).body(Body::empty()).unwrap())
        .await;
    let codes: Vec<String> = status.assert_ok()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.iter().any(|code| code == "oidc_open_to_anyone"), "{codes:?}");
}

/// A person who refused at the provider and a client the provider does not
/// know look alike on screen. The log tells them apart, and never carries
/// the code.
#[tokio::test]
async fn a_provider_refusal_is_logged_with_its_reason() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let path = format!(
        "/api/v1/auth/oidc/callback?error=access_denied&error_description=The+user+said+no&state={}",
        flow.state
    );
    let response = callback(&app, Some(&flow.cookie), &path)
        .with_subscriber(tracing::Dispatch::new(subscriber))
        .await;

    assert_eq!(response.location().as_deref(), Some("/?signin=failed"));
    let log = capture.contents();
    assert!(log.contains("access_denied") && log.contains("The user said no"), "{log}");
}

/// The callback answers a browser following a link, so a session that cannot
/// be written still sends it back to the sign-in screen rather than to a page
/// of JSON.
#[tokio::test]
async fn a_failed_session_write_still_redirects() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "nonce": flow.nonce }));
    app.state.pool.close().await;

    let path = format!("/api/v1/auth/oidc/callback?code=abc&state={}", flow.state);
    let response = callback(&app, Some(&flow.cookie), &path).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location().as_deref(), Some("/?signin=failed"));
}

/// Sign-in clicked twice, or in two tabs: each attempt has a cookie of its
/// own, and both finish whichever comes back first.
#[tokio::test]
async fn two_attempts_in_one_browser_both_finish() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let first = start_flow(&app).await;
    let second = start_flow(&app).await;
    let jar = format!("{}; {}", first.cookie, second.cookie);

    for (flow, code) in [(&second, "two"), (&first, "one")] {
        idp.will_claim(serde_json::json!({ "nonce": flow.nonce }));
        let path = format!("/api/v1/auth/oidc/callback?code={code}&state={}", flow.state);
        assert_eq!(callback(&app, Some(&jar), &path).await.location().as_deref(), Some("/"));
    }
    assert_eq!(app.count("SELECT COUNT(*) FROM sessions").await, 2);
}
