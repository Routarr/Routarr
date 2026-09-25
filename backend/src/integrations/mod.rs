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
        message: format!("unreadable {service} response: {e}"),
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
    let body = response.text().await.unwrap_or_default();
    Err(AppError::ExternalApi {
        service: service.to_string(),
        status,
        retry_after,
        // Upstream bodies can be huge HTML error pages; keep the log and the API
        // response readable and avoid echoing an unbounded payload back.
        message: truncate(&body, 500),
    })
}

/// `Retry-After` as seconds.
///
/// Only the delta-seconds form is honoured. The HTTP-date form is legal but
/// rare, and getting it wrong would mean waiting for a date in the past — no
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
        // for longer than any pass should last; the caller's own backoff covers
        // that case better than a sleep nobody can interrupt.
        .filter(|seconds| *seconds <= 300)
}

/// Describe a transport failure **without echoing the URL**.
///
/// `reqwest::Error`'s `Display` is `"... for url (<the full URL>)"`, query string
/// included — and the TMDb URL carries `?api_key=`. That message is both logged
/// and returned to the caller, so using it verbatim would publish the key on an
/// ordinary network hiccup. The source chain carries the useful part ("dns
/// error", "connection reset") and never the URL, so report that instead.
fn describe_transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "request timed out".to_string();
    }
    if e.is_connect() {
        return "connection refused or host unreachable".to_string();
    }
    if e.is_redirect() {
        return "the server redirected somewhere Routarr will not follow".to_string();
    }

    // Deepest cause: the most specific description that is still URL-free.
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut described = None;
    while let Some(current) = cause {
        described = Some(current.to_string());
        cause = std::error::Error::source(current);
    }
    described.unwrap_or_else(|| "the request failed".to_string())
}

/// Deserialize a byte count that is only ever displayed.
///
/// Radarr and Sonarr document `freeSpace` as an integer, but a proxy, a fork or
/// a future version returning `9.0e11` — or omitting the field — must not break
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
        let client = reqwest::Client::new();
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
