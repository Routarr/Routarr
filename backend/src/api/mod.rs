pub mod account;
pub mod applications;
pub mod auth;
pub mod backup;
pub mod categories;
pub mod conditions;
pub mod config;
pub mod contract;
pub mod decisions;
pub mod deprecated;
pub mod health;
pub mod instances;
pub mod jobs;
pub mod localization;
pub mod logs;
pub mod maintenance;
pub mod media;
pub mod metadata;
pub mod metrics;
pub mod notifications;
pub mod onboarding;
pub mod overrides;
pub mod root_folders;
pub mod rule_tests;
pub mod rules;
pub mod security_log;
pub mod settings;
pub mod simulation;
pub mod templates;
pub mod webhook;

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// `axum::Json` with the application's error envelope on rejection, since the stock
/// extractor answers a body it cannot parse in `text/plain`. Every handler uses this one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<S, T> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = axum::extract::rejection::JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => {
                Err(body_refused(rejection.status(), rejection.body_text(), &rejection))
            }
        }
    }
}

/// `axum::body::Bytes` with the application's error envelope on rejection, for a handler that
/// reads its body whole and parses it itself.
#[derive(Debug, Clone, Default)]
pub struct Bytes(pub axum::body::Bytes);

impl<S> FromRequest<S> for Bytes
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::body::Bytes::from_request(req, state).await {
            Ok(bytes) => Ok(Self(bytes)),
            Err(rejection) => {
                Err(body_refused(rejection.status(), rejection.body_text(), &rejection))
            }
        }
    }
}

/// A body that could not be read: 408 when it missed its deadline (`main::body_deadline`),
/// which blames the link and not the request, the envelope otherwise.
fn body_refused(
    status: StatusCode,
    message: String,
    why: &(dyn std::error::Error + 'static),
) -> Response {
    let mut cause = Some(why);
    while let Some(error) = cause {
        if error.is::<tower_http::timeout::TimeoutError>() {
            let message =
                crate::localization::Localizer::new("en").translate("ErrorBodyTooSlow", &[]);
            return crate::error::ErrorResponse::new("request_timeout", message)
                .answered(StatusCode::REQUEST_TIMEOUT);
        }
        cause = error.source();
    }
    envelope(status, message)
}

/// `axum::extract::Query` with the application's error envelope on rejection, since the
/// stock one answers `?page=abc` in `text/plain`. Every handler reads its query through it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(envelope(rejection.status(), rejection.body_text())),
        }
    }
}

/// `axum::extract::Path` with the application's error envelope on rejection, since the
/// stock one answers an undecodable segment in `text/plain`. Every handler uses this one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Path<T>(pub T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(envelope(rejection.status(), rejection.body_text())),
        }
    }
}

/// The `{ error, message }` body every failure carries, for a status the
/// error type has no variant for. A body over the cap stays a 413 and a wrong
/// content type a 415, and the two shapes of "cannot parse this" are one 400.
fn envelope(status: StatusCode, message: String) -> Response {
    let (status, error) = match status {
        StatusCode::PAYLOAD_TOO_LARGE => (status, "payload_too_large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (status, "unsupported_media_type"),
        _ => (StatusCode::BAD_REQUEST, "bad_request"),
    };
    crate::error::ErrorResponse::new(error, message).answered(status)
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// JSON a client may keep, and ask for again with `If-None-Match`: the same
/// bytes answer 304 with nothing more. `no-cache` has it ask every time, since
/// what it holds changes with a setting or a release.
pub fn revalidated(headers: &axum::http::HeaderMap, json: String) -> Response {
    use axum::http::{HeaderValue, header};
    use sha2::Digest;
    let digest = sha2::Sha256::digest(json.as_bytes());
    let opaque: String = digest[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    let opaque = format!("\"{opaque}\"");
    // Weak, compared weakly (RFC 9110 8.8.3): the compression layer encodes this one body two ways.
    let tag = format!("W/{opaque}");
    let held = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|sent| {
            sent.split(',')
                .map(str::trim)
                .any(|each| each == "*" || each.strip_prefix("W/").unwrap_or(each) == opaque)
        });
    let mut response = if held {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, "application/json")], json).into_response()
    };
    if let Ok(tag) = HeaderValue::from_str(&tag) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// What a removal answers.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Deleted {
    pub deleted: bool,
}

/// What a creation answers: 201, the new resource, and in `Location` the
/// address it is found at from now on.
pub struct Created<T> {
    location: String,
    body: T,
}

impl<T> Created<T> {
    /// `path` under the API, as `/rules/{id}`.
    pub fn at(state: &crate::state::AppState, path: impl std::fmt::Display, body: T) -> Self {
        Self { location: format!("{}/api/v1{path}", state.config.base_path), body }
    }
}

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        let location = [(axum::http::header::LOCATION, self.location)];
        (StatusCode::CREATED, location, axum::Json(self.body)).into_response()
    }
}

/// The envelope of every paged list.
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Pagination {
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
    pub total_pages: i64,
}

impl<T> Page<T> {
    pub fn new(data: Vec<T>, page: u32, per_page: u32, total: i64) -> Self {
        let per_page = per_page.max(1);
        Self {
            data,
            pagination: Pagination {
                page,
                per_page,
                total,
                total_pages: (total + per_page as i64 - 1) / per_page as i64,
            },
        }
    }
}

/// Which page of a list that has no filter of its own.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    /// From 1. Defaults to 1.
    pub page: Option<u32>,
    /// From 1 to 200. Defaults to 50.
    pub per_page: Option<u32>,
}

/// Clamp user-supplied paging to `(page, per_page, offset)`. Without the cap,
/// `per_page=1000000` turns any list endpoint into a memory amplifier.
pub fn paginate(page: Option<u32>, per_page: Option<u32>) -> (u32, u32, u32) {
    // `page` is bounded too, or the offset overflows: a panic in debug, a wrong page in release.
    const MAX_PAGE: u32 = 100_000;
    let page = page.unwrap_or(1).clamp(1, MAX_PAGE);
    let per_page = per_page.unwrap_or(50).clamp(1, 200);
    (page, per_page, (page - 1) * per_page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_defaults() {
        assert_eq!(paginate(None, None), (1, 50, 0));
    }

    #[test]
    fn pagination_clamps_hostile_input() {
        assert_eq!(paginate(Some(0), Some(100_000)), (1, 200, 0));
        assert_eq!(paginate(Some(3), Some(0)), (3, 1, 2));
    }

    #[test]
    fn a_page_number_past_every_library_cannot_overflow_the_offset() {
        let (page, per_page, offset) = paginate(Some(u32::MAX), Some(200));
        assert_eq!(page, 100_000);
        assert_eq!(per_page, 200);
        assert_eq!(offset, 99_999 * 200, "the offset follows the clamped page");

        assert!(u32::checked_mul(page - 1, per_page).is_some());
    }

    #[test]
    fn total_pages_rounds_up() {
        let page = Page::new(vec![1, 2, 3], 1, 50, 101);
        assert_eq!(page.pagination.total_pages, 3);
    }

    #[test]
    fn total_pages_is_zero_when_empty() {
        let page: Page<i32> = Page::new(vec![], 1, 50, 0);
        assert_eq!(page.pagination.total_pages, 0);
    }
}
