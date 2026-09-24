---
paths:
  - "backend/**"
---

# Backend

## Handlers

- Axum 0.8 writes a path parameter `{id}`, never `:id`.
- The live API key is `state.api_key()`. `config.api_key` holds only `ROUTARR_API_KEY`.
- A write a person asked for takes `Extension<Identity>` and records
  `Attribution::manual(identity.actor())`. The scheduler and the webhook pass
  `Attribution::unattended(trigger)`. Mind the names: `Identity::actor()` fills the `subject`
  column, and the `actor` column of `decisions` and `execution_logs` holds the trigger.
- `scripts/smoke-image.sh` looks for two log lines of `backend/src/main.rs`, `Generated an API
  key` and `Routarr stopped cleanly`: reworded, either fails the image check in CI.
- A refusal the user must read is `BadRequest`, `NotFound` or `Conflict`. `Database`,
  `Serialization`, `Config` and `Internal` log their text and answer a generic 500
  (`backend/src/error.rs`).

## One answer per question

Each of these questions has one function. Call it, never spell the question again:

- where an item goes: `routing::route`, shared by the simulation and the executor's
  revalidation (`routing::current_targets`)
- whether an item has matchable metadata: `api::media::metadata_predicate`
- every warning: `offline_warnings` in `backend/src/api/health.rs`, returned by both `/status`
  and `/health`. `/status` is polled and never probes. A finding only a probe can make reaches
  it through the `probe_results` table, which `/health` writes.
- a stored timestamp: `routing::format_timestamp` and `routing::parse_timestamp`
- a category name: `api::categories::normalise`, run by every writer of `categories`
- a folder path: `rule_engine::normalize_path` in Rust, `rtrim(path, '/')` in SQL. An Arr
  reports a root folder with or without its trailing slash.

## Data and SQL

- `backend/migrations/001_initial_schema.sql` is part of v0.1.0. Change the schema in a new
  file listed in `MIGRATIONS` (`backend/src/db.rs`): `_migrations` records names, so an edit to
  a shipped file never reaches an existing database.
- Timestamps are TEXT in `%Y-%m-%d %H:%M:%S` UTC, the shape `datetime('now')` writes, so both
  compare as strings. Booleans are INTEGER.
- A tuple `query_as` target binds by position and stops at 16 fields. Use a named `FromRow`
  struct for a wide row or one likely to grow.
- Write `AssertSqlSafe` at each call site, never inside a helper, so `grep AssertSqlSafe` lists
  every dynamic statement. Splice only consts and literal fragments and bind every value:
  `db::placeholders` for an `IN` list, chunked by `routing::BIND_CHUNK`, and `db::escape_like`
  for user text in a `LIKE ? ESCAPE '\'`. A `&'static str` parameter needs no marker
  (`maintenance::delete_older_than`).
- A new `media` column goes in a migration, the `Media` struct, `MEDIA_COLUMNS` in
  `backend/src/services/routing.rs` (reads) and `upsert_media` in
  `backend/src/services/sync.rs` (writes). `FromRow` maps by name, so a column missing from
  either fails at run time on that one path.
- A column holding a category name is called `category` or `*_category`. A new one needs its
  `UPDATE` in the statement list of `api::categories::rename` and a row in
  `seed_every_reference` (`backend/src/tests/categories.rs`), whose test finds the columns by
  that name and fails until both are there. A column named otherwise escapes both.
- Declared and synced folders share `root_folders`, told apart by `origin`: a second table
  would put a `UNION` in `routing::load_context` and in every executor join.
- A `root_folders` row with `origin = 'declared'` belongs to the operator. A cleanup of folders
  an Arr stopped reporting stays scoped to `origin = 'arr'`, or it deletes the declared row and
  the category mapped onto it. A declared path an Arr adopts is promoted in place (`do_sync`),
  keeping one row per path.
- An apply stamps `media.moved_at`. Code writing the path columns from an Arr read takes its
  `read_at` before the first request and goes through `upsert_media`, which keeps a newer move.
  The `apply` and `sync:{instance}` job locks are distinct, so nothing else closes that race.
- A rule test stores a snapshot (`Media`, merged metadata, `now`), never a `media_id`: a sync
  deletes the rows an Arr stops returning, and a case tied to one would go with it.
- The default `ROUTARR_DB_PATH` is relative: `cargo run` in `backend/` opens `backend/data/`,
  another library than the release binary run from the repository root. A test relocates a
  config with `Config::set_db_path`, never by assigning `db_path` alone.

## Rules and conditions

- A new `Condition` variant needs `kind`, `is_empty` and `metadata_field` in
  `backend/src/models/rule.rs` and an arm in `evaluate_single_condition` (the compiler finds
  all four), a `CONDITIONS` entry in `backend/src/api/conditions.rs` (its tests find a missing
  one), and a `ConditionLabel<PascalKind>` key in `backend/locales/en.json`, which nothing
  checks. The frontend builder reads the catalogue and changes only for a new `value_type`.
- The values of a plain condition are alternatives. The `_all` variants
  (`genre_contains_all`, `keyword_contains_all`, `origin_country_all`, `tag_in_all`) require
  every value, and nothing reads an AND out of a separator. Compare through `contains_any` and
  `contains_all`, which fold case, accents and punctuation (`normalise_value`) and invent no
  synonym.

## Sources and outbound HTTP

- Evaluation reads the configured order (`metadata_order`) whatever the keys, so a cached
  answer keeps counting after its key is removed. Fetching and the "no source can answer"
  warnings read `metadata_providers`, the subset able to answer today.
- A new source needs a `PROVIDERS` entry with its `Addressing` and a `FetchingSource` variant,
  whose matches the compiler finds. These fall through silently when missed:
  `FetchingSource::resolve` for a `Search` source, `FetchingSource::rate` for a paced public
  endpoint, `AppState::metadata_sources`, and for a keyed source `provider_key_from` and
  `provider_keys_from`.
- A source answers in Routarr's vocabulary: an ISO 639-1 language code and an ISO 3166-1
  alpha-2 country code, converted through `backend/src/integrations/language.rs`. A name or a
  three-letter code stored as it came matches no rule written against `ja` or `JP`.
- A sealed setting is `Kind::Secret` in `KNOWN` (`backend/src/api/settings.rs`) and its key
  ends in `_api_key`: `maintenance::reseal_secrets` selects settings by that suffix, so a
  secret named otherwise does not follow a master-key rotation. A source's key is
  `<id>_api_key`, the name `provider_key_from` builds.
- Every call uses the shared `state.http`, never `reqwest::Client::new()`: the shared client
  carries the timeout and the same-origin redirect policy that keeps `X-Api-Key` from leaking.
- Send through `integrations::send_json` or `send_ok`. Never format a `reqwest::Error` with
  `Display`: it embeds the URL, and a TMDb URL carries `?api_key=`.

## Localization

- Translate a refusal naming something the operator typed, since it is read under that field.
  An error about an id the interface sent stays English. `backend/src/tests/localization.rs`
  pins both sides.
- A new key needs `en.json` alone, since every language falls back to English key by key.
  `scripts/check-locales.py` fails on a key the code uses and `en.json` lacks, a key another
  language has and English lacks, a changed `{placeholder}`, an empty value, a key no source
  file quotes, and a language below 50%.
- `python3 scripts/add-locale.py <code> < batch.json` merges translations into
  `backend/locales/<code>.json` and refuses a lost or invented `{placeholder}`.
- The dictionaries are `include_str!`'d. When a locale edit is missing from a fresh build,
  `touch backend/src/localization.rs` and build again.

## Tests

- `TestApp` (`backend/src/tests/mod.rs`) drives the real `Router`, middleware included,
  against an in-memory database. `TestApp::new()` runs with auth off and
  `TestApp::with_api_key(key)` turns it on. The router holds the state it is built from, so a
  test that changes `app.state` rebuilds with `TestApp::around(state)`. Fixtures:
  `seed_library`, `seed_anime_rule`, `seed_instance_at`.
- Anything touching `backend/src/integrations/`, the sync or the executor runs against an
  in-process stand-in on an ephemeral port (`fake_arr.rs`, `fake_tmdb.rs`, `fake_sources.rs`,
  `fake_oidc.rs` in `backend/src/tests/`) and asserts on what it recorded.
  `FakeArr::failing(status)` drives the error paths.
- A test asserting that nothing happened needs a positive control proving the fixture can
  make it happen (`ready_to_apply` in `backend/src/tests/scheduler.rs`).
- `backend/src/tests/scale.rs` counts a simulation's queries at two library sizes. A query per
  item fails there and nowhere else, since every other test seeds one item.
- The webhook is the one route that reaches the library without the API key. No body may
  produce a 500 or a panic (`backend/src/tests/webhook_fuzz.rs`), and a bad token answers a
  404 that does not confirm the instance exists (`backend/src/tests/security.rs`).
- `backend/src/tests/live_sources.rs` (the real AniList, Jikan, OMDb and TheTVDB) stays
  `#[ignore]`d and out of CI: a green build never depends on a third party's uptime. Their
  clients keep captured-payload tests beside them, which run in CI.
