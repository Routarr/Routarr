use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tracing::error as log_error;

use crate::localization::Told;

/// Unified application error type.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    // No variant carries a `reqwest::Error`, and none converts from one: its `Display` holds the
    // full URL, TMDB's `?api_key=` included, so a `?` would put the key in a 502 body. Outbound
    // failures go through `integrations::send_json`.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    /// The state of what the call names refuses it, and stays so until somebody
    /// changes it: a name already taken, a category still in use. `reason`
    /// tells a script which, where the sentence is for a person.
    #[error("Conflict: {message}")]
    Conflict { reason: &'static str, message: String },

    /// A job holding what the call needs is running, and the same call may
    /// pass once it ends. Answered 409 like `Conflict`, with a `Retry-After`,
    /// so a script tells a wait from a refusal that will not clear.
    #[error("In progress: {message}")]
    InProgress { reason: &'static str, message: String },

    /// A destructive batch needs an explicit second pass from the caller, told by its code and
    /// never by its English message.
    ///
    /// `kind` names the guardrail that asked, which the caller sends back: one boolean would let
    /// "the destination is asleep" answer "there is not enough room" too. `includes` names the
    /// others the question states, each sent back beside `kind`.
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

    /// The server is already checking as many passwords as it will: a short
    /// wait, answered 503 with `Retry-After: 1`.
    #[error("Busy: {0}")]
    Busy(String),

    /// The sign-in behind this session is too old for what it asks, and has
    /// to be made again at the provider first.
    #[error("Sign in again: {0}")]
    Reauthenticate(String),

    /// The archive is sealed with a passphrase the request did not give, or
    /// gave wrong: asked again with the passphrase it was taken with.
    #[error("Passphrase: {0}")]
    PassphraseRequired(String),

    /// An outbound call failed: an Arr, a metadata source, the identity
    /// provider or the notification webhook.
    ///
    /// `kind` says what became of the call, `message` the detail it came
    /// with. `status` is the status the service gave, 0 when no answer came
    /// back usable, and for a service that answers 200 and refuses in its
    /// body (`Upstream::RefusedWith200`), the one its refusal amounts to.
    #[error("{}", describe_external(service, *status, kind, message))]
    ExternalApi {
        service: String,
        status: u16,
        kind: Upstream,
        message: String,
        /// Seconds the upstream asked to wait, from its `Retry-After`, carried apart from `message`
        /// so no limiter parses prose.
        retry_after: Option<u64>,
    },

    /// An Arr that is down or failing, told in the reader's language: a 502 whose sentence
    /// the interface shows as it is.
    #[error("{0}")]
    UpstreamDown(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Whether the text is the operator's alone: a database, serialization or configuration error
    /// names the schema, the input or the setup, which even the keyless webhook would answer.
    pub fn is_internal(&self) -> bool {
        matches!(
            self,
            AppError::Database(_)
                | AppError::Serialization(_)
                | AppError::Config(_)
                | AppError::Internal(_)
        )
    }

    /// An upstream failure as a key and its values.
    pub fn told(&self) -> Option<Told> {
        match self {
            AppError::ExternalApi { service, status, kind, message, .. } => {
                Some(told_external(service, *status, kind, message))
            }
            _ => None,
        }
    }

    /// The text a caller may read, in a response, a job row or a placement: the sentence alone,
    /// with no English prefix before a translated message. A full disk or a held lock says what
    /// to do, any other internal failure one generic sentence, its detail logged where it happens.
    pub fn public_message(&self) -> String {
        match self {
            AppError::NotFound(message)
            | AppError::BadRequest(message)
            | AppError::Conflict { message, .. }
            | AppError::InProgress { message, .. }
            | AppError::UpstreamDown(message)
            | AppError::Forbidden(message)
            | AppError::ConfirmationRequired { message, .. }
            | AppError::ConfirmationWithheld { message, .. }
            | AppError::TooManyRequests { message, .. }
            | AppError::Busy(message)
            | AppError::Reauthenticate(message)
            | AppError::PassphraseRequired(message) => message.clone(),
            database if database.storage().is_some() => {
                let key = match database.storage() {
                    Some(Storage::Full) => "ErrorDiskFull",
                    _ => "ErrorDatabaseBusy",
                };
                crate::localization::Localizer::new("en").translate(key, &[])
            }
            internal if internal.is_internal() => {
                "An internal error occurred. See the server log for details.".to_string()
            }
            other => other.to_string(),
        }
    }

    /// What a database failure says of the disk or of another writer, which a caller acts on
    /// where an internal error tells them nothing: free some space, or try again.
    fn storage(&self) -> Option<Storage> {
        let AppError::Database(sqlx::Error::Database(error)) = self else { return None };
        // An extended result code keeps the primary one in its low byte.
        match error.code()?.parse::<i32>().ok()? & 0xff {
            13 => Some(Storage::Full),
            5 | 6 => Some(Storage::Locked),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Storage {
    Full,
    Locked,
}

/// What became of an outbound call that failed. The code decides on it and
/// the reader is told it, so it travels as a value: a decision read back out
/// of a sentence breaks on the first rewording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Upstream {
    /// No usable answer came back.
    Transport(Transport),
    /// An answer came back that is not the API's: a web page, another
    /// service, a record that is not an object.
    Unreadable,
    /// The answer outgrew the most read of it, which `message` names.
    TooLarge,
    /// A write was redirected, and would reach the new address as a read.
    WriteRedirected,
    /// A redirect towards this other host, which the client stopped.
    RedirectedTo(String),
    /// A 401 from something in front of the service, asking for its own
    /// sign-in.
    SignInInFront,
    /// An error status on the status line.
    Http,
    /// An answer of 200 whose body refuses.
    RefusedWith200,
    /// The requests the source allows today are spent.
    QuotaSpent,
    /// The Arr's answer to a write does not show it made.
    NotListed,
}

/// What stopped a request before any usable answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
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
    /// A cloud host's metadata service answers at the address, where
    /// Routarr does not connect.
    MetadataAddress,
    /// Anything else, `message` holding the deepest cause.
    Failed,
}

impl Transport {
    fn english(self) -> &'static str {
        match self {
            Transport::NameUnresolved => "the host name does not resolve",
            Transport::Unreachable => "connection refused or host unreachable",
            Transport::TimedOut => "request timed out",
            Transport::HandshakeFailed => "the TLS handshake failed",
            Transport::NotHttp => "the server did not answer in HTTP",
            Transport::RedirectLoop => "the server redirects in a loop",
            Transport::MetadataAddress => "a cloud host's metadata service answers at the address",
            Transport::Failed => "",
        }
    }
}

/// An upstream failure in English, for the log, so the cause survives into
/// it: rendered by status alone, a body that does not decode and a refused
/// connection both read "Radarr returned 0".
fn describe_external(service: &str, status: u16, kind: &Upstream, message: &str) -> String {
    let said = message.trim();
    let with = |sentence: String| {
        if said.is_empty() { sentence } else { format!("{sentence}: {said}") }
    };
    match kind {
        Upstream::Transport(Transport::Failed) => with(format!("{service} is unreachable")),
        Upstream::Transport(transport) => {
            format!("{service} is unreachable: {}", transport.english())
        }
        Upstream::Unreadable => with(format!("{service} answered something unreadable")),
        Upstream::TooLarge => format!("{service}'s answer is larger than {said}"),
        Upstream::WriteRedirected => {
            format!("{service} redirected the write, which would reach the new address as a read")
        }
        Upstream::RedirectedTo(host) => {
            format!("{service} returned HTTP {status}: redirected to {host}")
        }
        Upstream::SignInInFront => {
            format!("{service} returned HTTP {status}: a sign-in is asked for in front of it")
        }
        Upstream::Http => with(format!("{service} returned HTTP {status}")),
        Upstream::RefusedWith200 => with(format!("{service} answered with an error")),
        Upstream::QuotaSpent => format!("{service} allows no more requests today"),
        Upstream::NotListed => with(format!("{service} did not make the change")),
    }
}

/// What an upstream failure says in the reader's language: the same
/// sentence as [`describe_external`], as a key and its values.
fn told_external(service: &str, status: u16, kind: &Upstream, message: &str) -> Told {
    let said = message.trim();
    let told = |key: &str| Told::new(key).with("service", service);
    let with_detail = |key: &str, detailed: &str| {
        if said.is_empty() { told(key) } else { told(detailed).with("detail", said) }
    };
    match kind {
        Upstream::Transport(Transport::Failed) => {
            with_detail("UpstreamUnreachable", "UpstreamFailed")
        }
        Upstream::Transport(Transport::NameUnresolved) => told("UpstreamNameUnresolved"),
        Upstream::Transport(Transport::Unreachable) => told("UpstreamUnreachable"),
        Upstream::Transport(Transport::TimedOut) => told("UpstreamTimedOut"),
        Upstream::Transport(Transport::HandshakeFailed) => told("UpstreamHandshakeFailed"),
        Upstream::Transport(Transport::NotHttp) => told("UpstreamNotHttp"),
        Upstream::Transport(Transport::RedirectLoop) => told("UpstreamRedirectLoop"),
        Upstream::Transport(Transport::MetadataAddress) => told("UpstreamMetadataAddress"),
        Upstream::Unreadable => with_detail("UpstreamUnreadable", "UpstreamUnreadableSaid"),
        Upstream::TooLarge => told("UpstreamTooLarge").with("size", said),
        Upstream::WriteRedirected => told("UpstreamWriteRedirected"),
        Upstream::RedirectedTo(host) => told("UpstreamRedirectedTo").with("host", host),
        Upstream::SignInInFront => told("UpstreamSignInInFront"),
        Upstream::Http => {
            with_detail("UpstreamStatus", "UpstreamStatusSaid").with("status", status)
        }
        Upstream::RefusedWith200 => with_detail("UpstreamRefused", "UpstreamRefusedSaid"),
        Upstream::QuotaSpent => told("UpstreamQuotaSpent"),
        Upstream::NotListed => with_detail("UpstreamNotListed", "UpstreamNotListedSaid"),
    }
}

/// Seconds a caller refused by a running job waits before asking again.
const RETRY_IN_PROGRESS: u64 = 5;

/// The envelope every refusal and failure answers.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    /// A stable code: `bad_request`, `unauthorized`, `forbidden`, `not_found`,
    /// `method_not_allowed`, `conflict`, `confirmation_required`,
    /// `payload_too_large`, `unsupported_media_type`, `too_many_requests`,
    /// `busy`, `reauthentication_required`, `passphrase_required`,
    /// `external_api_error`, `insufficient_storage` (the data disk is full) or `internal_error`.
    /// An open list.
    pub error: String,
    /// A sentence for a person, in the interface language. Never part of the
    /// contract.
    pub message: String,
    /// Why a `conflict` refuses, an open list. A job still running, with a
    /// `Retry-After` header: `apply_running`, `simulation_running`,
    /// `previews_full`, `sync_running`, `enrichment_running`, `backup_running`,
    /// `backup_just_taken`, `restore_staging`, `purge_running`. A state that
    /// stays until somebody changes it: `name_taken`, `in_use`,
    /// `already_listed`, `not_declared`, `category_mapped`,
    /// `proposals_replaced`, `dry_run`, `not_cancellable`, `key_pinned`,
    /// `only_credential`, `secret_unreadable`. Beside `unauthorized`: `key_expired`, an
    /// application key past its expiry. Beside `busy`: `database_busy`, another write held the
    /// database past the wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
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

impl AppError {
    /// The stable code of the envelope's `error`, which a task that failed on
    /// this error keeps beside its sentence.
    pub fn code(&self) -> &'static str {
        self.status_and_code().1
    }

    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            database if database.storage() == Some(Storage::Full) => {
                (StatusCode::INSUFFICIENT_STORAGE, "insufficient_storage")
            }
            database if database.storage() == Some(Storage::Locked) => {
                (StatusCode::SERVICE_UNAVAILABLE, "busy")
            }
            internal if internal.is_internal() => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict { .. } | AppError::InProgress { .. } => {
                (StatusCode::CONFLICT, "conflict")
            }
            AppError::ConfirmationRequired { .. } | AppError::ConfirmationWithheld { .. } => {
                (StatusCode::CONFLICT, "confirmation_required")
            }
            AppError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::TooManyRequests { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "too_many_requests")
            }
            AppError::Busy(_) => (StatusCode::SERVICE_UNAVAILABLE, "busy"),
            AppError::Reauthenticate(_) => (StatusCode::FORBIDDEN, "reauthentication_required"),
            AppError::PassphraseRequired(_) => (StatusCode::BAD_REQUEST, "passphrase_required"),
            AppError::ExternalApi { .. } | AppError::UpstreamDown(_) => {
                (StatusCode::BAD_GATEWAY, "external_api_error")
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_type) = self.status_and_code();

        if self.storage() == Some(Storage::Locked) {
            tracing::warn!("{self}");
        } else if self.is_internal() {
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
        let reason = match &self {
            AppError::Conflict { reason, .. } | AppError::InProgress { reason, .. } => {
                Some(*reason)
            }
            locked if locked.storage() == Some(Storage::Locked) => Some("database_busy"),
            _ => None,
        };
        let retry_after = match &self {
            AppError::TooManyRequests { retry_after, .. } => Some(*retry_after),
            AppError::Busy(_) => Some(1),
            locked if locked.storage() == Some(Storage::Locked) => Some(1),
            AppError::InProgress { .. } => Some(RETRY_IN_PROGRESS),
            AppError::ExternalApi { retry_after, .. } => *retry_after,
            _ => None,
        };
        let body = ErrorResponse {
            error: error_type.to_string(),
            message,
            reason,
            confirm,
            answerable,
            includes,
        };

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
            kind: Upstream::Http,
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
            kind: Upstream::Transport(Transport::Unreachable),
            message: String::new(),
            retry_after: None,
        };
        assert_eq!(
            error.to_string(),
            "Sonarr is unreachable: connection refused or host unreachable"
        );
    }

    #[test]
    fn an_empty_cause_still_reads_as_a_sentence() {
        let failed = Upstream::Transport(Transport::Failed);
        assert_eq!(describe_external("TMDB", 0, &failed, "   "), "TMDB is unreachable");
        assert_eq!(describe_external("TMDB", 500, &Upstream::Http, ""), "TMDB returned HTTP 500");
    }

    /// An answer that arrived is never said to be unreachable, and a body of
    /// 200 that refuses is never said to be an HTTP status.
    #[test]
    fn each_kind_of_upstream_failure_says_what_happened() {
        let english = |status, kind: Upstream, message: &str| {
            describe_external("OMDb", status, &kind, message)
        };
        assert_eq!(
            english(0, Upstream::Unreadable, "expected value at line 1"),
            "OMDb answered something unreadable: expected value at line 1"
        );
        assert_eq!(
            english(401, Upstream::RefusedWith200, "Invalid API key!"),
            "OMDb answered with an error: Invalid API key!"
        );
        assert_eq!(english(429, Upstream::QuotaSpent, ""), "OMDb allows no more requests today");

        let french = crate::localization::Localizer::new("fr");
        for (status, kind, message) in [
            (0, Upstream::Transport(Transport::TimedOut), ""),
            (0, Upstream::Transport(Transport::Failed), "connection reset"),
            (0, Upstream::Unreadable, "expected value"),
            (0, Upstream::TooLarge, "16 MiB"),
            (0, Upstream::WriteRedirected, ""),
            (302, Upstream::RedirectedTo("sso.example".into()), ""),
            (401, Upstream::SignInInFront, ""),
            (500, Upstream::Http, "boom"),
            (401, Upstream::RefusedWith200, "Invalid API key!"),
            (429, Upstream::QuotaSpent, ""),
            (0, Upstream::NotListed, "the path did not change"),
        ] {
            let told = told_external("OMDb", status, &kind, message);
            let english = crate::localization::Localizer::new("en");
            assert_ne!(english.translate(&told.key, &[]), told.key, "{kind:?}: en.json lacks it");
            assert!(told.render(&french).contains("OMDb"), "{kind:?}: {told:?}");
        }
    }

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

    /// A full disk and a write held behind another answer what the caller can do: free some
    /// space, or try again, where an internal error tells them nothing.
    #[tokio::test]
    async fn a_full_disk_and_a_held_lock_say_what_to_do() {
        use axum::body::to_bytes;
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        let dir = crate::tests::TempDir::new("storage-errors");
        let options = SqliteConnectOptions::new()
            .filename(dir.join("r.db"))
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::ZERO);
        let pool = SqlitePoolOptions::new().max_connections(2).connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE t (v TEXT)").execute(&pool).await.unwrap();
        let mut holder = pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *holder).await.unwrap();
        let locked = sqlx::query("INSERT INTO t VALUES ('x')").execute(&pool).await.unwrap_err();
        sqlx::query("ROLLBACK").execute(&mut *holder).await.unwrap();
        sqlx::query("PRAGMA max_page_count = 2").execute(&mut *holder).await.unwrap();
        let fill = "INSERT INTO t VALUES (hex(randomblob(4096)))";
        let full = sqlx::query(fill).execute(&mut *holder).await.unwrap_err();

        for (error, status, code, said) in [
            (full, StatusCode::INSUFFICIENT_STORAGE, "insufficient_storage", "is full"),
            (locked, StatusCode::SERVICE_UNAVAILABLE, "busy", "busy with another write"),
        ] {
            let response = AppError::Database(error).into_response();
            assert_eq!(response.status(), status);
            let retry = response.headers().get(axum::http::header::RETRY_AFTER).is_some();
            assert_eq!(retry, code == "busy", "{code}");
            let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"], code);
            assert!(body["message"].as_str().unwrap().contains(said), "{body}");
        }
    }

    #[tokio::test]
    async fn a_refusal_still_carries_its_reason() {
        use axum::body::to_bytes;

        let response = AppError::BadRequest("Category name cannot be empty".into()).into_response();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Category name cannot be empty"), "{body}");
    }
}
