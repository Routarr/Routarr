pub mod auth;
pub mod backup;
pub mod categories;
pub mod conditions;
pub mod config;
pub mod decisions;
pub mod health;
pub mod instances;
pub mod jobs;
pub mod localization;
pub mod logs;
pub mod maintenance;
pub mod media;
pub mod metadata;
pub mod metrics;
pub mod overrides;
pub mod root_folders;
pub mod rule_tests;
pub mod rules;
pub mod settings;
pub mod simulation;
pub mod webhook;

use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// `axum::Json` with the application's error envelope on rejection.
///
/// The stock extractor answers a body it cannot parse with a `text/plain`
/// 400, 415 or 422 of its own, outside the `{ error, message }` envelope
/// every other failure uses — and the message is serde's, which names the
/// Rust field it choked on. Every handler takes and returns this one instead.
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
            Err(rejection) => Err(envelope(rejection.status(), rejection.body_text())),
        }
    }
}

/// The `{ error, message }` body every failure carries, for a status the
/// error type has no variant for. A body over the cap stays a 413 and a wrong
/// content type a 415; the two shapes of "cannot parse this" are one 400.
fn envelope(status: StatusCode, message: String) -> Response {
    let (status, error) = match status {
        StatusCode::PAYLOAD_TOO_LARGE => (status, "payload_too_large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (status, "unsupported_media_type"),
        _ => (StatusCode::BAD_REQUEST, "bad_request"),
    };
    (status, axum::Json(serde_json::json!({ "error": error, "message": message }))).into_response()
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Standard pagination envelope shared by every list endpoint.
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
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

/// Clamp user-supplied paging parameters to a sane window.
///
/// Returns `(page, per_page, offset)`. Without the cap, `per_page=1000000`
/// turns any list endpoint into a memory amplifier.
pub fn paginate(page: Option<u32>, per_page: Option<u32>) -> (u32, u32, u32) {
    // Both ends bounded, and `page` for the same reason `per_page` already
    // was. `(page - 1) * per_page` overflows a u32 long before that ceiling,
    // and the two profiles fail differently: debug panics — which, with no
    // panic layer, dropped the connection with no response at all — while
    // release wraps and answers 200 with an offset that is not the one asked
    // for. A page nobody can reach is not worth either.
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

    /// `per_page` was clamped and `page` was not, so the multiplication
    /// overflowed: a panic in debug and, in the binary that ships, a 200
    /// carrying whatever page the wrapped offset landed on.
    #[test]
    fn a_page_number_past_every_library_cannot_overflow_the_offset() {
        let (page, per_page, offset) = paginate(Some(u32::MAX), Some(200));
        assert_eq!(page, 100_000);
        assert_eq!(per_page, 200);
        assert_eq!(offset, 99_999 * 200, "the offset follows the clamped page");

        // The whole product still fits, which is the property that matters.
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
