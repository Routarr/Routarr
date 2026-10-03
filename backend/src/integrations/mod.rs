pub mod adapter;
pub mod anilist;
pub mod certification;
pub mod jikan;
pub mod language;
pub mod omdb;
pub mod radarr;
pub mod sonarr;
pub mod tmdb;
pub mod tvdb;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

/// The transport failures an `ExternalApi` message states, each written by
/// this module and read back by [`transport_failure`] through the same
/// constant, so a rewording moves both sides at once.
const TIMED_OUT: &str = "request timed out";
const NAME_UNRESOLVED: &str = "the host name does not resolve";
const UNREACHABLE: &str = "connection refused or host unreachable";
const HANDSHAKE_FAILED: &str = "the TLS handshake failed";
const NOT_HTTP: &str = "the server did not answer in HTTP";
const REDIRECT_LOOP: &str = "the server redirects in a loop";
const UNREADABLE: &str = "unreadable ";

/// What a refusal with a status says beyond it, written and read back here
/// like the transport failures: the host a stopped redirect leads to, and a
/// sign-in asked for by something in front of the service.
const REDIRECTED_TO: &str = "redirected to ";
const SIGN_IN_IN_FRONT: &str = "a sign-in is asked for in front of the service";

/// What stopped a request before a usable answer came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    NameUnresolved,
    /// Nothing listens at the address, or the network does not reach it.
    Unreachable,
    TimedOut,
    /// The TLS handshake failed: a certificate not trusted, or `https` on a
    /// port that serves plain HTTP.
    HandshakeFailed,
    /// What answered is not HTTP, as TLS on a port given with `http`.
    NotHttp,
    /// Redirects that never leave the origin. One towards another origin is
    /// stopped by the client's policy and arrives as its 3xx status.
    RedirectLoop,
    /// Something answered 2xx with a body that is not what the API returns.
    Unreadable,
}

/// The transport failure a status-0 `ExternalApi` message states, if any.
pub(crate) fn transport_failure(message: &str) -> Option<Transport> {
    match message {
        TIMED_OUT => Some(Transport::TimedOut),
        NAME_UNRESOLVED => Some(Transport::NameUnresolved),
        UNREACHABLE => Some(Transport::Unreachable),
        HANDSHAKE_FAILED => Some(Transport::HandshakeFailed),
        NOT_HTTP => Some(Transport::NotHttp),
        REDIRECT_LOOP => Some(Transport::RedirectLoop),
        other if other.starts_with(UNREADABLE) => Some(Transport::Unreadable),
        _ => None,
    }
}

/// The host a 3xx leads to, when the client stopped it there for being
/// another host rather than another port or scheme of the same one.
pub(crate) fn redirected_to(message: &str) -> Option<&str> {
    message.strip_prefix(REDIRECTED_TO)
}

/// Whether a 401 came from something in front of the service, which asked
/// for its own sign-in, rather than from the service refusing its key.
pub(crate) fn signs_in_in_front(message: &str) -> bool {
    message == SIGN_IN_IN_FRONT
}

/// Send a request and decode its JSON body, turning any non-2xx into a typed
/// `ExternalApi` error carrying the upstream status.
///
/// Written once rather than repeated in every client, so a fix to error
/// reporting or truncation applies everywhere at once.
pub(crate) async fn send_json<T: serde::de::DeserializeOwned>(
    service: &'static str,
    request: reqwest::RequestBuilder,
) -> AppResult<T> {
    let response = check_status(service, request).await?;
    response.json::<T>().await.map_err(|e| AppError::ExternalApi {
        service: service.to_string(),
        status: 0,
        // Only a body that arrived whole and did not decode is unreadable. A
        // body that stopped coming, timed out or cut, is a transport failure,
        // although reqwest files both under `is_decode`.
        message: if in_chain(&e, &|error| error.is::<serde_json::Error>()) {
            format!("{UNREADABLE}{service} response: {}", deepest_cause(&e))
        } else {
            describe_transport_error(&e)
        },
        retry_after: None,
    })
}

/// Send a request and only assert that it succeeded.
pub(crate) async fn send_ok(
    service: &'static str,
    request: reqwest::RequestBuilder,
) -> AppResult<()> {
    check_status(service, request).await.map(|_| ())
}

async fn check_status(
    service: &'static str,
    request: reqwest::RequestBuilder,
) -> AppResult<reqwest::Response> {
    let response = request.send().await.map_err(|e| AppError::ExternalApi {
        service: service.to_string(),
        status: 0,
        message: describe_transport_error(&e),
        retry_after: None,
    })?;

    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status().as_u16();
    // Read before the body is consumed: the header is the only place a source
    // states how long it wants to be left alone.
    let retry_after = parse_retry_after(response.headers());
    let said_beyond_the_status =
        redirect_host(&response).map(|host| format!("{REDIRECTED_TO}{host}")).or_else(|| {
            (status == 401 && challenged_in_front(&response, service))
                .then(|| SIGN_IN_IN_FRONT.to_string())
        });
    let body = response.text().await.unwrap_or_default();
    Err(AppError::ExternalApi {
        service: service.to_string(),
        status,
        retry_after,
        message: said_beyond_the_status.unwrap_or_else(|| upstream_message(&body)),
    })
}

/// The host a redirect the client stopped leads to, when it is not the host
/// that answered. The client follows a redirect within the origin, so a 3xx
/// arriving here leaves it, and on the same host only the port or the scheme
/// changed.
fn redirect_host(response: &reqwest::Response) -> Option<String> {
    if !response.status().is_redirection() {
        return None;
    }
    let location = response.headers().get(reqwest::header::LOCATION)?.to_str().ok()?;
    let target = response.url().join(location).ok()?;
    let host = target.host_str()?;
    let here = response.url().host_str()?;
    (!host.eq_ignore_ascii_case(here)).then(|| host.to_string())
}

/// Whether a 401 carries a challenge from something in front of `service`.
///
/// Radarr and Sonarr answer a wrong key with a bare 401, or with a Basic
/// challenge whose realm is their own name. A proxy's basic auth or a
/// sign-in portal names another realm, or asks for another scheme.
fn challenged_in_front(response: &reqwest::Response, service: &str) -> bool {
    let Some(challenge) = response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    !challenge.to_ascii_lowercase().contains(&format!("realm=\"{}\"", service.to_ascii_lowercase()))
}

/// `Retry-After` as seconds.
///
/// Only the delta-seconds form is honoured. The HTTP-date form is legal but
/// rare, and getting it wrong would mean waiting for a date in the past. No
/// answer is better than a wrong one, since the caller already has a sane
/// default to fall back on.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        // A source asking for an hour is either broken or telling us to go away
        // for longer than any pass should last. The caller's own backoff covers
        // that case better than a sleep nobody can interrupt.
        .filter(|seconds| *seconds <= 300)
}

/// Describe a transport failure **without echoing the URL**.
///
/// `reqwest::Error`'s `Display` is `"... for url (<the full URL>)"`, query string
/// included, and the TMDb URL carries `?api_key=`. That message is both logged
/// and returned to the caller, so using it verbatim would publish the key on an
/// ordinary network hiccup. The source chain carries the useful part ("dns
/// error", "connection reset") and never the URL, so report that instead.
fn describe_transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return TIMED_OUT.to_string();
    }
    // The connector resolves the name, opens the socket and runs the TLS
    // handshake, and all three fail as one kind of error.
    if e.is_dns() {
        return NAME_UNRESOLVED.to_string();
    }
    if e.is_connect() {
        let tls = in_chain(e, &|error| {
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::InvalidData)
        });
        return if tls { HANDSHAKE_FAILED } else { UNREACHABLE }.to_string();
    }
    if e.is_redirect() {
        return REDIRECT_LOOP.to_string();
    }
    if in_chain(e, &|error| {
        error.downcast_ref::<hyper::Error>().is_some_and(hyper::Error::is_parse)
    }) {
        return NOT_HTTP.to_string();
    }
    deepest_cause(e)
}

/// The deepest cause, the most specific description that is still URL-free.
fn deepest_cause(e: &reqwest::Error) -> String {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut described = None;
    while let Some(current) = cause {
        described = Some(current.to_string());
        cause = std::error::Error::source(current);
    }
    described.unwrap_or_else(|| "the request failed".to_string())
}

/// Whether any cause of `e` answers `test`, the error an `io::Error` wraps
/// included: its `source()` skips it, and the TLS failure is carried there.
fn in_chain(e: &reqwest::Error, test: &dyn Fn(&(dyn std::error::Error + 'static)) -> bool) -> bool {
    fn visit(
        error: &(dyn std::error::Error + 'static),
        test: &dyn Fn(&(dyn std::error::Error + 'static)) -> bool,
    ) -> bool {
        if test(error) {
            return true;
        }
        let wrapped = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .is_some_and(|inner| visit(inner, test));
        wrapped || error.source().is_some_and(|source| visit(source, test))
    }
    std::error::Error::source(e).is_some_and(|source| visit(source, test))
}

/// Deserialize a byte count that is only ever displayed.
///
/// Radarr and Sonarr document `freeSpace` as an integer, but a proxy, a fork or
/// a future version returning `9.0e11`, or omitting the field, must not break
/// the whole synchronization over a number nobody routes on. Anything that is
/// not a usable integer becomes `None`.
///
/// This leniency is deliberately scoped to informational fields: a malformed
/// `id` or `path` still fails loudly, because routing depends on them.
pub(crate) fn lenient_bytes<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as i64)),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// Whether the Arr behind `filesystem`, a request to its `/api/v3/filesystem`,
/// can see this directory.
///
/// Asked of the Arr rather than of Routarr's own filesystem: the two run in
/// different containers as often as not, and `/media/films` existing here
/// says nothing about whether the process that will do the writing can
/// reach it. That mismatch is the commonest homelab fault of all, and it is
/// otherwise discovered at apply time.
pub(crate) async fn directory_exists(
    service: &'static str,
    filesystem: reqwest::RequestBuilder,
    path: &str,
) -> AppResult<bool> {
    let query = directory_query(path);
    let listing: DirectoryListing =
        send_json(service, filesystem.query(&[("path", query)])).await?;
    Ok(listing.holds(path))
}

/// What Radarr and Sonarr answer on `GET /api/v3/filesystem`.
#[derive(Debug, Deserialize)]
pub(crate) struct DirectoryListing {
    #[serde(default)]
    directories: Vec<ListedDirectory>,
}

#[derive(Debug, Deserialize)]
struct ListedDirectory {
    #[serde(default)]
    path: String,
}

impl DirectoryListing {
    /// Whether this listing holds the directory, named in full.
    ///
    /// A leaf name alone matches a namesake in another directory, so
    /// `/movies/standard/kids` would read as seen because `/movies/kids`
    /// exists. The Arr ends every directory's path with its separator.
    pub(crate) fn holds(&self, path: &str) -> bool {
        let wanted = directory_query(path);
        self.directories.iter().any(|entry| directory_query(&entry.path) == wanted)
    }
}

/// The `path` to send to `/api/v3/filesystem` so the answer lists the
/// directory that holds `path`.
///
/// Both Arrs answer through `FileSystemLookupService.LookupContents`, which
/// cuts the query after its last separator and lists what is left: asked
/// `/movies/anime` it lists `/movies/`, where the directory has to appear.
/// Asked for the parent without a trailing separator, it lists the
/// grandparent. Only a trailing `/` is trimmed, as `root_folders::create`
/// trims it before storing the path: the path checked is the path stored,
/// and anything else in it, a backslash included, belongs to a folder name.
pub(crate) fn directory_query(path: &str) -> &str {
    path.trim_end_matches('/')
}

/// What an upstream's error body says to whoever called Routarr.
///
/// Radarr and Sonarr answer `{"message", "description"}`, the description a
/// .NET stack trace meant for their own operator, and a validation refusal as
/// a list of fields each with its `errorMessage`. Anything else is kept as it
/// came, capped: bodies can be huge HTML error pages, and the cap keeps the log
/// and the API response readable.
fn upstream_message(body: &str) -> String {
    use serde_json::Value;
    let said = match serde_json::from_str::<Value>(body) {
        Ok(Value::Object(envelope)) => {
            envelope.get("message").and_then(Value::as_str).map(str::to_string)
        }
        Ok(Value::Array(fields)) => {
            let sentences: Vec<&str> =
                fields.iter().filter_map(|field| field.get("errorMessage")?.as_str()).collect();
            (!sentences.is_empty()).then(|| sentences.join(" "))
        }
        _ => None,
    };
    truncate(said.as_deref().unwrap_or(body), 500)
}

fn truncate(input: &str, max: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max).collect();
    format!("{cut}… (truncated)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `reqwest`'s own message embeds the URL, and the TMDb URL embeds the key.
    /// Whatever the failure, the description handed to the log and to the API
    /// must not contain it.
    #[tokio::test]
    async fn a_transport_failure_never_echoes_the_url_or_the_key() {
        let client = reqwest::Client::builder()
            .dns_resolver(std::sync::Arc::new(crate::tests::NoNames))
            .build()
            .unwrap();
        let error = client
            .get("http://routarr-nonexistent.invalid/3/movie/1?api_key=SUPERSECRET123")
            .send()
            .await
            .expect_err("an unresolvable host must fail");

        // The precondition: reqwest really does put the key in its own message.
        assert!(
            error.to_string().contains("SUPERSECRET123"),
            "precondition changed, reqwest no longer echoes the URL: {error}"
        );

        let described = describe_transport_error(&error);
        assert!(!described.contains("SUPERSECRET123"), "the key leaked: {described}");
        assert!(!described.contains("api_key"), "the query string leaked: {described}");
    }

    /// A connection the far end closes before answering is none of the named
    /// failures, and its description is built from the deepest cause: the one
    /// path where the error's own text, and the key in its URL, could get out.
    #[tokio::test]
    async fn a_connection_closed_before_the_answer_never_echoes_the_url_or_the_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });

        let error = reqwest::Client::new()
            .get(format!("http://{address}/3/movie/1?api_key=SUPERSECRET123"))
            .send()
            .await
            .expect_err("a closed connection must fail");
        assert!(error.to_string().contains("SUPERSECRET123"), "precondition changed: {error}");

        let described = describe_transport_error(&error);
        let named =
            [TIMED_OUT, NAME_UNRESOLVED, UNREACHABLE, HANDSHAKE_FAILED, REDIRECT_LOOP, NOT_HTTP];
        assert!(
            !named.contains(&described.as_str()),
            "a named failure, not the fallback: {described}"
        );
        assert!(!described.contains("SUPERSECRET123"), "the key leaked: {described}");
        assert!(!described.contains("api_key"), "the query string leaked: {described}");
    }

    /// A 200 that is not the API's JSON, as a captive portal or a cut body
    /// answers. reqwest puts the URL in a decode error, and a TMDb or OMDb key
    /// travels in that URL, so the description is built from the error's
    /// cause alone.
    #[tokio::test]
    async fn a_body_that_is_not_json_never_echoes_the_url_or_the_key() {
        let portal = axum::Router::new().route(
            "/3/movie/1",
            axum::routing::get(|| async {
                axum::response::Html("<html>Sign in to the Wi-Fi</html>")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, portal).await });

        let request = reqwest::Client::new()
            .get(format!("http://{address}/3/movie/1?api_key=SUPERSECRET123"));
        let error = send_json::<serde_json::Value>("TMDb", request)
            .await
            .expect_err("a page of HTML is not the API's JSON");

        let message = error.to_string();
        assert!(message.contains("unreadable"), "not reported as unreadable: {message}");
        assert!(!message.contains("SUPERSECRET123"), "the key leaked: {message}");
        assert!(!message.contains("api_key"), "the query string leaked: {message}");
    }

    /// The listing in the shape `FileSystemResult` serialises to: camelCase,
    /// every directory's path ending in the Arr's separator. Written from
    /// Radarr's and Sonarr's `FileSystemLookupService`, not captured from a
    /// running instance.
    const LISTING_OF_MOVIES: &str = r#"{
        "parent": "/",
        "directories": [
            { "type": "folder", "name": "anime", "path": "/movies/anime/",
              "lastModified": "2026-09-01T10:00:00Z", "size": 0 },
            { "type": "folder", "name": "kids", "path": "/movies/kids/",
              "lastModified": "2026-09-01T10:00:00Z", "size": 0 }
        ],
        "files": []
    }"#;

    #[test]
    fn the_query_names_the_directory_itself_so_the_arr_lists_its_parent() {
        // Cut after its last separator by the Arr, `/movies/anime` lists
        // `/movies/`. A query naming the parent would list the grandparent.
        assert_eq!(directory_query("/movies/anime/"), "/movies/anime");
        assert_eq!(directory_query("/movies/anime\\"), "/movies/anime\\", "a backslash is a name");
    }

    #[test]
    fn a_listed_directory_is_held_and_a_namesake_elsewhere_is_not() {
        let listing: DirectoryListing = serde_json::from_str(LISTING_OF_MOVIES).unwrap();

        assert!(listing.holds("/movies/anime"));
        assert!(listing.holds("/movies/anime/"), "a trailing separator is the same folder");
        assert!(!listing.holds("/movies/anmie"), "a misspelt leaf read as seen");
        // `kids` is listed, but under `/movies`, not under `/movies/standard`.
        assert!(!listing.holds("/movies/standard/kids"), "a namesake read as the folder asked");
    }

    #[test]
    fn lenient_bytes_accepts_the_shapes_an_arr_may_send() {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default, deserialize_with = "lenient_bytes")]
            free_space: Option<i64>,
        }
        let parse = |json: &str| serde_json::from_str::<Probe>(json).unwrap().free_space;

        assert_eq!(parse(r#"{"free_space": 900000000000}"#), Some(900_000_000_000));
        assert_eq!(parse(r#"{"free_space": 9.0e11}"#), Some(900_000_000_000));
        assert_eq!(parse(r#"{"free_space": "12345"}"#), Some(12_345));
        assert_eq!(parse(r#"{"free_space": null}"#), None);
        assert_eq!(parse(r#"{}"#), None, "an absent field must not fail the whole payload");
        assert_eq!(parse(r#"{"free_space": "unknown"}"#), None);
        assert_eq!(parse(r#"{"free_space": true}"#), None);
    }

    /// Radarr and Sonarr answer a failure with `message` and a
    /// `description` holding a .NET stack trace, which is theirs alone.
    #[test]
    fn an_arr_error_keeps_its_message_and_drops_its_stack_trace() {
        let body = r#"{"message":"Movie with tmdbId 999 was not found.","description":"NzbDrone.Core.Exceptions.MovieNotFoundException: at NzbDrone.Core.MetadataSource"}"#;
        assert_eq!(upstream_message(body), "Movie with tmdbId 999 was not found.");
    }

    /// A validation refusal is a list of fields, each with its own sentence.
    #[test]
    fn a_validation_refusal_keeps_every_sentence() {
        let body = r#"[{"propertyName":"Path","errorMessage":"Path is already configured"},{"propertyName":"Tags","errorMessage":"Tag does not exist"}]"#;
        assert_eq!(upstream_message(body), "Path is already configured Tag does not exist");
    }

    #[test]
    fn a_body_that_is_not_an_arr_envelope_is_kept_capped() {
        assert_eq!(upstream_message("Bad Gateway"), "Bad Gateway");
        assert!(upstream_message(&"x".repeat(900)).ends_with("(truncated)"));
    }

    #[test]
    fn truncate_keeps_short_bodies() {
        assert_eq!(truncate("  hello  ", 100), "hello");
    }

    #[test]
    fn truncate_caps_long_bodies() {
        let long = "x".repeat(1000);
        let out = truncate(&long, 10);
        assert!(out.starts_with("xxxxxxxxxx"));
        assert!(out.ends_with("(truncated)"));
    }

    #[test]
    fn truncate_is_char_safe() {
        let long = "é".repeat(50);
        assert!(truncate(&long, 10).starts_with("éééééééééé"));
    }
}
