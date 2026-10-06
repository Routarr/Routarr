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
    // carries `?api_key=`: a `From` impl would put that key in a 502 body from
    // a single `?`, silently. Every outbound failure goes through
    // `integrations::send_json`, which builds `ExternalApi` from the error's
    // source chain. Without the impl, the shortcut does not compile.
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
    /// back. Several ask through this variant, and with a single boolean
    /// answering one would answer them all: confirming "the destination is
    /// asleep" would also wave through "there is not enough room", silently,
    /// because only the first to fire is ever read.
    ///
    /// `includes` names the other guardrails the question states and answers
    /// with it, as the batch question states a sleeping or full destination.
    /// Each has to come back beside `kind`, or a caller that may answer the
    /// first would answer the others without being allowed to.
    #[error("Confirmation required: {message}")]
    ConfirmationRequired { kind: &'static str, includes: Vec<&'static str>, message: String },

    /// A guardrail asked a caller not allowed to answer it: an application key
    /// that was not given that name. The same 409 and the same `confirm`, with
    /// `answerable: false`, so a script hands the question to a person instead
    /// of sending the name back to be refused again.
    #[error("Confirmation required from a person: {message}")]
    ConfirmationWithheld { kind: &'static str, includes: Vec<&'static str>, message: String },

    /// The caller is known and may not do this.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// The caller asks faster than it may, and may ask again in
    /// `retry_after` seconds.
    #[error("Too many requests: {message}")]
    TooManyRequests { message: String, retry_after: u64 },

    /// An outbound call failed: an Arr, a metadata source, the identity
    /// provider or the notification webhook.
    ///
    /// `status` is 0 for a transport failure (refused, timed out, unreadable
    /// body), and the cause is always in `message`.
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
        /// exists to avoid.
        retry_after: Option<u64>,
    },

    /// An Arr that is down or failing, told in the reader's language.
    ///
    /// A 502 like `ExternalApi`, since the caller's request was right and a
    /// retry may get through, carrying a sentence the interface shows as it
    /// is rather than the upstream's own text.
    #[error("{0}")]
    UpstreamDown(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Whether the text is the operator's alone. `sqlx::Error` names
    /// constraints, columns and sometimes the statement, `serde_json::Error`
    /// quotes the input it choked on, and a configuration error names the key
    /// setup. None of that helps whoever made the call, and the webhook is
    /// reachable without a key, so a database failure there would describe
    /// the schema to an anonymous caller.
    pub fn is_internal(&self) -> bool {
        matches!(
            self,
            AppError::Database(_)
                | AppError::Serialization(_)
                | AppError::Config(_)
                | AppError::Internal(_)
        )
    }

    /// The text a caller may read, in a response, a job row or a placement.
    ///
    /// The human sentence alone: the `error` field already carries the
    /// machine-readable kind, and a prefix such as "Bad request:" would pin an
    /// English word in front of an otherwise translated message. An internal
    /// failure reads as one generic sentence, and its detail goes to the log
    /// where it happens.
    pub fn public_message(&self) -> String {
        match self {
            AppError::NotFound(message)
            | AppError::BadRequest(message)
            | AppError::Conflict(message)
            | AppError::UpstreamDown(message)
            | AppError::Forbidden(message)
            | AppError::ConfirmationRequired { message, .. }
            | AppError::ConfirmationWithheld { message, .. }
            | AppError::TooManyRequests { message, .. } => message.clone(),
            internal if internal.is_internal() => {
                "An internal error occurred. See the server log for details.".to_string()
            }
            other => other.to_string(),
        }
    }
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

/// The envelope every refusal and failure answers.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    /// A stable code: `bad_request`, `unauthorized`, `forbidden`, `not_found`,
    /// `conflict`, `confirmation_required`, `too_many_requests`,
    /// `external_api_error`, or an internal kind.
    pub error: String,
    /// A sentence for a person, in the interface language. Never part of the
    /// contract.
    pub message: String,
    /// The guardrail that asks, for `confirmation_required`. Sending it back
    /// in `confirm` accepts that question and no other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<&'static str>,
    /// Beside `confirm`: whether this caller may send the name back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answerable: Option<bool>,
    /// Beside `confirm`: the other guardrails the question states, sent back
    /// in `confirm` with it to accept the question whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub includes: Option<Vec<&'static str>>,
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
            AppError::ConfirmationRequired { .. } | AppError::ConfirmationWithheld { .. } => {
                (StatusCode::CONFLICT, "confirmation_required")
            }
            AppError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::TooManyRequests { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "too_many_requests")
            }
            AppError::ExternalApi { .. } | AppError::UpstreamDown(_) => {
                (StatusCode::BAD_GATEWAY, "external_api_error")
            }
            AppError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "config_error"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };

        if self.is_internal() {
            log_error!("{self}");
        }
        let message = self.public_message();

        let (confirm, answerable, includes) = match &self {
            AppError::ConfirmationRequired { kind, includes, .. } => {
                (Some(*kind), Some(true), Some(includes.clone()).filter(|i| !i.is_empty()))
            }
            AppError::ConfirmationWithheld { kind, includes, .. } => {
                (Some(*kind), Some(false), Some(includes.clone()).filter(|i| !i.is_empty()))
            }
            _ => (None, None, None),
        };
        let retry_after = match &self {
            AppError::TooManyRequests { retry_after, .. } => Some(*retry_after),
            _ => None,
        };
        let body =
            ErrorResponse { error: error_type.to_string(), message, confirm, answerable, includes };

        let mut response = (status, axum::Json(body)).into_response();
        if let Some(seconds) = retry_after {
            response.headers_mut().insert(axum::http::header::RETRY_AFTER, seconds.into());
        }
        response
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

    /// A user-facing refusal keeps its sentence: the rule above must not
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
