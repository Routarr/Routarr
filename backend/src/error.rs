use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::error as log_error;

/// Unified application error type.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    // There is deliberately **no** variant carrying a `reqwest::Error`, and in
    // particular no `#[from]` for one. Its `Display` is
    // `"… for url (<the full URL>)"` with the query string, and the TMDb URL
    // carries `?api_key=` — a `From` impl would put that key in a 502 body from
    // a single `?`, silently. Every outbound failure goes through
    // `integrations::send_json`, which builds `ExternalApi` from the error's
    // source chain; without the impl, the shortcut does not compile.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    /// A destructive batch needs an explicit second pass from the caller.
    ///
    /// Distinct from `Conflict` so clients can recognise it by code: detecting
    /// it by substring-matching the English message breaks on any rewording,
    /// and on every translation.
    ///
    /// `kind` names *which* guardrail asked, and the caller sends that name
    /// back. Three of them ask through this variant, and a single boolean meant
    /// answering one answered all three: confirming "the destination is asleep"
    /// also waved through "there is not enough room", silently, because only
    /// the first to fire is ever read.
    #[error("Confirmation required: {message}")]
    ConfirmationRequired { kind: &'static str, message: String },

    /// A call to Radarr, Sonarr or TMDb failed.
    ///
    /// `status` is 0 for a transport failure (refused, timed out, unreadable
    /// body); the cause is always in `message`.
    #[error("{}", describe_external(service, *status, message))]
    ExternalApi {
        service: String,
        status: u16,
        message: String,
        /// Seconds the upstream asked us to wait, from its `Retry-After`.
        ///
        /// Carried structurally rather than left in `message`: a rate limiter
        /// that had to parse prose to learn how long to wait would break on the
        /// first rewording, which is the mistake `ConfirmationRequired` above
        /// exists to remember.
        retry_after: Option<u64>,
    },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Render an upstream failure so the cause survives into logs and API responses.
///
/// Rendered by status alone, a deserialization failure and a refused connection
/// both read "Radarr returned 0", with no way to tell them apart.
fn describe_external(service: &str, status: u16, message: &str) -> String {
    match (status, message.trim()) {
        (0, "") => format!("{service} is unreachable"),
        (0, cause) => format!("{service} is unreachable: {cause}"),
        (status, "") => format!("{service} returned HTTP {status}"),
        (status, cause) => format!("{service} returned HTTP {status}: {cause}"),
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
    /// Which guardrail asked, for the one variant that asks. The caller sends
    /// it back to say what it looked at, and nothing else is waved through.
    #[serde(skip_serializing_if = "Option::is_none")]
    confirm: Option<&'static str>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database_error"),
            AppError::Serialization(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "serialization_error")
            }
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::ConfirmationRequired { .. } => {
                (StatusCode::CONFLICT, "confirmation_required")
            }
            AppError::ExternalApi { .. } => (StatusCode::BAD_GATEWAY, "external_api_error"),
            AppError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "config_error"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };

        // The `error` field already carries the machine-readable kind, so the
        // message is the human sentence alone. Prefixing it with "Bad request:"
        // both duplicated that and pinned an English word in front of an
        // otherwise translated message.
        let message = match &self {
            AppError::NotFound(message)
            | AppError::BadRequest(message)
            | AppError::Conflict(message)
            | AppError::ConfirmationRequired { message, .. } => message.clone(),

            // The underlying text is logged, never returned. `sqlx::Error`
            // names constraints, columns and sometimes the statement;
            // `serde_json::Error` quotes the input it choked on. None of that
            // helps whoever made the call, and the webhook is reachable without
            // a key — so a database failure there would describe the schema to
            // an anonymous caller. The kind is in the `error` field, and the
            // detail is one `docker compose logs` away for the operator.
            internal @ (AppError::Database(_)
            | AppError::Serialization(_)
            | AppError::Config(_)
            | AppError::Internal(_)) => {
                log_error!("{internal}");
                "An internal error occurred. See the server log for details.".to_string()
            }

            other => other.to_string(),
        };

        let confirm = match &self {
            AppError::ConfirmationRequired { kind, .. } => Some(*kind),
            _ => None,
        };
        let body = ErrorResponse { error: error_type.to_string(), message, confirm };

        (status, axum::Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upstream_http_error_keeps_its_cause() {
        let error = AppError::ExternalApi {
            service: "Radarr".into(),
            status: 422,
            message: "movie already exists".into(),
            retry_after: None,
        };
        assert_eq!(error.to_string(), "Radarr returned HTTP 422: movie already exists");
    }

    #[test]
    fn a_transport_failure_reads_as_unreachable() {
        let error = AppError::ExternalApi {
            service: "Sonarr".into(),
            status: 0,
            message: "connection refused or host unreachable".into(),
            retry_after: None,
        };
        assert_eq!(
            error.to_string(),
            "Sonarr is unreachable: connection refused or host unreachable"
        );
    }

    #[test]
    fn a_decoding_failure_says_so_instead_of_returning_zero() {
        // The undiagnosable case: rendered by status alone this reads
        // "External API error: Radarr returned 0".
        let error = AppError::ExternalApi {
            service: "Radarr".into(),
            status: 0,
            message: "unreadable Radarr response: invalid type: floating point".into(),
            retry_after: None,
        };
        assert!(error.to_string().contains("invalid type: floating point"), "{error}");
    }

    #[test]
    fn an_empty_cause_still_reads_as_a_sentence() {
        assert_eq!(describe_external("TMDb", 0, "   "), "TMDb is unreachable");
        assert_eq!(describe_external("TMDb", 500, ""), "TMDb returned HTTP 500");
    }

    /// The database names its constraints, and the webhook is reachable without
    /// a key: a failure there must not describe the schema to whoever called.
    #[tokio::test]
    async fn an_internal_failure_does_not_describe_itself_to_the_caller() {
        use axum::body::to_bytes;

        let leaky = "UNIQUE constraint failed: instances.webhook_token";
        for error in [
            AppError::Database(sqlx::Error::Protocol(leaky.into())),
            AppError::Internal(leaky.into()),
            AppError::Config(leaky.into()),
        ] {
            let response = error.into_response();
            let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();

            assert!(!body.contains("constraint"), "the cause reached the client: {body}");
            assert!(!body.contains("webhook_token"), "a column name reached the client: {body}");
            // The machine-readable kind still has to be there, or a client
            // cannot tell an internal failure from a refusal.
            assert!(body.contains("_error"), "the error kind was lost: {body}");
        }
    }

    /// A user-facing refusal keeps its sentence — the rule above must not
    /// swallow the messages the interface actually renders.
    #[tokio::test]
    async fn a_refusal_still_carries_its_reason() {
        use axum::body::to_bytes;

        let response = AppError::BadRequest("Category name cannot be empty".into()).into_response();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Category name cannot be empty"), "{body}");
    }
}
