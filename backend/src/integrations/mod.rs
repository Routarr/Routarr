pub mod adapter;
pub mod anilist;
pub mod arr_moves;
pub mod arr_webhook;
pub mod certification;
pub mod language;
pub mod myanimelist;
pub mod omdb;
pub mod radarr;
pub mod sonarr;
pub mod status;
pub mod tenrai;
pub mod tmdb;
pub mod tvdb;

use serde::Deserialize;

use crate::error::{AppError, AppResult, Transport, Upstream};

/// The most of an answer read before giving up, a library listing aside
/// (`ROUTARR_MAX_LIBRARY_MIB`): an address streaming without end must not fill memory, once
/// for each request in flight.
pub(crate) const MAX_BODY: usize = if cfg!(test) { TESTED_BODY } else { SHIPPED_BODY };
const SHIPPED_BODY: usize = 16 << 20;
/// Small enough for a test to stream past it at once.
const TESTED_BODY: usize = 1 << 20;
/// The most of an error body read: what is shown of it is cut far shorter.
const MAX_ERROR_BODY: usize = 64 << 10;

/// Send a request and decode its JSON body, turning any non-2xx into a typed
/// `ExternalApi` error carrying the upstream status.
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
        // A body cut or timed out is a transport failure, not an unreadable one.
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
    // A literal address never reaches the resolver that keeps names off the metadata addresses.
    let literal = request.url().host_str().and_then(|host| {
        host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().ok()
    });
    if literal.is_some_and(crate::http::is_metadata_address) {
        let metadata = Upstream::Transport(Transport::MetadataAddress);
        return Err(upstream(service, 0, metadata, String::new()));
    }
    let (method, sent_to) = (request.method().clone(), request.url().clone());
    let response = client.execute(request).await.map_err(|e| transport_failure(service, &e))?;
    // A redirect turns a write into a GET without its body, whose success is not the write's.
    if !method.is_safe() && response.url() != &sent_to {
        return Err(upstream(service, 0, Upstream::WriteRedirected, String::new()));
    }

    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status().as_u16();
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

pub(crate) fn is_quota_spent(error: &AppError) -> bool {
    matches!(error, AppError::ExternalApi { kind: Upstream::QuotaSpent, .. })
}

/// The host a redirect the client stopped leads to, when it is not the host that answered.
/// The client follows a redirect within the origin, so on the same host only the port or the
/// scheme changed.
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

/// Whether a 401 carries a challenge from something in front of `service`. An Arr answers a
/// wrong key bare or with a Basic challenge in its own realm, a proxy or a sign-in portal
/// names another realm or scheme.
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

/// `Retry-After` as seconds. The HTTP-date form is not read: the caller's own default beats
/// a wait misread from a date.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        // A longer wait is a sleep nobody can interrupt, which the caller's backoff covers.
        .filter(|seconds| *seconds <= 300)
}

/// Describe a transport failure **without echoing the URL**: `reqwest::Error`'s `Display`
/// holds it, query string included, and the TMDB URL carries `?api_key=`.
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
    // The connector's name lookup, socket and TLS handshake fail as one kind of error.
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

/// Deserialize a byte count that is only displayed, `None` unless a usable integer, so an odd
/// shape never fails a whole sync. For informational fields only: a malformed `id` or `path`
/// still fails, because routing depends on them.
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

/// Deserialize a whole number a condition reads, as a profile id or a runtime. One the Arr
/// sends in another shape is unknown, which no positive condition matches, rather than a
/// failed listing of the whole library.
pub(crate) fn lenient_number<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(n)) => {
            n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))
        }
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// Deserialize a name a condition reads, as a studio or a network: blank or of another shape,
/// it is unknown, as [`lenient_number`] reads a number.
pub(crate) fn lenient_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    })
}

/// Deserialize Radarr's `ratings`, its scores by source, keeping each on a scale Routarr knows.
/// A score in another shape is unknown, as [`lenient_number`] reads a number.
pub(crate) fn lenient_ratings<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, crate::models::Rating>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(serde_json::Value::Object(sources)) =
        Option::<serde_json::Value>::deserialize(deserializer)?
    else {
        return Ok(Default::default());
    };
    const SCALES: [(&str, &str); 4] = [
        ("imdb", "imdb"),
        ("tmdb", "tmdb"),
        ("metacritic", "metacritic"),
        ("rottenTomatoes", "rotten_tomatoes"),
    ];
    Ok(SCALES
        .iter()
        .filter_map(|(key, scale)| {
            let given = sources.get(*key)?;
            let value = given.get("value").and_then(serde_json::Value::as_f64);
            let votes = given.get("votes").and_then(serde_json::Value::as_i64);
            crate::models::rating(scale, value, votes)
        })
        .collect())
}

/// A film's collection as Radarr names it: `title` since its v5 payload, `name` before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArrCollection {
    pub title: Option<String>,
    pub tmdb_id: Option<i64>,
}

/// Deserialize Radarr's `collection`, unknown in any shape but an object.
pub(crate) fn lenient_collection<'de, D>(deserializer: D) -> Result<Option<ArrCollection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(serde_json::Value::Object(fields)) =
        Option::<serde_json::Value>::deserialize(deserializer)?
    else {
        return Ok(None);
    };
    let text = |key: &str| {
        fields.get(key).and_then(|value| value.as_str()).map(str::trim).filter(|s| !s.is_empty())
    };
    let title = text("title").or_else(|| text("name")).map(str::to_string);
    let tmdb_id = fields.get("tmdbId").and_then(serde_json::Value::as_i64).filter(|id| *id > 0);
    Ok((title.is_some() || tmdb_id.is_some()).then_some(ArrCollection { title, tmdb_id }))
}

/// Whether the Arr behind `filesystem`, a request to its `/api/v3/filesystem`, can see this
/// directory. Asked of the Arr, not of Routarr's filesystem: the two often run in different
/// containers, and the Arr does the writing.
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
    /// Whether this listing holds the directory, named in full: a leaf name alone matches a
    /// namesake elsewhere. The Arr ends every path with its separator, and on Windows reads a
    /// name whatever its case.
    pub(crate) fn holds(&self, path: &str) -> bool {
        self.directories.iter().any(|entry| crate::paths::same(&entry.path, path))
    }
}

/// The `path` to send to `/api/v3/filesystem` so the answer lists the directory holding it.
/// Both Arrs cut the query after its last separator and list what is left, so the folder is
/// asked as `root_folders::create` stores it, closing separators trimmed.
pub(crate) fn directory_query(path: &str) -> String {
    crate::paths::trimmed(path)
}

/// What an upstream's error body says to whoever called Routarr: its sentence, never an Arr's
/// .NET stack trace, and any other body as it came, capped, since it can be a huge HTML page.
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
    /// The test builds read a smaller cap, so only this sees the shipped one.
    #[test]
    fn a_release_reads_a_few_megabytes_of_an_answer_and_no_more() {
        assert!((4 << 20..=64 << 20).contains(&SHIPPED_BODY), "{SHIPPED_BODY} bytes");
    }

    use super::*;

    /// A header missing or unreadable states nothing.
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

    /// Longer, or a date, is not honoured: a pass must not sleep where nobody can interrupt it.
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

    /// A closed connection is described from its deepest cause, the one path where the
    /// error's own text, and the key in its URL, could get out.
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

    /// A 200 that is not the API's JSON, as a captive portal answers: reqwest puts the URL,
    /// and a TMDB or OMDb key with it, in a decode error.
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

    /// The listing in the shape `FileSystemResult` serialises to, written from the Arrs'
    /// `FileSystemLookupService`, not captured from a running instance.
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
        // Cut after its last separator by the Arr, `/movies/anime` lists `/movies/`.
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
    fn a_field_a_condition_reads_is_unknown_in_a_shape_it_does_not_expect() {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default, deserialize_with = "lenient_number")]
            runtime: Option<i64>,
            #[serde(default, deserialize_with = "lenient_text")]
            studio: Option<String>,
            #[serde(default, deserialize_with = "lenient_collection")]
            collection: Option<ArrCollection>,
        }
        let parse = |json: &str| serde_json::from_str::<Probe>(json).unwrap();

        let named = parse(
            r#"{"runtime": "86", "studio": " Ghibli ", "collection": {"name": "Ghibli", "tmdbId": 9}}"#,
        );
        assert_eq!((named.runtime, named.studio.as_deref()), (Some(86), Some("Ghibli")));
        let collection = ArrCollection { title: Some("Ghibli".into()), tmdb_id: Some(9) };
        assert_eq!(named.collection, Some(collection));
        let titled = parse(r#"{"runtime": 85.6, "collection": {"title": "Ghibli", "tmdbId": 0}}"#);
        assert_eq!(titled.runtime, Some(86));
        assert_eq!(
            titled.collection,
            Some(ArrCollection { title: Some("Ghibli".into()), tmdb_id: None })
        );

        let odd = parse(r#"{"runtime": [1], "studio": 4, "collection": "Ghibli"}"#);
        assert_eq!((odd.runtime, odd.studio, odd.collection), (None, None, None));
        let blank = parse(r#"{"studio": "  ", "collection": {"title": ""}}"#);
        assert_eq!((blank.studio, blank.collection), (None, None));
        let absent = parse("{}");
        assert_eq!((absent.runtime, absent.studio, absent.collection), (None, None, None));
    }

    #[test]
    fn radarr_scores_are_read_by_scale_and_an_unrated_one_is_left_out() {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default, deserialize_with = "lenient_ratings")]
            ratings: std::collections::BTreeMap<String, crate::models::Rating>,
        }
        let parse = |json: &str| serde_json::from_str::<Probe>(json).unwrap().ratings;

        let read = parse(
            r#"{"ratings": {"imdb": {"votes": 393000, "value": 8.1, "type": "user"},
                            "tmdb": {"votes": 0, "value": 0, "type": "user"},
                            "metacritic": {"votes": 0, "value": 86},
                            "rottenTomatoes": {"votes": 0, "value": 94},
                            "trakt": {"votes": 9, "value": 8}}}"#,
        );
        let scales: Vec<&str> = read.keys().map(String::as_str).collect();
        assert_eq!(scales, ["imdb", "metacritic", "rotten_tomatoes"]);
        assert_eq!((read["imdb"].value, read["imdb"].votes), (8.1, Some(393000)));
        assert_eq!(read["metacritic"].votes, None, "Metacritic counts no votes");
        assert!(parse(r#"{"ratings": [1]}"#).is_empty());
        assert!(parse(r#"{"ratings": {"imdb": {"value": "high"}}}"#).is_empty());
        assert!(parse("{}").is_empty());
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

    /// TMDB and a GraphQL service each say their sentence in a field of their own.
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
