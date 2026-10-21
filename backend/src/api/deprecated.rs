//! The paths the contract renamed, still answered under their old names until their removal
//! before 1.0, and marked deprecated on every answer. One table feeds the router's aliases, the
//! application key grants and the contract, so an old path can never reach more than its new one.

use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

/// Each old path template beside the one that replaced it, under `/api/v1`.
pub const DEPRECATED_PATHS: &[(&str, &str)] = &[
    ("/overrides", "/exceptions"),
    ("/overrides/external", "/exceptions/external"),
    ("/overrides/{id}", "/exceptions/{id}"),
    ("/jobs", "/tasks"),
    ("/jobs/{id}", "/tasks/{id}"),
    ("/jobs/{id}/cancel", "/tasks/{id}/cancel"),
    ("/logs", "/move-log"),
    ("/logs/export", "/move-log/export"),
    ("/media", "/library"),
    ("/media/facets", "/library/facets"),
    ("/media/{id}", "/library/{id}"),
    ("/media/{id}/explain", "/library/{id}/explain"),
    ("/media/{id}/keep", "/library/{id}/keep"),
    ("/metadata/providers", "/sources"),
    ("/metadata/refresh", "/sources/refresh"),
];

/// When the old paths were deprecated, an RFC 9745 date: 21 October 2026.
const DEPRECATED_AT: &str = "@1792540800";

/// Where a person reads which path replaced each one.
const DEPRECATION_PAGE: &str =
    "<https://routarr.app/api/#deprecated>; rel=\"deprecation\"; type=\"text/html\"";

/// The path that replaced the deprecated `template`, `None` for a current one.
pub fn replacement(template: &str) -> Option<&'static str> {
    DEPRECATED_PATHS.iter().find(|(old, _)| *old == template).map(|(_, new)| *new)
}

/// `template` as its current path: an old one's replacement, any other itself.
pub fn current(template: &str) -> &str {
    replacement(template).unwrap_or(template)
}

/// The route a request matched, its prefix up to `/api/v1` left out.
pub fn route_of(request: &Request) -> Option<&str> {
    let matched = request.extensions().get::<MatchedPath>()?.as_str();
    Some(matched.rsplit_once("/api/v1").map_or(matched, |(_, route)| route))
}

/// On an answer to a deprecated path, `Deprecation` and `Link` (RFC 9745), and a `Location`
/// written under the old names the caller used, so a script following it stays on them.
pub async fn signal(request: Request, next: Next) -> Response {
    let deprecated = route_of(&request).and_then(replacement).is_some();
    let mut response = next.run(request).await;
    if deprecated {
        let headers = response.headers_mut();
        headers.insert("deprecation", HeaderValue::from_static(DEPRECATED_AT));
        headers.append(header::LINK, HeaderValue::from_static(DEPRECATION_PAGE));
        let renamed = headers
            .get(header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(under_old_names)
            .and_then(|location| HeaderValue::from_str(&location).ok());
        if let Some(location) = renamed {
            headers.insert(header::LOCATION, location);
        }
    }
    response
}

/// `location` with a resource renamed back to its old name, as `/api/v1/tasks/x` to
/// `/api/v1/jobs/x`. `None` when it names no renamed resource.
fn under_old_names(location: &str) -> Option<String> {
    let (base, path) = location.split_once("/api/v1")?;
    DEPRECATED_PATHS.iter().find_map(|(old, new)| {
        let (old, new) = (old.strip_suffix("{id}")?, new.strip_suffix("{id}")?);
        let id = path.strip_prefix(new)?;
        Some(format!("{base}/api/v1{old}{id}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_under_a_new_name_is_written_back_under_the_old_one() {
        assert_eq!(
            under_old_names("/routarr/api/v1/tasks/abc").as_deref(),
            Some("/routarr/api/v1/jobs/abc")
        );
        assert_eq!(
            under_old_names("/api/v1/exceptions/o-1").as_deref(),
            Some("/api/v1/overrides/o-1")
        );
        assert_eq!(under_old_names("/api/v1/rules/r-1"), None);
    }

    /// One row per old path, every new path among the current ones, none deprecated twice.
    #[test]
    fn the_table_names_each_old_path_once_and_never_a_new_one() {
        let olds: std::collections::HashSet<&str> =
            DEPRECATED_PATHS.iter().map(|(o, _)| *o).collect();
        assert_eq!(olds.len(), DEPRECATED_PATHS.len());
        assert!(DEPRECATED_PATHS.iter().all(|(_, new)| replacement(new).is_none()));
    }
}
