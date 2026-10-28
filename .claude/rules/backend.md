---
paths:
  - "backend/**"
---
# Backend

## Handlers

- Axum 0.8 writes a path parameter `{id}`, never `:id`.
- The live API key is `state.api_key()`. `config.api_key` holds only `ROUTARR_API_KEY`.
- A write somebody asked for takes `Extension<Identity>` and records `identity.attribution()`:
  the `manual` trigger for a person, `api` and its name for an application key. The scheduler and
  the webhook pass `Attribution::unattended(trigger)`. Mind the names: `Identity::actor()` fills
  the `subject` column, and the `actor` column of `decisions` and `execution_logs` holds the
  trigger.
- `scripts/smoke-image.sh` looks for `Generated an API key` (`backend/src/main.rs`) and `Routarr
  stopped cleanly` (`backend/src/process.rs`) in the log: reworded, either fails the image check.
- A new route is the owner's until `GRANTS` in `backend/src/api/applications.rs` names it with a
  scope, and a granted route is one an outside application relies on. A handler that moves files
  runs the executor through `on_behalf_of` (`backend/src/api/decisions.rs`), which holds an
  application key to the guardrails and file moves it was given.
- A granted route needs its operation in `backend/src/api/contract.rs`, and the types it takes
  and returns derive `utoipa::ToSchema`. Their `///` comments are published in
  `backend/openapi/v1.json`, so a remark for Routarr's developers goes in a `//` comment. A change
  to one of those types is pinned with `ROUTARR_WRITE_CONTRACT=1 cargo test contract`, and
  `scripts/check-api-breaks.sh` refuses a removal, a rename or a retype against the last release
  that `scripts/api-breaks/since-<tag>.txt` does not list, a list kept before 1.0 only.
- A type read from a body or a query takes `#[serde(deny_unknown_fields)]`, flattened ones too:
  ignored, a misspelled field changes what is written. A query is one struct of its own fields,
  since a flattened query reads every value as text. A create answers `api::Created`, a removal
  `api::Deleted`, and a timestamp leaves through `crate::timestamp` with
  `#[schema(format = DateTime)]` (`backend/src/tests/contract.rs` finds each one missed).
- A route that starts one job answers through `api::tasks::answer`: the work runs on its own task
  whatever the caller does, and `Prefer: respond-async` gets 202 once the job has started. Every
  question and refusal comes before `JobRegistry::start`, and the work calls `JobHandle::report`
  before the outcome, so a finished job always carries the report its call answers.
- A refusal the user must read is `BadRequest`, `NotFound`, `Conflict` with the `reason` a script
  branches on, `InProgress` when a running job holds what the call needs (409 and `Retry-After`,
  which the scheduler skips), and `Forbidden` what a known caller may not do. `Database`,
  `Serialization`, `Config` and `Internal` log their text and answer `internal_error`.

## One answer per question

Each of these questions has one function. Call it, never spell the question again:

- where an item goes: `routing::route`, shared by the simulation, the executor's revalidation
  (`routing::revalidated_targets`), the explanation and `GET /route` (`routing::route_one_with`,
  which takes a title the library does not hold as a media row it never writes)
- where the last run sent a title, moved or not: its `media_routing` row, which
  `routing::store_run` writes. The decisions hold only what a run proposed.
- whether an item has matchable metadata: `api::library::metadata_predicate`
- every warning: `offline_warnings` in `backend/src/api/health.rs`, returned by both `/status`
  and `/health`. `/status` is polled and never probes. A finding only a probe can make reaches
  it through the `probe_results` table, which `/health` writes.
- a stored timestamp: `routing::format_timestamp` and `routing::parse_timestamp`
- a category name: `api::categories::normalise` to write one, `stored_form` to compare with one
- whether two folders are one, or one holds the other: `crate::paths` in Rust (`key`, `same`,
  `within`), `a = b COLLATE path` in SQL, registered on every pool by `db::with_paths`. An Arr
  closes a root folder with its separator, and one on Windows writes `D:\Media\` and reads a
  name whatever its case.

## Localization

- Translate a refusal naming something the operator typed, since it is read under that field.
  An error about an id the interface sent stays English. `backend/src/tests/localization.rs`
  pins both sides.
- A text a row keeps for later (a decision's reasons, a failed move, a sync status) is a `Told`,
  stored by `Told::stored` and said by `localization::said` in the language it is read in. An
  outbound failure carries its `Upstream` kind (`integrations::upstream`), which code branches on.

## Outbound HTTP

- Every call uses the shared `state.http`, never `reqwest::Client::new()`: the shared client
  carries the timeout and the same-origin redirect policy that keeps `X-Api-Key` from leaking.
- Send through `integrations::send_json` or `send_ok`. Never format a `reqwest::Error` with
  `Display`: it embeds the URL, and a TMDB URL carries `?api_key=`.

## Tests

- An address no test means to reach is port 1 on the loopback (`AN_INSTANCE`,
  `Config::for_tests`), never a host name: a name goes through the host's resolver.
- A test asserting that nothing happened needs a positive control proving the fixture can
  make it happen (`ready_to_apply` in `backend/src/tests/scheduler.rs`).
- The webhook is the one route that reaches the library without the API key. No body may
  produce a 500 or a panic (`backend/src/tests/webhook_fuzz.rs`), and a bad token answers a
  404 that does not confirm the instance exists (`backend/src/tests/security.rs`).
