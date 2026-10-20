//! OpenID Connect against a provider on an ephemeral port: discovery, the
//! attempt a browser carries, the exchange, who is let in, and the session
//! it opens.

use axum::body::Body;
use axum::http::{Request, StatusCode};

use serde_json::json;

use super::TestApp;

/// The whole flow against a provider on an ephemeral port: discovery, the
/// authorization URL, the exchange, and the session it opens.
pub(super) async fn oidc_app(idp: &crate::tests::fake_oidc::FakeOidc) -> TestApp {
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
    let message = match crate::services::oidc::start(&app.state, false, None).await {
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
    let refused =
        crate::services::oidc::start(&app.state, false, None).await.err().map(|e| e.to_string());
    let refused = refused.expect("an authorization endpoint in the clear was accepted");
    assert!(refused.contains("authorization_endpoint"), "{refused}");

    let renamed = crate::tests::fake_oidc::FakeOidc::start().await;
    renamed.call_itself("https://elsewhere.example");
    let app = oidc_app(&renamed).await;
    let refused =
        crate::services::oidc::start(&app.state, false, None).await.err().map(|e| e.to_string());
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

        let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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
/// cookie that ties it to this browser, the redirect with the `state` and the
/// nonce the provider is to echo, and the code the provider issued.
struct Flow {
    cookie: String,
    set_cookie: String,
    state: String,
    nonce: String,
    location: String,
    code: String,
}

/// Answer the provider as the person would, and come back with the code it
/// issued for this attempt.
async fn authorize(location: &str) -> String {
    let browser =
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let answer = browser.get(location).send().await.unwrap();
    assert_eq!(answer.status(), reqwest::StatusCode::FOUND, "the provider refused {location}");
    let back = answer.headers()[reqwest::header::LOCATION].to_str().unwrap();
    let back = reqwest::Url::parse(back).unwrap();
    let code = back.query_pairs().find(|(name, _)| name == "code").expect("a code");
    code.1.into_owned()
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
    let code = authorize(&location).await;
    Flow { cookie, set_cookie, state, nonce: asked["nonce"].clone(), location, code }
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
async fn the_provider_is_asked_to_describe_itself_once_until_the_description_ages() {
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

    // Kept for a while only, so a provider moving an endpoint is followed.
    if let Some((_, fresh_until)) = app.state.oidc_provider.write().await.as_mut() {
        *fresh_until = std::time::Instant::now();
    }
    app.get("/api/v1/auth/oidc/start").await.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(idp.discoveries(), 2, "a description past its time was kept");
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

    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", mine.code, mine.state);
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
    idp.will_claim(serde_json::json!({ "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
    callback(&app, Some(&flow.cookie), &path).await;

    assert_eq!(idp.discoveries(), 1, "one sign-in cost {} discoveries", idp.discoveries());
}

#[tokio::test]
async fn a_sound_callback_opens_a_session_naming_the_subject() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let flow = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "preferred_username": "alice" }));

    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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

    // The client proved the exchange with its secret, and with the verifier
    // the provider paired with the challenge before it answered.
    let exchange = idp.exchanges().pop().expect("one exchange");
    assert_eq!(exchange.basic, Some(("routarr".into(), "shhh".into())), "{exchange:?}");
    assert!(!exchange.form.contains_key("client_secret"), "{exchange:?}");
}

/// Signs in once through the provider `app` is configured for, and answers
/// where the callback sent the browser.
async fn signed_in(app: &TestApp) -> String {
    let flow = start_flow(app).await;
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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

    assert_eq!(signed_in(&app).await, "/");

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

    assert_eq!(signed_in(&app).await, "/");

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

        idp.will_claim(bad.clone());

        let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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
        "aud": ["routarr", "other-a"], "azp": "routarr"
    }));

    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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
    idp.will_claim(serde_json::json!({ "preferred_username": "alice" }));
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);

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

    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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
    idp.will_claim(serde_json::json!({ "sub": "u2" }));

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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
    idp.will_claim(serde_json::json!({ "sub": "u7", "roles": ["family", "media"] }));
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", member.code, member.state);
    assert_eq!(callback(&app, Some(&member.cookie), &path).await.location().as_deref(), Some("/"));

    let outsider = start_flow(&app).await;
    idp.will_claim(serde_json::json!({ "sub": "u8", "roles": ["family"] }));
    let path =
        format!("/api/v1/auth/oidc/callback?code={}&state={}", outsider.code, outsider.state);
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
    assert_eq!(signed_in(&app).await, "/");

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
    app.state.pool.close().await;

    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
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

    for flow in [&second, &first] {
        let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
        assert_eq!(callback(&app, Some(&jar), &path).await.location().as_deref(), Some("/"));
    }
    assert_eq!(app.count("SELECT COUNT(*) FROM sessions").await, 2);
}

/// Routarr holds no password to ask an OpenID Connect session for, so a
/// session older than ten minutes signs in again at the provider before it
/// makes a key, and a fresh one goes on.
#[tokio::test]
async fn an_old_oidc_session_signs_in_again_before_it_mints_a_key() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let session = |age: &str| {
        let app = &app;
        let age = age.to_string();
        async move {
            let full = crate::services::accounts::Reach::Full;
            let id = crate::services::accounts::open_session(&app.state.pool, "u-42", "oidc", full)
                .await
                .unwrap();
            let shifted = format!("-{age}");
            sqlx::query("UPDATE sessions SET created_at = datetime('now', ?) WHERE id = ?")
                .bind(shifted)
                .bind(crate::services::accounts::stored(&id))
                .execute(&app.state.pool)
                .await
                .unwrap();
            format!("routarr_session={id}")
        }
    };

    let old = session("11 minutes").await;
    let (status, body) = super::security::with_session(
        &app,
        "POST",
        "/api/v1/auth/api-key",
        &old,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "reauthentication_required", "{body}");

    let fresh = session("1 minute").await;
    let (status, body) = super::security::with_session(
        &app,
        "POST",
        "/api/v1/auth/api-key",
        &fresh,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Asked to sign the person in again, the provider is told so, and its token
/// has to say it just did: a session it kept would otherwise pass for a fresh
/// sign-in. The browser lands back on the screen that asked.
#[tokio::test]
async fn a_sign_in_asked_again_needs_a_fresh_auth_time_and_lands_where_it_was_asked() {
    use axum::http::header;
    use tower::ServiceExt;

    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = oidc_app(&idp).await;
    let again = |claims: serde_json::Value| {
        let app = &app;
        let idp = &idp;
        async move {
            let request = Request::get("/api/v1/auth/oidc/start?max_age=0&return=/settings")
                .body(Body::empty())
                .unwrap();
            let started = app.router.clone().oneshot(request).await.unwrap();
            let location = started.headers()[header::LOCATION].to_str().unwrap().to_string();
            let cookie = started.headers()[header::SET_COOKIE].to_str().unwrap();
            let cookie = cookie.split(';').next().unwrap().to_string();
            let asked: std::collections::HashMap<_, _> =
                reqwest::Url::parse(&location).unwrap().query_pairs().into_owned().collect();
            assert_eq!(asked.get("max_age").map(String::as_str), Some("0"), "{location}");
            assert_eq!(asked.get("prompt").map(String::as_str), Some("login"), "{location}");
            idp.will_claim(claims);
            let code = authorize(&location).await;
            let path = format!("/api/v1/auth/oidc/callback?code={code}&state={}", asked["state"]);
            callback(app, Some(&cookie), &path).await.location()
        }
    };

    let now = chrono::Utc::now().timestamp();
    assert_eq!(again(serde_json::json!({})).await.as_deref(), Some("/?signin=failed"));
    let stale = serde_json::json!({ "auth_time": now - 3600 });
    assert_eq!(again(stale).await.as_deref(), Some("/?signin=failed"));
    let fresh = serde_json::json!({ "auth_time": now });
    assert_eq!(again(fresh).await.as_deref(), Some("/settings"));
}

/// A screen to come back to is a path under the mount point, and nothing
/// that leaves it.
#[test]
fn a_sign_in_lands_only_under_the_mount_point() {
    use crate::services::oidc::screen;
    assert_eq!(screen("/settings").as_deref(), Some("/settings"));
    for leaving in ["//evil.example", "https://evil.example", "/\\evil", "settings", "/a b"] {
        assert_eq!(screen(leaving), None, "{leaving}");
    }
}

/// `user-42` runs Routarr, and the members of `family` read.
async fn readers_app(idp: &crate::tests::fake_oidc::FakeOidc) -> TestApp {
    oidc_app_configured(idp, |config| config.oidc_read_groups = vec!["family".into()]).await
}

/// Sign `claims` in, and answer the session cookie handed, `None` when refused.
async fn signed_in_as(
    app: &TestApp,
    idp: &crate::tests::fake_oidc::FakeOidc,
    claims: serde_json::Value,
) -> Option<String> {
    let flow = start_flow(app).await;
    idp.will_claim(claims);
    let path = format!("/api/v1/auth/oidc/callback?code={}&state={}", flow.code, flow.state);
    let answer = callback(app, Some(&flow.cookie), &path).await;
    let handed = answer.header("set-cookie").filter(|c| c.starts_with("routarr_session="))?;
    Some(handed.split(';').next().unwrap().to_string())
}

/// How many security events say `message`, once written.
async fn said(app: &TestApp, message: &'static str) -> i64 {
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM security_events WHERE message = ?")
            .bind(message)
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
    };
    super::until(|| async { count().await > 0 }).await;
    count().await
}

/// A member of a read group signs in to read alone: the provider is asked for the groups
/// though no group gives full access, the session keeps the level, and a write is refused,
/// said in the security log and to the shell.
#[tokio::test]
async fn a_member_of_a_read_group_signs_in_to_read_alone() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = readers_app(&idp).await;
    assert!(start_flow(&app).await.location.contains("scope=openid+profile+groups"));

    let reader = signed_in_as(&app, &idp, serde_json::json!({ "sub": "u7", "groups": ["family"] }))
        .await
        .expect("the reader was refused");

    let reach: String = sqlx::query_scalar("SELECT reach FROM sessions WHERE subject = 'u7'")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(reach, "read");
    assert_eq!(said(&app, "AuditOidcSignedInToRead").await, 1);
    let (status, me) =
        super::security::with_session(&app, "GET", "/api/v1/auth/me", &reader, json!({})).await;
    assert_eq!((status, me["read_only"].clone()), (StatusCode::OK, json!(true)), "{me}");
    let (status, refused) =
        super::security::with_session(&app, "POST", "/api/v1/simulate", &reader, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["reason"], "read_only", "{refused}");
    assert_eq!(said(&app, "AuditReaderRefused").await, 1);
}

/// Named for full access, a person has it, whatever group they read through besides.
#[tokio::test]
async fn full_access_wins_over_a_read_group() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = readers_app(&idp).await;

    let claims = serde_json::json!({ "sub": "user-42", "groups": ["family"] });
    let owner = signed_in_as(&app, &idp, claims).await.expect("the owner was refused");

    let (_, me) =
        super::security::with_session(&app, "GET", "/api/v1/auth/me", &owner, json!({})).await;
    assert_eq!(me["read_only"], false, "{me}");
    let reach: String =
        sqlx::query_scalar("SELECT reach FROM sessions").fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(reach, "full");
}

/// A token with no groups claim at all is the usual mistake, a provider keeping groups in
/// userinfo or sending them under no mapper: refused, and the security log says why.
#[tokio::test]
async fn a_token_carrying_no_groups_is_refused_saying_so() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = readers_app(&idp).await;

    let refused = signed_in_as(&app, &idp, serde_json::json!({ "sub": "u9" })).await;

    assert!(refused.is_none());
    assert_eq!(app.count("SELECT COUNT(*) FROM sessions").await, 0);
    said(&app, "AuditOidcRefused").await;
    let params: String =
        sqlx::query_scalar("SELECT params FROM security_events WHERE message = 'AuditOidcRefused'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(params.contains("no 'groups' claim"), "{params}");
}

/// Every route but the open few, walked by a reader: what the read scope grants a key
/// answers, everything else is refused as read only, owner routes included.
#[tokio::test]
async fn a_reader_reaches_what_the_read_scope_grants_and_nothing_else() {
    use crate::api::applications::scope_for;
    use crate::services::applications::Scope;

    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = readers_app(&idp).await;
    app.seed_library().await;
    let reader = signed_in_as(&app, &idp, serde_json::json!({ "sub": "u7", "groups": ["family"] }))
        .await
        .expect("the reader was refused");

    let routes = super::security::declared_routes();
    let mut reached = 0;
    for (method, path) in routes {
        if super::security::OPEN.contains(&(method, path.as_str())) {
            continue;
        }
        let template = super::security::route_template(&path);
        let readable = scope_for(&method.parse().unwrap(), &template) == Some(Scope::Read);
        let (status, body) =
            super::security::with_session(&app, method, &path, &reader, json!({})).await;
        if readable {
            reached += 1;
            assert!(
                status != StatusCode::FORBIDDEN && status != StatusCode::UNAUTHORIZED,
                "{method} {path} answered a reader {status}: {body}"
            );
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} let a reader in: {body}");
            assert_eq!(body["reason"], "read_only", "{method} {path}: {body}");
        }
    }
    assert!(reached > 20, "the walk reached only {reached} route(s): it proves nothing");
}

/// A reader reads the instances as a key does: no webhook token, which would post as the Arr.
#[tokio::test]
async fn a_reader_never_sees_a_webhook_token() {
    let idp = crate::tests::fake_oidc::FakeOidc::start().await;
    let app = readers_app(&idp).await;
    app.seed_library().await;
    let reader = signed_in_as(&app, &idp, serde_json::json!({ "sub": "u7", "groups": ["family"] }))
        .await
        .expect("the reader was refused");

    let (status, listed) =
        super::security::with_session(&app, "GET", "/api/v1/instances", &reader, json!({})).await;

    assert_eq!(status, StatusCode::OK, "{listed}");
    let instances = listed.as_array().expect("a list");
    assert!(!instances.is_empty(), "{listed}");
    for instance in instances {
        assert!(instance["webhook_token"].is_null(), "{instance}");
        assert!(instance["webhook_url"].is_null(), "{instance}");
    }
}
