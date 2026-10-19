//! The API an outside application may rely on, as an OpenAPI document. Each
//! function below documents one route `GRANTS` opens and is never called, while
//! every schema comes from the handlers' types. `openapi/v1.json` pins the document.

use axum::extract::State;
use axum::http::Method;
use utoipa::openapi::path::Operation;
use utoipa::openapi::response::{Response, ResponseBuilder};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{ContentBuilder, OpenApi as Document, Ref, Server};
use utoipa::{Modify, OpenApi};

use crate::api::applications::scope_for;
use crate::state::AppState;

use crate::api::auth::Me;
use crate::api::backup::BackupListResponse;
use crate::api::conditions::ConditionCatalog;
use crate::api::decisions::{
    ApplyAllRequest, ApplyDecisionsRequest, RevertAllRequest, RevertDecisionsRequest,
};
use crate::api::health::{HealthQuery, HealthResponse, Pong, StatusResponse};
use crate::api::jobs::{Accepted, Job, JobQuery};
use crate::api::logs::{LogEntry, LogFilter, LogQuery};
use crate::api::media::LibraryFacets;
use crate::api::media::{Explanation, ExternalTitle, MediaDetail, MediaListItem, RouteQuery};
use crate::api::metadata::ProvidersResponse;
use crate::api::overrides::{KeepRequest, PinRequest};
use crate::api::root_folders::{Declared, Mapped, MappingConflict};
use crate::api::rules::{
    PreviewRequest, PreviewResponse, Reordered, RuleImportReport, RuleVerdict,
};
use crate::api::templates::{TemplateAdded, TemplatePreview, TemplateRequest, TemplateSummary};
use crate::api::{Deleted, PageQuery};
use crate::error::ErrorResponse;
use crate::models::{
    Category, CategoryWithUsage, CreateCategoryRequest, CreateOverrideRequest, CreateRuleRequest,
    Decision, DecisionQuery, DeclareRootFolder, ImportRulesRequest, InstanceResponse, MediaQuery,
    OverrideEntry, OverrideWithMedia, RenameCategoryRequest, ReorderRulesRequest,
    RootFolderWithInstance, Rule, RuleBundle, SimulationRequest, SimulationResult,
    UpdateRootFolderCategory,
};
use crate::services::backup::BackupFile;
use crate::services::executor::{ApplyReport, BatchApplyReport};
use crate::services::notify::Notification;
use crate::services::placement::Placement;
use crate::services::rule_health::RuleHealthReport;
use crate::services::rule_tests::{NewRuleTest, RuleTest, RuleTestRun};
use crate::services::sync::SyncReport;

/// The scope an operation asks of an application key, as the document states it.
pub const SCOPE_EXTENSION: &str = "x-routarr-scope";

/// A page of a list as the contract describes it: its items reference their
/// own schema, where `Page<T>` describes each again inline. The handlers
/// answer `Page<T>`, which serializes the same.
macro_rules! documented_page {
    ($page:ident, $name:ident, $item:ty) => {
        #[derive(utoipa::ToSchema)]
        #[schema(as = $name)]
        #[expect(dead_code, reason = "a page's documentation, never built")]
        struct $page {
            data: Vec<$item>,
            pagination: crate::api::Pagination,
        }
    };
}

documented_page!(MediaPage, Page_MediaListItem, MediaListItem);
documented_page!(DecisionPage, Page_Decision, Decision);
documented_page!(JobPage, Page_Job, Job);
documented_page!(LogPage, Page_LogEntry, LogEntry);
documented_page!(ExceptionPage, Page_OverrideWithMedia, OverrideWithMedia);
documented_page!(RuleTestPage, Page_RuleTest, RuleTest);

#[derive(OpenApi)]
#[openapi(
    // No `version`: the crate's is used, and `scripts/check-versions.py` holds the pin to it.
    info(
        title = "Routarr API",
        description = "The part of Routarr's API another application may rely on: a script, \
n8n, Home Assistant or a dashboard.\n\n\
Authenticate with an application key, made on Routarr's Applications screen, sent as \
`X-Api-Key: rtr_...` or `Authorization: Bearer rtr_...`. Each operation names the scope it \
needs in `x-routarr-scope`. Every key reads, and `operate`, `write` and `configure` are granted \
each on its own. The owner's key reaches every operation.\n\n\
A failure answers one envelope: `error`, a stable code, and `message`, a sentence in the \
interface language that is not part of the contract. A `conflict` names its `reason`: one a \
running task causes comes with `Retry-After` and clears once the task ends, any other stays \
until somebody changes what refuses. A move that crosses a guardrail answers \
409 `confirmation_required` with the guardrail's name in `confirm`, and in `includes` the other \
guardrails its question states. Send those names back in `confirm` to go ahead, or, when \
`answerable` is false, leave the question to a person.\n\n\
A call that starts long work (a simulation, an apply, a revert, a sync, a backup) waits for \
it and answers its report. Sent with `Prefer: respond-async`, it answers 202 as soon \
as the task has started, with `Location` naming the task and `Retry-After` the seconds to wait \
before looking: `GET /jobs/{id}` follows it, and its `result` holds the report once it has \
finished, or its `error` the code the call would have answered. A guardrail's question and any \
refusal still answer at once.\n\n\
The notification webhook set in Routarr's settings receives a `Notification` for each failure, \
and for the syncs that failed, the simulations and the moves that finished when asked to. A \
format set to Discord, ntfy, Gotify or Apprise, or recognised from a Discord or ntfy.sh address, \
writes the same events in that receiver's own shape instead, which this contract does not \
cover. Its `message` is written in the language set for Routarr's interface, so a program reads \
`event` and `data`, which never change with it. With a signing secret every shape is signed as Standard Webhooks specifies: `webhook-id`, stable across \
retries, `webhook-timestamp`, and `webhook-signature`, `v1,` and a base64 HMAC-SHA256 of \
`id.timestamp.body` under the secret's key, twice while a new secret replaces an old one. A \
delivery refused with 5xx or 429, or not answered, is tried again after 10 s, 60 s and 5 min.\n\n\
An application key asks at most ten times a second past a burst of fifty, and takes a backup \
at most every ten minutes. Past either it is answered `429`, with `Retry-After` in seconds.\n\n\
From 1.0, nothing documented under `/api/v1` is removed or renamed, and no field changes type. \
Before it, a minor release may, and its release notes name each change. New operations, new \
fields and new values of the open lists (`action`, `status`, `error`, `reason`, `confirm`, the \
kinds of a condition) may appear in any release."
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
        keep_title,
        place_title,
        list_decisions,
        simulate,
        apply,
        apply_all,
        revert,
        revert_all,
        list_exceptions,
        set_exception,
        remove_exception,
        pin_by_external_id,
        unpin_by_external_id,
        list_tasks,
        get_task,
        sync_all,
        sync_instance,
        list_instances,
        get_instance,
        list_sources,
        library_facets,
        list_logs,
        export_logs,
        list_backups,
        take_backup,
        list_rules,
        get_rule,
        create_rule,
        update_rule,
        remove_rule,
        duplicate_rule,
        reorder_rules,
        condition_catalog,
        rule_health,
        export_rules,
        import_rules,
        validate_rule,
        preview_rule,
        list_templates,
        preview_template,
        import_template,
        list_rule_tests,
        create_rule_test,
        remove_rule_test,
        run_rule_tests,
        create_category,
        rename_category,
        remove_category,
        list_root_folders,
        mapping_conflicts,
        declare_destination,
        remove_destination,
        map_folder,
    ),
    components(schemas(ErrorResponse, Notification)),
    modifiers(&Keys, &Failures, &Scopes),
    security(("api_key" = []), ("bearer" = [])),
    tags(
        (name = "status", description = "Whether Routarr is up, and what needs attention."),
        (name = "library", description = "The titles Routarr read from the Arrs, and where \
each one goes."),
        (name = "decisions", description = "Simulating the rules over the library, and \
applying or reverting the moves they decide."),
        (name = "exceptions", description = "Titles whose category is forced by hand, which \
outrank every rule."),
        (name = "tasks", description = "What ran in the background, and how it ended."),
        (name = "instances", description = "The Arrs Routarr reads, and reading them again."),
        (name = "rules", description = "The rules that decide where each title goes, and the \
tests that pin what they should decide."),
        (name = "categories", description = "The categories, and the folder each one leads to on \
each Arr."),
        (name = "backups", description = "Copies of Routarr's data, taken on a schedule or on \
request."),
    ),
)]
pub struct Contract;

/// The document, with the server a client reaches it through.
pub fn document(base_path: &str) -> Document {
    let mut document = Contract::openapi();
    document.servers = Some(vec![Server::new(format!("{base_path}/api/v1"))]);
    document
}

/// `GET /api/v1/openapi.json`, public, since a client needs it before it holds a key.
/// Written once per mount point, and answered 304 to a client that holds it already.
pub async fn serve(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    static WRITTEN: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::LazyLock::new(Default::default);
    let base_path = &state.config.base_path;
    let json = WRITTEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(base_path.clone())
        .or_insert_with(|| serde_json::to_string(&document(base_path)).unwrap_or_default())
        .clone();
    crate::api::revalidated(&headers, json)
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
/// Read from the database alone, so it is cheap enough to poll. `warnings` lists
/// the configuration problems Routarr can see, in the interface language.
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
/// Probes each enabled instance and source and records the result for `/status`. A probe of
/// the same configuration under 30 seconds old is answered again unless `fresh=true`, and a
/// call waits for one running. `probe=false` reads the database alone: instances read
/// `unchecked`, sources `connected: null`, and `warnings` repeat the last probe.
#[utoipa::path(
    get,
    path = "/health",
    tag = "status",
    params(HealthQuery),
    responses(
        (status = 200, body = HealthResponse),
        (status = 503, description = "The database does not answer: `status` reads `failing`.",
body = HealthResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn health() {}

// ----------------------------------------------------------------- library

/// The categories, with what uses each
#[utoipa::path(
    get,
    path = "/categories",
    tag = "categories",
    responses((status = 200, body = Vec<CategoryWithUsage>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_categories() {}

/// The library, a page at a time
///
/// Every title Routarr read from the Arrs, with the category the rules chose
/// for it at the last simulation and the one an exception forces on it.
#[utoipa::path(
    get,
    path = "/media",
    tag = "library",
    params(MediaQuery),
    responses((status = 200, body = MediaPage))
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
/// Every rule the title was held against, each condition with what it expected and
/// found, and the winner's folder: a simulation's answer for one title, storing nothing.
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
/// Decided as a simulation decides, on each enabled Arr of the title's kind or the one
/// `instance` names, storing nothing. A title the library lacks is looked up in the Arr
/// (Radarr by TMDB or IMDb id, Sonarr by TheTVDB or IMDb id) and its `action` is `add`.
/// `unanswered_fields` lists what the rules read and no source answered, which `enrich=true`
/// asks the sources for. 404 when no instance knows the title.
#[utoipa::path(
    get,
    path = "/route",
    tag = "library",
    params(RouteQuery),
    responses((status = 200, body = Placement))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn place_title() {}

// --------------------------------------------------------------- proposals

/// The decisions, a page at a time
///
/// A pending decision is a move a simulation proposed. `status=pending` with
/// `simulation_id` lists what one simulation proposed.
#[utoipa::path(
    get,
    path = "/decisions",
    tag = "decisions",
    params(DecisionQuery),
    responses((status = 200, body = DecisionPage))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_decisions() {}

/// Run the rules over the library
///
/// With `persist: true` the decisions are stored under the `simulation_id` answered,
/// replacing the pending ones. One persisting run goes at a time, and another answers 409.
/// A preview, `persist: false`, waits its turn, and answers 409 while four run or wait.
#[utoipa::path(
    post,
    path = "/simulate",
    tag = "decisions",
    request_body = SimulationRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses((status = 200, body = SimulationResult), (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),)
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn simulate() {}

/// Apply chosen decisions
///
/// Asks the Arr to move each title, behind the interface's guardrails. A decision no longer
/// pending is skipped, so a repeat moves nothing. `move_files` needs a key that may move files.
#[utoipa::path(
    post,
    path = "/decisions/apply",
    tag = "decisions",
    request_body = ApplyDecisionsRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = ApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),
        (status = 409, description = "A guardrail asks for a confirmation, named in `confirm`, \
another apply is running, or the global dry run is on.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn apply() {}

/// Apply everything a simulation proposed
///
/// Every move, or those a filter keeps, in slices of the batch limit, stopping at the first
/// that fails. The `batch` guardrail always asks first, its question stating how many titles
/// move and the filter that chose them.
#[utoipa::path(
    post,
    path = "/decisions/apply-all",
    tag = "decisions",
    request_body = ApplyAllRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = BatchApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),
        (status = 409, description = "The `batch` confirmation is asked, another apply is \
running, or the global dry run is on.", body = ErrorResponse),
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
    tag = "decisions",
    request_body = RevertDecisionsRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = ApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),
        (status = 409, description = "A guardrail asks for a confirmation, named in `confirm`, \
an apply is running, or the global dry run is on.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn revert() {}

/// Move back every title a filter matches
///
/// The applied moves a revert may still undo that the filter, the simulation and the times
/// keep, in slices of the batch limit. The `batch` guardrail always asks first, as for an
/// apply of a whole simulation.
#[utoipa::path(
    post,
    path = "/decisions/revert-all",
    tag = "decisions",
    request_body = RevertAllRequest,
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = BatchApplyReport),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),
        (status = 400, description = "No move the filter matches can be reverted.",
body = ErrorResponse),
        (status = 409, description = "The `batch` confirmation is asked, an apply is running, \
or the global dry run is on.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn revert_all() {}

// -------------------------------------------------------------- exceptions

/// The exceptions
///
/// A page of them, the newest first.
#[utoipa::path(
    get,
    path = "/overrides",
    tag = "exceptions",
    params(PageQuery),
    responses((status = 200, body = ExceptionPage))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_exceptions() {}

/// Force a title's category
///
/// Replaces the title's exception if it has one. The pending decisions for the
/// title are withdrawn, since the exception now decides.
#[utoipa::path(
    post,
    path = "/overrides",
    tag = "exceptions",
    request_body = CreateOverrideRequest,
    responses((status = 201, body = OverrideEntry,
headers(("Location" = String, description = "Where the new exception is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn set_exception() {}

/// Keep a title where it is
///
/// Pins the title to the category of the root folder it is in, so no run proposes to move it,
/// and decides it again at once.
#[utoipa::path(
    post,
    path = "/media/{id}/keep",
    tag = "exceptions",
    params(("id" = String, Path, description = "The title's media id.")),
    request_body = KeepRequest,
    responses(
        (status = 201, body = OverrideEntry,
headers(("Location" = String, description = "Where the exception is found."))),
        (status = 409, description = "`reason` is `no_current_folder`, or `folder_unmapped` when \
the folder leads to no category.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn keep_title() {}

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

/// Force a title's category by the id another service gives it
///
/// For an application that knows the title by its TMDB, TheTVDB or IMDb id. Forces every
/// copy the library holds, one per instance, or only the one on `instance`, replacing its
/// exception and withdrawing its pending decisions. 404 when no copy is in the library.
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

/// Remove a title's exception by the id another service gives it
///
/// On every copy of the title, or the one on `instance`. `deleted` is false when none
/// had one. 404 when no copy is in the library.
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
    responses((status = 200, body = JobPage))
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
/// Answers once every instance is read, one report each. One that fails is reported with
/// its `error` and stops none of the others. With `respond-async` the task holds the
/// reports when it ends, and each instance is also read under a task of its own.
#[utoipa::path(
    post,
    path = "/instances/sync",
    tag = "instances",
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its reports."),),
    responses((status = 200, body = Vec<SyncReport>), (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),)
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn sync_all() {}

/// Read one Arr again
///
/// A sync of an instance already running answers 409, and so does a disabled
/// instance, with the reason `instance_disabled`.
#[utoipa::path(
    post,
    path = "/instances/{id}/sync",
    tag = "instances",
    params(("id" = String, Path, description = "The instance's id."), ("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses((status = 200, body = SyncReport), (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),)
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn sync_instance() {}

/// The Arrs Routarr reads
///
/// Their address, kind and last sync, never the Arr's key. For an application key
/// `webhook_url` is `null`, since its token posts as the Arr, and `api_key_masked` empty.
#[utoipa::path(
    get,
    path = "/instances",
    tag = "instances",
    responses((status = 200, body = Vec<InstanceResponse>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_instances() {}

/// One Arr
#[utoipa::path(
    get,
    path = "/instances/{id}",
    tag = "instances",
    params(("id" = String, Path, description = "The instance's id in Routarr.")),
    responses((status = 200, body = InstanceResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn get_instance() {}

/// The metadata sources, and what each can answer
///
/// Each source in the configured order, whether it is enabled, whether it
/// needs a key and holds one, and the fields it answers. Never the key.
#[utoipa::path(
    get,
    path = "/metadata/providers",
    tag = "status",
    responses((status = 200, body = ProvidersResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_sources() {}

/// The values the library holds, for each condition
///
/// The genres, languages, countries, certifications, tags, series types and folders the
/// library holds, each with its title count, and the closed lists of languages and countries.
#[utoipa::path(
    get,
    path = "/media/facets",
    tag = "library",
    responses((status = 200, body = LibraryFacets))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn library_facets() {}

// -------------------------------------------------------------------- logs

/// What every move did, a page at a time
///
/// Each move and revert Routarr asked an Arr for, with its outcome, newest
/// first.
#[utoipa::path(
    get,
    path = "/logs",
    tag = "tasks",
    params(LogQuery),
    responses((status = 200, body = LogPage))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_logs() {}

/// The same log as a CSV file
///
/// The filters of `/logs` without its paging, up to the newest 50 000 rows. A cell that
/// would start a spreadsheet formula starts with an apostrophe, read as text.
#[utoipa::path(
    get,
    path = "/logs/export",
    tag = "tasks",
    params(LogFilter),
    responses((status = 200, description = "The log as CSV.", content_type = "text/csv",
headers(("X-Routarr-Truncated" = String, description = "`true` when the log holds more rows \
than the export."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn export_logs() {}

// ----------------------------------------------------------------- backups

/// The backups kept
///
/// Their names, sizes and dates. Downloading or restoring one stays the
/// owner's: an archive holds the key every stored credential is sealed with.
#[utoipa::path(
    get,
    path = "/backups",
    tag = "backups",
    responses((status = 200, body = BackupListResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_backups() {}

/// Take a backup now
///
/// Before a large apply, for instance. The oldest past the retention count
/// is removed.
#[utoipa::path(
    post,
    path = "/backups",
    tag = "backups",
    params(("Prefer" = Option<String>, Header, description = "`respond-async` answers 202 once the \
task has started, instead of its report."),),
    responses(
        (status = 200, body = BackupFile),
        (status = 202, description = "The task has started, as `Prefer: respond-async` asked.",
body = Accepted, headers(("Location" = String, description = "The task, under `/jobs`."),
("Preference-Applied" = String, description = "`respond-async`."),
("Retry-After" = u32, description = "Seconds to wait before the first look at the task."))),
        (status = 409, description = "A backup is already running, or one was taken this \
second.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn take_backup() {}

// ------------------------------------------------------------------- rules

/// The rules, in priority order
#[utoipa::path(get, path = "/rules", tag = "rules", responses((status = 200, body = Vec<Rule>)))]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_rules() {}

/// One rule
#[utoipa::path(
    get,
    path = "/rules/{id}",
    tag = "rules",
    params(("id" = String, Path, description = "The rule's id.")),
    responses((status = 200, body = Rule))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn get_rule() {}

/// Create a rule
///
/// Refused with 400, as `/rules/validate` would say, when its target category is missing, a
/// condition has no value or no enabled source answers one. It counts from the next simulation.
#[utoipa::path(
    post,
    path = "/rules",
    tag = "rules",
    request_body = CreateRuleRequest,
    responses((status = 201, body = Rule,
headers(("Location" = String, description = "Where the new rule is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn create_rule() {}

/// Replace a rule
#[utoipa::path(
    put,
    path = "/rules/{id}",
    tag = "rules",
    params(("id" = String, Path, description = "The rule's id.")),
    request_body = CreateRuleRequest,
    responses((status = 200, body = Rule))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn update_rule() {}

/// Remove a rule
#[utoipa::path(
    delete,
    path = "/rules/{id}",
    tag = "rules",
    params(("id" = String, Path, description = "The rule's id.")),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn remove_rule() {}

/// Copy a rule, disabled
#[utoipa::path(
    post,
    path = "/rules/{id}/duplicate",
    tag = "rules",
    params(("id" = String, Path, description = "The rule to copy.")),
    responses((status = 201, body = Rule,
headers(("Location" = String, description = "Where the new copy is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn duplicate_rule() {}

/// Put the rules in a new order
///
/// Every rule's id, highest priority first. The priorities are numbered again
/// in steps of ten.
#[utoipa::path(
    post,
    path = "/rules/reorder",
    tag = "rules",
    request_body = ReorderRulesRequest,
    responses((status = 200, body = Reordered))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn reorder_rules() {}

/// The conditions a rule may use
///
/// Each condition with its value type, media types, the metadata field it reads, whether
/// an enabled source answers it, and the facet that lists its values.
#[utoipa::path(
    get,
    path = "/rules/conditions",
    tag = "rules",
    responses((status = 200, body = ConditionCatalog))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn condition_catalog() {}

/// What each rule decides
///
/// Rules a broader one above them shadows, rules identical to another, and
/// rules that match no title of the library.
#[utoipa::path(
    get,
    path = "/rules/health",
    tag = "rules",
    responses((status = 200, body = RuleHealthReport))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn rule_health() {}

/// The rules as a bundle
///
/// What `/rules/import` takes back, here or on another installation.
#[utoipa::path(get, path = "/rules/export", tag = "rules", responses((status = 200, body = RuleBundle)))]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn export_rules() {}

/// Import a bundle of rules
///
/// Each rule is judged as `POST /rules` judges it, and one refused is
/// skipped with its reason. `replace` removes the rules in place first.
#[utoipa::path(
    post,
    path = "/rules/import",
    tag = "rules",
    request_body = ImportRulesRequest,
    responses((status = 200, body = RuleImportReport))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn import_rules() {}

/// Judge a draft rule, storing nothing
#[utoipa::path(
    post,
    path = "/rules/validate",
    tag = "rules",
    request_body = CreateRuleRequest,
    responses((status = 200, body = RuleVerdict))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn validate_rule() {}

/// What a draft rule would move, storing nothing
///
/// The library evaluated with and without the draft, which replaces
/// `rule_id` when it is given: the titles whose destination would change.
#[utoipa::path(
    post,
    path = "/rules/preview",
    tag = "rules",
    request_body = PreviewRequest,
    responses((status = 200, body = PreviewResponse))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn preview_rule() {}

/// The rule templates
///
/// Ready-made rule sets for the usual library splits, in the reader's
/// language: what each suggests, what it asks, and whether one is added.
#[utoipa::path(
    get,
    path = "/rules/templates",
    tag = "rules",
    responses((status = 200, body = Vec<TemplateSummary>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_templates() {}

/// What adding a template would change, storing nothing
#[utoipa::path(
    post,
    path = "/rules/templates/{id}/preview",
    tag = "rules",
    params(("id" = String, Path, description = "The template.")),
    request_body = TemplateRequest,
    responses((status = 200, body = TemplatePreview))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn preview_template() {}

/// Add a template's rules
///
/// After the rules written by hand, among the templates added in catalogue order, creating
/// the categories it names. One already added answers 409 `template_added` unless `replace`.
#[utoipa::path(
    post,
    path = "/rules/templates/{id}/import",
    tag = "rules",
    params(("id" = String, Path, description = "The template.")),
    request_body = TemplateRequest,
    responses((status = 201, body = TemplateAdded,
headers(("Location" = String, description = "The rules."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn import_template() {}

/// The rule tests
///
/// Each pins the category a title should get, as the title was when it was
/// pinned. A page of them, by name.
#[utoipa::path(
    get,
    path = "/rule-tests",
    tag = "rules",
    params(PageQuery),
    responses((status = 200, body = RuleTestPage))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_rule_tests() {}

/// Pin what the rules decide for a title as a test
#[utoipa::path(
    post,
    path = "/rule-tests",
    tag = "rules",
    request_body = NewRuleTest,
    responses((status = 201, body = RuleTest,
headers(("Location" = String, description = "Where the new test is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn create_rule_test() {}

/// Remove a rule test
#[utoipa::path(
    delete,
    path = "/rule-tests/{id}",
    tag = "rules",
    params(("id" = String, Path, description = "The test's id.")),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn remove_rule_test() {}

/// Run every rule test against the rules as they are
#[utoipa::path(
    post,
    path = "/rule-tests/run",
    tag = "rules",
    responses((status = 200, body = RuleTestRun))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn run_rule_tests() {}

// -------------------------------------------------------------- categories

/// Create a category
#[utoipa::path(
    post,
    path = "/categories",
    tag = "categories",
    request_body = CreateCategoryRequest,
    responses((status = 201, body = Category,
headers(("Location" = String, description = "Where the new category is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn create_category() {}

/// Rename a category
///
/// Every rule, mapping, exception and test naming it follows, in one
/// transaction.
#[utoipa::path(
    put,
    path = "/categories/{id}",
    tag = "categories",
    params(("id" = String, Path, description = "The category's id.")),
    request_body = RenameCategoryRequest,
    responses((status = 200, body = Category))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn rename_category() {}

/// Remove a category
///
/// Refused with 409 while a rule, a mapping, an exception or a test names it,
/// and with 400 for the fallback category.
#[utoipa::path(
    delete,
    path = "/categories/{id}",
    tag = "categories",
    params(("id" = String, Path, description = "The category's id.")),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn remove_category() {}

/// The root folders, and the category each leads to
///
/// The folders each Arr reports and the destinations declared beside them,
/// with their free space, reachability and category.
#[utoipa::path(
    get,
    path = "/root-folders",
    tag = "categories",
    responses((status = 200, body = Vec<RootFolderWithInstance>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn list_root_folders() {}

/// What the mappings leave unsaid
///
/// Two folders of one instance claiming one category, a rule's category with no folder on an
/// instance it reaches, a folder the Arr did not reach last time, a mapping to a removed one.
#[utoipa::path(
    get,
    path = "/root-folders/conflicts",
    tag = "categories",
    responses((status = 200, body = Vec<MappingConflict>))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn mapping_conflicts() {}

/// Declare a destination the Arr does not list
///
/// Written as the Arr sees it: `/data/movies/4k` in a Linux container,
/// `D:\Movies\4K` or `\\nas\films` on Windows.
#[utoipa::path(
    post,
    path = "/root-folders",
    tag = "categories",
    request_body = DeclareRootFolder,
    responses((status = 201, body = Declared,
headers(("Location" = String, description = "Where the new destination is found."))))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn declare_destination() {}

/// Remove a declared destination
///
/// Refused with 409 for a folder an Arr reports: the next sync would bring it
/// back, without its category.
#[utoipa::path(
    delete,
    path = "/root-folders/{id}",
    tag = "categories",
    params(("id" = String, Path, description = "The folder's id.")),
    responses((status = 200, body = Deleted))
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn remove_destination() {}

/// Lead a folder to a category, or to none
#[utoipa::path(
    put,
    path = "/root-folders/{id}/category",
    tag = "categories",
    params(("id" = String, Path, description = "The folder's id.")),
    request_body = UpdateRootFolderCategory,
    responses(
        (status = 200, body = Mapped),
        (status = 409, description = "`reason` is `category_mapped` when another folder of the \
instance already leads to the category and `add_to_set` is not true.", body = ErrorResponse),
    )
)]
#[expect(dead_code, reason = "a route's documentation, never called")]
fn map_folder() {}
