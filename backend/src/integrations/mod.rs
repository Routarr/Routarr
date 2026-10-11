pub mod adapter;
pub mod anilist;
pub mod arr_moves;
pub mod certification;
pub mod jikan;
pub mod language;
pub mod omdb;
pub mod radarr;
pub mod sonarr;
pub mod status;
pub mod tmdb;
pub mod tvdb;

use serde::Deserialize;

use crate::error::{AppError, AppResult, Transport, Upstream};

/// The most of an answer read before the request is given up, a library
/// listing aside (`ROUTARR_MAX_LIBRARY_MIB`): a metadata answer or an Arr's
/// folder list holds a few megabytes at most, and an address streaming without
/// end must not fill memory first, once for each request in flight.
#[cfg(not(test))]
pub(crate) const MAX_BODY: usize = 16 << 20;
#[cfg(test)]
pub(crate) const MAX_BODY: usize = 1 << 20;
/// The most of an error body read: what is shown of it is cut far shorter.
const MAX_ERROR_BODY: usize = 64 << 10;

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
    json_within(service, response).await
}

/// [`send_json`] for a library listing, read up to `cap` bytes, which
/// `ROUTARR_MAX_LIBRARY_MIB` sets.
pub(crate) async fn send_json_within<T: serde::de::DeserializeOwned>(
    service: &'static str,
    request: reqwest::RequestBuilder,
    cap: usize,
) -> AppResult<T> {
    let response = check_status(service, request).await?;
    json_capped(service, response, cap, Some("ROUTARR_MAX_LIBRARY_MIB")).await
}

/// What a source states of its own limit beside an answer: AniList sends
/// `X-RateLimit-Limit` and `X-RateLimit-Remaining` with each one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatedLimit {
    /// Requests a minute.
    pub per_minute: Option<u32>,
    /// Requests left before the source refuses.
    pub remaining: Option<u32>,
    /// When the allowance renews, in seconds since the Unix epoch.
    pub reset: Option<u64>,
}

impl StatedLimit {
    fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        fn number<T: std::str::FromStr>(
            headers: &reqwest::header::HeaderMap,
            name: &str,
        ) -> Option<T> {
            headers.get(name)?.to_str().ok()?.trim().parse().ok()
        }
        Self {
            per_minute: number(headers, "x-ratelimit-limit"),
            remaining: number(headers, "x-ratelimit-remaining"),
            reset: number(headers, "x-ratelimit-reset"),
        }
    }
}

/// [`send_json`], with what the source states of its limit beside a success.
pub(crate) async fn send_json_stating<T: serde::de::DeserializeOwned>(
    service: &'static str,
    request: reqwest::RequestBuilder,
) -> AppResult<(T, StatedLimit)> {
    let response = check_status(service, request).await?;
    let stated = StatedLimit::from_headers(response.headers());
    Ok((json_within(service, response).await?, stated))
}

/// The JSON a successful answer carries, read up to [`MAX_BODY`].
pub(crate) async fn json_within<T: serde::de::DeserializeOwned>(
    service: &'static str,
    response: reqwest::Response,
) -> AppResult<T> {
    json_capped(service, response, MAX_BODY, None).await
}

/// The JSON of an answer read up to `cap` bytes. `variable` names what raises
/// the cap, said when an answer outgrows it.
async fn json_capped<T: serde::de::DeserializeOwned>(
    service: &'static str,
    response: reqwest::Response,
    cap: usize,
    variable: Option<&str>,
) -> AppResult<T> {
    let body = match read_up_to(response, cap).await {
        Ok((body, false)) => body,
        Ok((_, true)) => {
            let raise = variable.map(|name| format!(", which {name} raises")).unwrap_or_default();
            let size = format!("{} MiB{raise}", cap >> 20);
            return Err(upstream(service, 0, Upstream::TooLarge, size));
        }
        // A body that stopped coming, timed out or cut, is a transport
        // failure, and only one that arrived whole and did not decode is
        // unreadable.
        Err(e) => return Err(transport_failure(service, &e)),
    };
    serde_json::from_slice(&body)
        .map_err(|e| upstream(service, 0, Upstream::Unreadable, e.to_string()))
}

/// The body, up to `cap` bytes, and whether more was coming.
async fn read_up_to(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<(Vec<u8>, bool), reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = cap - body.len();
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
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
    let (client, request) = request.build_split();
    let request = request.map_err(|e| transport_failure(service, &e))?;
    // A literal address never reaches the resolver that keeps names off the
    // metadata addresses, so it is checked here, the one way out.
    let literal = request.url().host_str().and_then(|host| {
        host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().ok()
    });
    if literal.is_some_and(crate::http::is_metadata_address) {
        let metadata = Upstream::Transport(Transport::MetadataAddress);
        return Err(upstream(service, 0, metadata, String::new()));
    }
    let (method, sent_to) = (request.method().clone(), request.url().clone());
    let response = client.execute(request).await.map_err(|e| transport_failure(service, &e))?;
    // A 301, 302 or 303 turns a write into a GET without its body, and the
    // GET's success is not the write's. The redirect policy follows only
    // within the origin, so an answer from another address was redirected.
    if !method.is_safe() && response.url() != &sent_to {
        return Err(upstream(service, 0, Upstream::WriteRedirected, String::new()));
    }

    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status().as_u16();
    // Read before the body is consumed: the header is the only place a source
    // states how long it wants to be left alone.
    let retry_after = parse_retry_after(response.headers());
    let beyond_the_status = redirect_host(&response).map(Upstream::RedirectedTo).or_else(|| {
        (status == 401 && challenged_in_front(&response, service))
            .then_some(Upstream::SignInInFront)
    });
    if let Some(kind) = beyond_the_status {
        return Err(upstream(service, status, kind, String::new()));
    }
    let body = read_up_to(response, MAX_ERROR_BODY)
        .await
        .map(|(body, _)| String::from_utf8_lossy(&body).into_owned())
        .unwrap_or_default();
    Err(AppError::ExternalApi {
        service: service.to_string(),
        status,
        kind: Upstream::Http,
        retry_after,
        message: upstream_message(&body),
    })
}

/// An upstream failure with no `Retry-After`.
pub(crate) fn upstream(service: &str, status: u16, kind: Upstream, message: String) -> AppError {
    AppError::ExternalApi { service: service.to_string(), status, kind, message, retry_after: None }
}

/// The refusal of a source whose daily quota is spent: a 429 naming no wait,
/// since the wait lasts until the quota renews.
pub(crate) fn quota_spent(service: &str) -> AppError {
    upstream(service, 429, Upstream::QuotaSpent, String::new())
}

/// Whether `error` is [`quota_spent`]'s.
pub(crate) fn is_quota_spent(error: &AppError) -> bool {
    matches!(error, AppError::ExternalApi { kind: Upstream::QuotaSpent, .. })
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
/// included, and the TMDB URL carries `?api_key=`. That message is both logged
/// and returned to the caller, so using it verbatim would publish the key on an
/// ordinary network hiccup. The source chain carries the useful part ("dns
/// error", "connection reset") and never the URL, so report that instead.
fn transport_failure(service: &str, e: &reqwest::Error) -> AppError {
    let (transport, cause) = describe_transport_error(e);
    upstream(service, 0, Upstream::Transport(transport), cause)
}

/// Which transport failure `e` is, and its deepest cause when it is none of
/// the named ones.
fn describe_transport_error(e: &reqwest::Error) -> (Transport, String) {
    let named = |transport| (transport, String::new());
    if e.is_timeout() {
        return named(Transport::TimedOut);
    }
    // Before the resolver's own failure, which carries it.
    if in_chain(e, &|error| error.is::<crate::http::MetadataAddress>()) {
        return named(Transport::MetadataAddress);
    }
    // The connector resolves the name, opens the socket and runs the TLS
    // handshake, and all three fail as one kind of error.
    if e.is_dns() {
        return named(Transport::NameUnresolved);
    }
    if e.is_connect() {
        let tls = in_chain(e, &|error| {
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::InvalidData)
        });
        return named(if tls { Transport::HandshakeFailed } else { Transport::Unreachable });
    }
    if e.is_redirect() {
        return named(Transport::RedirectLoop);
    }
    if in_chain(e, &|error| {
        error.downcast_ref::<hyper::Error>().is_some_and(hyper::Error::is_parse)
    }) {
        return named(Transport::NotHttp);
    }
    (Transport::Failed, deepest_cause(e))
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
    /// exists. The Arr ends every directory's path with its separator, and an
    /// Arr on Windows reads a name whatever its case.
    pub(crate) fn holds(&self, path: &str) -> bool {
        self.directories.iter().any(|entry| crate::paths::same(&entry.path, path))
    }
}

/// The `path` to send to `/api/v3/filesystem` so the answer lists the
/// directory that holds `path`.
///
/// Both Arrs answer through `FileSystemLookupService.LookupContents`, which
/// cuts the query after its last separator and lists what is left: asked
/// `/movies/anime` it lists `/movies/`, where the directory has to appear.
/// Asked for the parent without a trailing separator, it lists the
/// grandparent. A folder is asked as `root_folders::create` stores it, closing
/// separators trimmed: `/` on Linux, where a backslash belongs to a name, and
/// either on Windows, where both separate.
pub(crate) fn directory_query(path: &str) -> String {
    crate::paths::trimmed(path)
}

/// What an upstream's error body says to whoever called Routarr.
///
/// Radarr and Sonarr answer `{"message", "description"}`, the description a
/// .NET stack trace meant for their own operator, and a validation refusal as
/// a list of fields each with its `errorMessage`. TMDB says it in
/// `status_message`, a GraphQL service in `errors[0].message`. Anything else
/// is kept as it came, capped: bodies can be huge HTML error pages, and the
/// cap keeps the log and the API response readable.
fn upstream_message(body: &str) -> String {
    use serde_json::Value;
    let said = match serde_json::from_str::<Value>(body) {
        Ok(Value::Object(envelope)) => envelope
            .get("message")
            .or_else(|| envelope.get("status_message"))
            .or_else(|| envelope.get("errors")?.get(0)?.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string),
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

    /// What a source states of its limit is read from its headers, and a
    /// header missing or unreadable states nothing.
    #[test]
    fn a_stated_limit_is_read_from_the_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-limit", "30".parse().unwrap());
        headers.insert("x-ratelimit-remaining", " 0 ".parse().unwrap());
        headers.insert("x-ratelimit-reset", "soon".parse().unwrap());
        assert_eq!(
            StatedLimit::from_headers(&headers),
            StatedLimit { per_minute: Some(30), remaining: Some(0), reset: None }
        );
    }

    /// A wait in seconds, up to five minutes. Longer, or a date, is not
    /// honoured: a sleep nobody can interrupt is not how a pass should spend
    /// an hour.
    #[test]
    fn a_retry_after_is_read_in_seconds_up_to_five_minutes() {
        let asked = |value: &str| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(reqwest::header::RETRY_AFTER, value.parse().unwrap());
            parse_retry_after(&headers)
        };
        assert_eq!(asked(" 5 "), Some(5));
        assert_eq!(asked("300"), Some(300));
        assert_eq!(asked("301"), None);
        assert_eq!(asked("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
    }

    /// `reqwest`'s own message embeds the URL, and the TMDB URL embeds the key.
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

        let described = transport_failure("TMDB", &error).to_string();
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

        let (transport, described) = describe_transport_error(&error);
        assert_eq!(transport, Transport::Failed, "a named failure, not the fallback");
        assert!(!described.contains("SUPERSECRET123"), "the key leaked: {described}");
        assert!(!described.contains("api_key"), "the query string leaked: {described}");
    }

    /// A 200 that is not the API's JSON, as a captive portal or a cut body
    /// answers. reqwest puts the URL in a decode error, and a TMDB or OMDb key
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
        let error = send_json::<serde_json::Value>("TMDB", request)
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

    /// TMDB's refusal and a GraphQL error each say their sentence in a field
    /// of their own, which is all a reader needs of the body.
    #[test]
    fn a_source_error_keeps_its_sentence_rather_than_its_json() {
        let tmdb = r#"{"status_code":7,"status_message":"Invalid API key: You must be granted a valid key.","success":false}"#;
        assert_eq!(upstream_message(tmdb), "Invalid API key: You must be granted a valid key.");
        let graphql = r#"{"data":null,"errors":[{"message":"Too Many Requests.","status":429}]}"#;
        assert_eq!(upstream_message(graphql), "Too Many Requests.");
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
