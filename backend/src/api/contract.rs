//! The API an outside application may rely on, as an OpenAPI document.
//!
//! Each function below stands for the handler of one route and is never
//! called: it carries the prose a reader of the reference sees, while every
//! schema comes from the types the handlers take and return, so the document
//! cannot describe a field the code does not send. The operations are the
//! routes `GRANTS` opens to an application key, and `/ping`.
//!
//! `openapi/v1.json` pins the document. `tests/contract.rs` fails when the two
//! part, and CI compares the pinned document with the last release's to catch
//! a change that would break a client.

use axum::extract::State;
use axum::http::Method;
use utoipa::openapi::path::Operation;
use utoipa::openapi::response::{Response, ResponseBuilder};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{ContentBuilder, OpenApi as Document, Ref, Server};
use utoipa::{Modify, OpenApi};

use super::Json;
use crate::api::applications::scope_for;
use crate::state::AppState;

// The types the operations name, under the names the schemas take.
use crate::api::Page;
use crate::api::auth::Me;
use crate::api::decisions::{ApplyAllRequest, ApplyDecisionsRequest, RevertDecisionsRequest};
use crate::api::health::{HealthQuery, HealthResponse, Pong, StatusResponse};
use crate::api::jobs::{Accepted, Job, JobQuery};
use crate::api::media::{Explanation, ExternalTitle, MediaDetail, MediaListItem, PlacementOptions};
use crate::api::overrides::{Deleted, PinRequest};
use crate::error::ErrorResponse;
use crate::models::{
    CategoryWithUsage, CreateOverrideRequest, Decision, DecisionQuery, MediaQuery, OverrideEntry,
    OverrideWithMedia, SimulationRequest, SimulationResult,
};
use crate::services::executor::{ApplyReport, BatchApplyReport};
use crate::services::notify::Notification;
use crate::services::placement::Placement;
use crate::services::sync::SyncReport;

/// The scope an operation asks of an application key, as the document states it.
pub const SCOPE_EXTENSION: &str = "x-routarr-scope";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Routarr API",
        version = "1",
        description = "The part of Routarr's API another application may rely on: a script, \
n8n, Home Assistant or a dashboard.\n\n\
Authenticate with an application key, made on Routarr's Applications screen, sent as \
`X-Api-Key: rtr_...` or `Authorization: Bearer rtr_...`. Each operation names the scope it \
needs in `x-routarr-scope`. Every key reads, and `operate` and `write` are granted each on its \
own. The owner's key reaches every operation.\n\n\
A failure answers one envelope: `error`, a stable code, and `message`, a sentence in the \
interface language that is not part of the contract. A move that crosses a guardrail answers \
409 `confirmation_required` with the guardrail's name in `confirm`, and in `includes` the other \
guardrails its question states. Send those names back in `confirm` to go ahead, or, when \
`answerable` is false, leave the question to a person.\n\n\
A call that starts long work (a simulation, an apply, a revert, the sync of one instance) waits \
for it and answers its report. Sent with `Prefer: respond-async`, it answers 202 as soon as the \
task has started, with `Location` naming the task: `GET /jobs/{id}` follows it, and its \
`result` holds the report once it has finished. A guardrail's question and any refusal still \
answer at once.\n\n\
The notification webhook set in Routarr's settings receives a `Notification` for each failure, \
and for the syncs that failed, the simulations and the moves that finished when asked to. With \
a signing secret it is signed as Standard Webhooks specifies: `webhook-id`, stable across \
retries, `webhook-timestamp`, and `webhook-signature`, `v1,` and a base64 HMAC-SHA256 of \
`id.timestamp.body` under the secret's key, twice while a new secret replaces an old one. A \
delivery refused with 5xx or 429, or not answered, is tried again after 10 s, 60 s and 5 min.\n\n\
Nothing documented under `/api/v1` is removed or renamed, and no field changes type. New \
operations, new fields and new values of the open lists (`action`, `status`, `error`, \
`confirm`, the kinds of a condition) may appear in any release."
    ),
    servers((url = "/api/v1")),
    paths(
        ping,
        who_am_i,
        status,
        metrics,
        health,
        list_categories,
        list_media,
        get_media,
        explain_media,
        place_title,
        list_decisions,
        simulate,
        apply,
        apply_all,
        revert,
        list_exceptions,
        set_exception,
        remove_exception,
        pin_by_external_id,
        unpin_by_external_id,
        list_tasks,
        get_task,
        sync_all,
        sync_instance,
    ),
    components(schemas(ErrorResponse, Notification)),
    modifiers(&Keys, &Failures, &Scopes),
    security(("api_key" = []), ("bearer" = [])),
    tags(
        (name = "status", description = "Whether Routarr is up, and what needs attention."),
        (name = "library", description = "The titles Routarr read from the Arrs, and where \
each one goes."),
        (name = "proposals", description = "Simulating the rules over the library, and \
applying or reverting what they propose."),
        (name = "exceptions", description = "Titles pinned to a category by hand, which \
outrank every rule."),
        (name = "tasks", description = "What ran in the background, and how it ended."),
        (name = "instances", description = "Reading the library again from the Arrs."),
    ),
)]
pub struct Contract;

/// The document, with the server a client reaches it through.
pub fn document(base_path: &str) -> Document {
    let mut document = Contract::openapi();
    document.servers = Some(vec![Server::new(format!("{base_path}/api/v1"))]);
    document
}

/// `GET /api/v1/openapi.json`, public: it describes the software, not the
/// installation, and a client needs it before it holds a key.
pub async fn serve(State(state): State<AppState>) -> Json<Document> {
    Json(document(&state.config.base_path))
}

/// The two ways a key travels.
struct Keys;

impl Modify for Keys {
    fn modify(&self, document: &mut Document) {
        let components = document.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-Api-Key"))),
        );
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

/// Every operation fails in the one envelope, whatever the status.
struct Failures;

impl Modify for Failures {
    fn modify(&self, document: &mut Document) {
        let failure: Response = ResponseBuilder::new()
            .description("A refusal or a failure, in the envelope every error shares.")
            .content(
                "application/json",
                ContentBuilder::new().schema(Some(Ref::from_schema_name("ErrorResponse"))).build(),
            )
            .build();
        for_each_operation(document, |_, _, operation| {
            operation.responses.responses.insert("default".to_string(), failure.clone().into());
        });
    }
}

/// The scope each operation asks, read from `GRANTS` so the document and the
/// middleware cannot disagree. The one operation no scope names needs no key.
struct Scopes;

impl Modify for Scopes {
    fn modify(&self, document: &mut Document) {
        for_each_operation(document, |method, path, operation| match scope_for(method, path) {
            Some(scope) => {
                operation.extensions = Some(
                    [(SCOPE_EXTENSION.to_string(), serde_json::json!(scope.as_str()))]
                        .into_iter()
                        .collect(),
                );
            }
            None => operation.security = Some(Vec::new()),
        });
    }
}

/// Every operation of the document, with its method and its path.
pub fn for_each_operation(
    document: &mut Document,
    mut visit: impl FnMut(&Method, &str, &mut Operation),
) {
    for (path, item) in document.paths.paths.iter_mut() {
        for (method, operation) in [
            (Method::GET, &mut item.get),
            (Method::POST, &mut item.post),
            (Method::PUT, &mut item.put),
            (Method::DELETE, &mut item.delete),
            (Method::PATCH, &mut item.patch),
        ] {
            if let Some(operation) = operation {
                visit(&method, path, operation);
            }
        }
    }
}

// ------------------------------------------------------------------ status

/// Whether Routarr is up
///
/// Needs no key, touches no database and calls nothing: an answer at all is
/// the news. For a container runtime or an uptime monitor.
#[utoipa::path(get, path = "/ping", tag = "status", responses((status = 200, body = Pong)))]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn ping() {}

/// Who the key belongs to
#[utoipa::path(get, path = "/auth/me", tag = "status", responses((status = 200, body = Me)))]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn who_am_i() {}

/// What needs attention
///
/// Read from the database alone, without reaching any Arr or source, so it is
/// cheap enough to poll. `warnings` lists the configuration problems Routarr
/// can see, each in the interface language.
#[utoipa::path(
    get,
    path = "/status",
    tag = "status",
    responses((status = 200, body = StatusResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn status() {}

/// Prometheus metrics
///
/// The text exposition format. A Prometheus scrape sends the key with
/// `authorization: credentials`, as a bearer token.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "status",
    responses((status = 200, description = "The metrics.", content_type = "text/plain"))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn metrics() {}

/// Reach every Arr and source, and report
///
/// Probes each enabled instance and metadata source, up to a connect timeout
/// each, and records what it found for `/status` to repeat. `probe=false`
/// answers from what the last probe recorded.
#[utoipa::path(
    get,
    path = "/health",
    tag = "status",
    params(HealthQuery),
    responses((status = 200, body = HealthResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn health() {}

// ----------------------------------------------------------------- library

/// The categories, with what uses each
#[utoipa::path(
    get,
    path = "/categories",
    tag = "library",
    responses((status = 200, body = Vec<CategoryWithUsage>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_categories() {}

/// The library, a page at a time
///
/// Every title Routarr read from the Arrs, with the category the rules chose
/// for it at the last simulation and the one an exception pins it to.
#[utoipa::path(
    get,
    path = "/media",
    tag = "library",
    params(MediaQuery),
    responses((status = 200, body = Page<MediaListItem>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_media() {}

/// One title
#[utoipa::path(
    get,
    path = "/media/{id}",
    tag = "library",
    params(("id" = String, Path, description = "The title's id in Routarr.")),
    responses((status = 200, body = MediaDetail))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn get_media() {}

/// Where a title goes, and why
///
/// Every rule the title was held against, each condition with what it
/// expected and what it found, and the folder the winner sends it to. The
/// same answer a simulation gives, for one title, without storing anything.
#[utoipa::path(
    get,
    path = "/media/{id}/explain",
    tag = "library",
    params(("id" = String, Path, description = "The title's id in Routarr.")),
    responses((status = 200, body = Explanation))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn explain_media() {}

/// Where a title another service names would go
///
/// For each enabled Arr of the title's kind, or the one `instance` names. A
/// title the library holds is decided as a simulation decides it. One the
/// library does not hold is looked up in the Arr and decided the same way,
/// and its `action` is `add`. Nothing is stored. `unanswered_fields` lists what
/// the rules read and no source answered, which `enrich=true` asks the sources
/// for. Radarr looks a movie up by its TMDb or IMDb id, Sonarr a series by its
/// TheTVDB or IMDb id. 404 when no instance knows the title.
#[utoipa::path(
    get,
    path = "/route",
    tag = "library",
    params(ExternalTitle, PlacementOptions),
    responses((status = 200, body = Placement))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn place_title() {}

// --------------------------------------------------------------- proposals

/// The proposals, a page at a time
///
/// A pending decision is a move a simulation proposed. `status=pending` with
/// `simulation_id` lists what one simulation proposed.
#[utoipa::path(
    get,
    path = "/decisions",
    tag = "proposals",
    params(DecisionQuery),
    responses((status = 200, body = Page<Decision>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_decisions() {}

/// Run the rules over the library
///
/// With `persist: true` the proposals are stored under the `simulation_id`
/// this answers, replacing the pending ones of an earlier run. Only one
/// persisting simulation runs at a time: another answers 409 until it ends.
/// A preview, `persist: false`, waits its turn, and answers 409 while four
/// are already running or waiting.
#[utoipa::path(
    post,
    path = "/simulate",
    tag = "proposals",
    request_body = SimulationRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses((status = 200, body = SimulationResult), (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."))),)
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn simulate() {}

/// Apply chosen proposals
///
/// Asks the Arr to move each title, behind the same guardrails as the
/// interface. A proposal no longer pending is skipped, so sending the same
/// list twice moves nothing twice. `move_files` needs a key allowed to move
/// files.
#[utoipa::path(
    post,
    path = "/decisions/apply",
    tag = "proposals",
    request_body = ApplyDecisionsRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = ApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."))),
        (status = 409, description = "A guardrail asks for a confirmation, named in `confirm`, \
or another apply is running.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn apply() {}

/// Apply everything a simulation proposed
///
/// In slices of the batch limit, stopping at the first slice that fails.
/// Always asks the `batch` guardrail first, whose question states how many
/// titles move.
#[utoipa::path(
    post,
    path = "/decisions/apply-all",
    tag = "proposals",
    request_body = ApplyAllRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = BatchApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."))),
        (status = 409, description = "The `batch` confirmation is asked, or another apply is \
running.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn apply_all() {}

/// Move titles back
///
/// Only a title's latest move can be reverted. The same guardrails ask as for
/// an apply.
#[utoipa::path(
    post,
    path = "/decisions/revert",
    tag = "proposals",
    request_body = RevertDecisionsRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = ApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."))),
        (status = 409, description = "A guardrail asks for a confirmation, named in `confirm`, \
or an apply is running.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn revert() {}

// -------------------------------------------------------------- exceptions

/// The exceptions
#[utoipa::path(
    get,
    path = "/overrides",
    tag = "exceptions",
    responses((status = 200, body = Vec<OverrideWithMedia>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_exceptions() {}

/// Pin a title to a category
///
/// Replaces the title's exception if it has one. The pending proposals for the
/// title are withdrawn, since the exception now decides.
#[utoipa::path(
    post,
    path = "/overrides",
    tag = "exceptions",
    request_body = CreateOverrideRequest,
    responses((status = 200, body = OverrideEntry))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn set_exception() {}

/// Remove an exception
#[utoipa::path(
    delete,
    path = "/overrides/{id}",
    tag = "exceptions",
    params(("id" = String, Path, description = "The exception's id.")),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn remove_exception() {}

/// Pin a title by the id another service gives it
///
/// For an application that knows the title by its TMDb, TheTVDB or IMDb id.
/// Every copy of the title the library holds is pinned, one per instance, or
/// only the one on `instance`. Each replaces the exception its copy had, and
/// the pending proposals for it are withdrawn. 404 when no copy is in the
/// library.
#[utoipa::path(
    put,
    path = "/overrides/external",
    tag = "exceptions",
    params(ExternalTitle),
    request_body = PinRequest,
    responses((status = 200, description = "The exceptions set, one per copy.",
body = Vec<OverrideEntry>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn pin_by_external_id() {}

/// Unpin a title by the id another service gives it
///
/// Removes the exception on every copy of the title, or on the one on
/// `instance`. `deleted` is false when none had one. 404 when no copy is in
/// the library.
#[utoipa::path(
    delete,
    path = "/overrides/external",
    tag = "exceptions",
    params(ExternalTitle),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn unpin_by_external_id() {}

// ------------------------------------------------------------------- tasks

/// The tasks, newest first
#[utoipa::path(
    get,
    path = "/jobs",
    tag = "tasks",
    params(JobQuery),
    responses((status = 200, body = Page<Job>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_tasks() {}

/// One task
#[utoipa::path(
    get,
    path = "/jobs/{id}",
    tag = "tasks",
    params(("id" = String, Path, description = "The task's id.")),
    responses((status = 200, body = Job))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn get_task() {}

// --------------------------------------------------------------- instances

/// Read every enabled Arr again
///
/// Answers once every instance is read, one report each. An instance that
/// fails is reported with its `error` and stops none of the others. It starts
/// one task per instance, so it always waits: to follow each one, sync the
/// instances one by one with `respond-async`.
#[utoipa::path(
    post,
    path = "/instances/sync",
    tag = "instances",
    responses((status = 200, body = Vec<SyncReport>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn sync_all() {}

/// Read one Arr again
///
/// A sync of an instance already running answers 409.
#[utoipa::path(
    post,
    path = "/instances/{id}/sync",
    tag = "instances",
    params(("id" = String, Path, description = "The instance's id."), ("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses((status = 200, body = SyncReport), (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."))),)
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn sync_instance() {}
