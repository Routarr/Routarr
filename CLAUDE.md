# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Routarr is a self-hosted routing/classification layer for the Servarr ecosystem: it reads movies and series from Radarr/Sonarr, enriches them from an ordered list of metadata sources (the Arrs themselves, then TMDb), evaluates user-defined rules, and moves media to the correct *root folder* through the Arr APIs. Rust/Axum backend + Svelte 5/Vite frontend, shipped as one Docker image.

## Layout

Three deliverables, one per directory, and nothing shared but the repository:

```
backend/     the Rust crate — src/, migrations/, locales/, Cargo.toml
frontend/    the interface, built into frontend/dist and served by the backend
site/        the showcase site, deployed to Cloudflare Pages, never in the image
scripts/     the consistency checks CI runs, the locale tooling, the image smoke test
Dockerfile   builds backend + frontend into one image; `site/` is in .dockerignore
```

The crate sits in `backend/` rather than at the repository root so the three
deliverables are visibly peers and none of them looks like it owns the whole
thing. The shape is load-bearing well past the source tree: the Dockerfile, the
CI path filters, the shell harnesses and the documentation links all address it,
so moving it is not a rename.

## Commands

Backend (`backend/`):
```bash
cargo run                   # http://0.0.0.0:9876, reads .env via dotenvy
cargo test                  # 700-odd tests, all offline (in-memory SQLite); 5 more are live and opt-in
cargo llvm-cov --summary-only --ignore-filename-regex 'src/(tests/|main\.rs)'  # 92% of lines, gated at 90 in CI
cargo test the_default_category_setting_is_honoured   # single test by name
cargo test services::rule_engine                      # one module
cargo clippy --all-targets -- -D warnings             # CI gate, must stay clean
cargo fmt
python3 ../scripts/check-locales.py    # run from anywhere; it resolves its own paths
python3 ../scripts/check-api-types.py  # the response structs against frontend/src/api/types.ts
python3 ../scripts/check-versions.py    # the toolchain versions, wherever they are written
```

Frontend (`frontend/`) — Svelte 5 + Vite:
```bash
npm install
npm run dev                 # Vite on :3000, proxies /api -> localhost:9876
npm run build               # svelte-check (the typecheck) + vite build -> frontend/dist
npm run check               # svelte-check alone
npm run lint                # ESLint, type-aware — dropped promises, stray console, type imports; CI gate
npm run coverage            # Vitest with a floor, measured over the whole tree
npm run format:check        # Prettier + prettier-plugin-svelte, CI gate
npm audit --audit-level=high # CI gate — the counterpart of `cargo audit`, on the whole tree
```

Full stack: `docker compose up -d --build`.

The dev container ([`.devcontainer/`](.devcontainer/)) prepares **both** npm trees — `frontend/` and `site/`, the latter since it became an Astro build — and provides everything the checks above need: the pinned Rust toolchain, Node 24 (matching CI and the production image — the Debian default is 18), python3 for the locale scripts, `cargo-audit` and `cargo-llvm-cov` as prebuilt binaries, `gh` for the repository and its CI, and `psmisc` for the `fuser` that `e2e/run.sh` needs. `postCreateCommand` installs npm dependencies and the Playwright browser, and both `postStartCommand` and `postAttachCommand` run [`prune.sh --daemon`](.devcontainer/prune.sh), which bounds the caches that otherwise only grow — superseded VS Code server builds, the extension download caches, and `backend/target`. It prunes in the hook's own process, an idle pass costing milliseconds, and arms a half-hourly loop on top: a process detached from a lifecycle hook does not reliably outlive the exec session that spawned it, so the detached half is the spare and not the mechanism. **It can delete build artefacts:** a cache that regenerates for free is cleared, one that costs compute (`incremental`, `llvm-cov-target`) is only touched once the target directory passes a ceiling, so an occasional slower rebuild is the script and not a fault. `e2e/run.sh` runs it after its release build for the same reason: one coverage pass, one test build and one release build together exceed the tmpfs, and the loop alone is too slow to catch that. `/cargo`, `/playwright`, `~/.claude`, `~/.config/gh` and `~/.vscode-server/extensions` are volumes, so a rebuild neither re-downloads them nor signs you out.

**The Rust build tree lives in RAM.** `runArgs` declares a 16 GiB tmpfs at `/ramdisk` — made by the container runtime, so nothing on the host is prepared, mounted or configured, and the container needs no privileges for it — and `CARGO_TARGET_DIR` points into it. `backend/target` is 4.8 GB of small files rewritten on every build, which is the worst thing a copy-on-write filesystem can be asked to hold; everything else measured is either too small to matter or too expensive to lose. `TMPDIR` is the mount point itself rather than a subdirectory, because processes that start before [`ramdisk.sh`](.devcontainer/ramdisk.sh) inherit it and a TMPDIR that does not exist yet fails `mktemp` in a way that reads as a broken image.

Everything in it is gone on stop, rebuild or recreate, and the price is one cold `cargo build`. What stays on disk stays for a reason: `/cargo` is a download, `/playwright` is a browser, `node_modules` is 533 MB of an already tight budget for a directory written once and read thereafter. `ramdisk.sh` asserts the mount is really a tmpfs and really the declared size — which it **reads out of `devcontainer.json`** rather than restating, since two copies of a figure drift the first time one is raised, and an assertion left on the old number warns on every start about a mount that is exactly what was asked for. The failure that matters is the silent one, where the mount is absent, every build still succeeds, and the writes land on the disk this exists to spare. Raising the size needs a container **rebuild**: the mount is made by the runtime at creation, so a running container keeps whatever it started with — which is what the size warning says out loud.

Because the target directory moves, nothing may assume where the binary is: [`e2e/run.sh`](frontend/e2e/run.sh) and [`screenshots/run.sh`](site/screenshots/run.sh) resolve `${CARGO_TARGET_DIR:-$ROOT/backend/target}`, which is also what keeps them working in CI, where the variable is unset. Both also **build from inside `backend/`** rather than pointing cargo at the manifest from where they happen to run: rustup resolves `rust-toolchain.toml` from the working directory, so `--manifest-path` compiles with whatever the default toolchain is — the one thing that file exists to prevent, and invisible for as long as the two versions agree. `prune.sh` measures a tmpfs tree by how **full** it is rather than how big — a full tmpfs does not slow a build down, it fails it with `ENOSPC` — and clears the cheap spill at 75% before touching `debug` at 90%. `bash .devcontainer/prune.sh --status` is the first thing to run when a build reports no space left; it also lists the VS Code server builds, which are ~700 MB each and the largest thing on the volume.

[RTK](https://github.com/rtk-ai/rtk) is installed in the image and hooked into Claude Code by `post-create.sh`: it rewrites Bash commands so the agent reads compressed output. It covers the **Bash tool only** — `Read`, `Grep` and `Glob` do not pass through it, so use shell equivalents when output size matters. Its telemetry is disabled through `RTK_TELEMETRY_DISABLED`, matching the rule the application itself follows: no outbound request nobody asked for. Installed from the GitHub release, never `cargo install rtk` — that name on crates.io is an unrelated project.

Edition 2024 / Rust 1.98. **[`backend/rust-toolchain.toml`](backend/rust-toolchain.toml)** is the version rustup reads — in CI, in the dev container and locally — and a CI on `stable` beside an image pinned to that version lets code pass every gate and fail only the release build.

It is not, however, the only place the number is written, and pretending otherwise is how it drifts: `Cargo.toml` states the MSRV and both Dockerfiles name a base image tag, so Rust appears four times and the Node major five. **[`scripts/check-versions.py`](scripts/check-versions.py)** is what makes them agree — it reads every one of them and fails naming the file left behind. It runs in the `changes` job, which is the only one with no path filter: a Dependabot `docker` pull request touches one `FROM` line, matches none of the three filters, and would otherwise land green while three files still hold the old version. Axum 0.8 — path params are `{id}`, not `:id`.

## Comments

State the constraint, not its history. A comment earns its place when it says
what would break if the code changed — the failure a guard exists for, an
external limitation, an invariant a refactor would quietly drop — and it says it
in the present tense. What was tried first, what a figure measured on the day,
and which change introduced it are the Git history's job.

A comment that exists because the code is unclear is a naming problem: prefer a
named constant, an extracted function or a more precise type, then delete the
comment. Nothing here should describe what a line does.

## Architecture

`api/` (HTTP handlers) → `services/` (business logic) → `integrations/` (external clients) → SQLite via `sqlx`.

The handlers are **not** uniformly thin, and the line is drawn on purpose rather than by accident: CRUD resources (categories, root folders, overrides, settings, instances) query from the handler, which is why `api/` holds more `sqlx::query` calls than `services/` does. What lives in `services/` is the logic that would still be logic without HTTP — simulation, application, sync, enrichment, rule evaluation, backup. A pass-through service for `GET /categories` would be ceremony; a route that decides what to move would not.

**Authentication is on by default.** With no `ROUTARR_API_KEY`, `main` generates one into `routarr.api_key` (0600, next to the database) and logs it once; `ROUTARR_AUTH=none` is the only way to run open. `Config::for_tests` leaves the mode at `None` so the unauthenticated cases stay testable. The e2e harness runs *with* a key and seeds it into the browser's storage, so the shipped posture is what the journeys exercise.

**The key is live state, not configuration.** `AppState.api_key` is an `RwLock` resolved once at startup and rewritten by `POST /auth/api-key`; `config.api_key` is only ever the *environment's* value. The environment wins over the stored file, which is the opposite of `provider_key` and deliberately so: a metadata key is one somebody typed into the interface, while this one is generated, so an operator pinning `ROUTARR_API_KEY` in their compose file after a first start must not find it ignored in favour of a file they never wrote. That precedence is also why rotation is refused while the variable is set — the new key would live until the next restart. `DELETE /auth/api-key` is refused in `apikey` mode, where it is the only way in, including for the request that would put it back.

**An identity is stored, not just resolved.** `Identity::actor()` returns the name worth recording — `None` for `none` and `external`, whose single anonymous subject names nobody — and `jobs::Attribution` carries it beside the trigger through the executor. The two are different questions: `actor` says *what* caused a write (`manual`, `schedule`, `webhook`), `subject` says *who* asked, and the scheduler only ever answers the first.

- **[main.rs](backend/src/main.rs)** — the whole route table, split into `public` (`/ping` only), `protected` (behind the API-key middleware) and `webhooks` (authenticated by a per-instance token in the path, because Radarr cannot send custom headers). Adding an endpoint means touching the `api/` module *and* this list.
- **[state.rs](backend/src/state.rs)** — `AppState` carries the pool, config, the shared `reqwest::Client`, the `SecretBox` and the job registry. Its `setting`/`bool_setting` helpers are how business logic reads the `settings` table; `adapter()` builds an Arr client with the decrypted key.
- **[services/rule_engine.rs](backend/src/services/rule_engine.rs)** — pure and synchronous, no I/O, `EvalContext` carries an injected `now` so time-relative conditions are testable. First match by ascending priority wins; ties break on name for determinism. An override short-circuits everything. Exclusions veto a rule that otherwise matched. Every condition emits a `ConditionOutcome` (expected + observed) even when it fails — that is the explainability contract the UI and `/media/{id}/explain` depend on.
- **[services/rule_health.rs](backend/src/services/rule_health.rs)** — which rules decide anything and which are unreachable. It also reports the two collisions the ordering cannot resolve: two rules sharing a name *and* a priority leave the winner to whatever id SQLite returns first, and two with identical conditions and target mean one decides nothing whatever the priorities say. Validation looks *inside* a rule; this is the only thing that looks between them, which is where first-match-by-priority puts its one trap. The engine already computed it — `Evaluation.alternatives` is the list of rules that matched and lost — and nothing aggregated it. A rule that never wins names the rule taking its items, because a bare zero is not actionable; matching nothing is reported apart, being a condition too narrow rather than a priority too low. `routing::evaluate_library` is the pass it uses: rule **ids**, no prose, no localizer, no writes.
- **[services/rule_tests.rs](backend/src/services/rule_tests.rs)** — pinned expectations, replayed against the rules as they stand. The preview answers "what would this change"; these answer "what must it *not* change", and first-match-by-priority means inserting one rule rebalances every rule below it. A case stores a **snapshot** of `EvalContext` — the serialised `Media`, the merged metadata, and the instant — never a `media_id`: `sync` deletes rows the Arr stops returning and that cascades, so a reference would vanish with the film it was written to protect. `now` is pinned for the same reason, being what `added_within_days` compares against. Overrides are deliberately *not* applied, or a pinned exception could mask a rule that had stopped working. `expected_category` is the fifth `%_category` column, so `categories::rename` updates it too.
- **[services/routing.rs](backend/src/services/routing.rs)** — the simulation. Everything is preloaded in a fixed number of queries (`load_context`) and decisions are written in one transaction; a query per item is the single biggest performance trap on this path. Rerunning marks prior `pending` decisions `superseded` so two contradictory proposals can never both be applied. Every whole-library pass — persisting or not, `evaluate_library` included — holds one of `MAX_CONCURRENT_LIBRARY_PASSES` permits from `library_pass` while it runs, so the third *waits*; `FULL_SIMULATION` is about who may write, not about how many may load.
- **[services/executor.rs](backend/src/services/executor.rs)** — the only code that writes to an Arr. Guardrails in order: global dry-run → batch limit → **reachability** → **capacity** → confirmation threshold → per-decision revalidation at apply time. The unattended path has nobody to ask, so `auto_apply::eligible_decisions` leaves a decision alone when its destination did not answer the last time anyone looked, rather than raising a question into an empty room. `guard_capacity` weighs the plan against `root_folders.free_space`, both of which the sync already stores; only bytes that *cross* a filesystem count, since two folders reporting the same free space are one volume where a move is a rename. It raises `ConfirmationRequired`, not a refusal — the same-volume test is evidence, not proof — and it runs **before** the threshold because a destination that cannot hold the plan is a graver thing to be told than a count. **A confirmation is named, not a boolean.** Each guardrail asks under a name from `executor::confirm` and the caller sends that name back in `Confirmed`, so answering one lifts one: a single flag meant confirming a capacity shortfall also waved the batch threshold through, silently, and the operator never saw the second fact. `Confirmed::all()` is `#[cfg(test)]` for that reason — in the application it would be the blanket flag this replaced. Moves are grouped by `(instance, target folder)` so Radarr's bulk editor is used. On success it also rewrites the local `media` row, otherwise the next simulation reproposes the same move — and stamps `moved_at`, because `apply` and `sync:{instance}` are **different** job locks, so a sync that read the Arr before the move could commit the old path back over it. `do_sync` captures `read_at` before its first request and `upsert_media` overwrites the two path columns only when that read is newer than `moved_at`. A lock is the other way to close that window and is deliberately not taken: it would refuse an apply for the whole duration of a sync, network read included.
- **[services/backup.rs](backend/src/services/backup.rs)** — `VACUUM INTO` for a consistent snapshot of a *live* database, zipped with the master key and the API key because a database without them restores nothing readable. A restore is **staged**, never applied in place: `apply_pending_restore` runs in `main` before the pool opens, since replacing the file under live connections is how a restore destroys what it recovers. `is_valid_backup_name` gates every route that turns a URL segment into a path — the archives sit in the same directory as the master key.
**A destination need not be a root folder the Arr reports.** `root_folders`
carries an `origin` — `arr` for a folder an instance lists, `declared` for one
typed into Routarr — because a target used to have to exist in Radarr or Sonarr
first, and the arrangement an operator wants is the opposite: one root folder
per Arr, and the targets beneath it named here. One table rather than two, since
`routing::load_context` builds the category map in a single query and the
executor joins on it; two would put a `UNION` in every reader, the hot path
`scale.rs` pins included. The sync's orphan cleanup is scoped to `origin = 'arr'`
— unscoped it deletes a declared row on the first pass, taking the category with
it — and a declared path the Arr later adopts is **promoted** rather than
duplicated, or the upsert would hit the path index and fail the whole instance.

A declared destination **inherits free space and reachability from the deepest
synced folder it sits under**, stamped onto the row by the sync so every reader
gets it without knowing the rule. That inheritance is the point: it is what
keeps `guard_capacity` and `guard_reachable` meaningful for a folder no Arr
reports. Under no known root it inherits nothing, and `guard_capacity` then
*asks* instead of skipping silently — for a synced folder a missing figure means
the Arr published none, but for a declared one it means nothing upstream will
catch a full disk either.

The path is checked against the **Arr's** filesystem (`/api/v3/filesystem`) by
listing its *parent* and looking for the leaf among the children — asked about the
path itself, that endpoint answers with the nearest directory above it, so every
misspelt last segment under a real root reads as verified. Not checked
against Routarr's own filesystem either: the two run in different containers as often as not, and a
path that exists here says nothing about the process that will do the writing.
An instance that cannot be asked does not block the save — refusing on an
unavailable probe locks the operator out at the worst moment — but the answer is
reported. Only a declared folder may be deleted here; one the Arr reports would
come back on the next sync without its category.

**An unreachable root folder is unknown, not gone.** An Arr reports a folder on
a NAS that has spun down as inaccessible, and `routing::load_context`
deliberately keeps it in the map: read as *absent* it unmapped its category, so
everything bound for it became `skip` — indistinguishable on screen from a
category nobody mapped — and the rerun then marked the previous plan
`superseded`, destroying a plan built while the disk was awake. Whether the
destination can be written to is asked at apply time instead, by
`guard_reachable`, which **asks rather than refuses**: a NAS that wakes on
access cannot be told from a dead disk. There is no grace period and no
threshold anywhere in this — `root_folders.last_accessible_at` records when the
folder last *answered* (`last_synced_at` is stamped on every row a pass writes,
including one just read as unreachable, so it would say "just now" for a disk
asleep for three days), and diagnostics states that date. Twenty minutes reads
as a nap and three days as a fault, and the operator is the one who knows their
hardware.

- **[services/sync.rs](backend/src/services/sync.rs)** — `update_sync_status` stamps `last_sync_attempt_at` always and `last_sync_at` **only on success**: writing a single column in both branches would let a failed attempt refresh it exactly as a success does, and the instance list would read "synchronised 2 minutes ago" beside an error badge, describing data two days old. One column says the scheduler is running, the other says the library is current. `sync_all_instances` runs instances through `buffered(SYNC_CONCURRENCY)`, not a `for` loop: `do_sync` fetches root folders, media and tags *before* it opens its transaction, so what overlaps is the network wait and the writes still land one at a time under SQLite's single writer. `buffered` rather than `buffer_unordered` — the reports are what the API returns and the interface lists. The bound sits below the pool's eight connections so a sync pass cannot starve the `/status` the interface is polling. It also stamps every row it writes with one `sync_token`, then deletes anything not carrying it. Skips that cleanup entirely when the Arr returns an empty list, since deleting media cascades to the user's overrides.
- **[services/metadata.rs](backend/src/services/metadata.rs)** — the metadata sources, their priority order and the merge. `arr` reads genres, original language and certification straight off the `media` row (Radarr and Sonarr return them in the payload the sync already reads, so it costs no request and needs no key); `tmdb` is fetched and cached. `MediaMetadata::merge` collapses them **field by field in the user's order** — first non-empty wins, the sources below fill the gaps — and records which source supplied each field, which is what `ConditionOutcome.source` reports. Reading uses the configured order (`state.metadata_order`) whatever the key situation, so a cached answer keeps working after a key is removed; fetching and the "no source provides this" warnings use `state.metadata_providers`, the usable subset. Adding a provider means a `PROVIDERS` entry (with its `Addressing`), a `FetchingSource` variant, and — for a source addressed by a search rather than by one of our ids — a `resolve` arm. Six ship: `arr`, `tmdb`, `anilist`, `jikan`, `omdb`, `tvdb`.
- **[services/rate_limit.rs](backend/src/services/rate_limit.rs)** — a reservation token bucket per source per pass. Concurrency bounds how many requests are *open*; this bounds how many are *made*. The balance is allowed to go negative so waiters are queued rather than all woken at once, and `Retry-After` outranks the computed pace. Only the *public* endpoints are paced: a base URL the operator changed is a mirror with its own limits.
- **[services/enrichment.rs](backend/src/services/enrichment.rs)** — drains the whole backlog with `buffer_unordered`, per fetched source, deduplicated by `(external_id, media_type)`.
- **[jobs/](backend/src/jobs/)** — `registry.rs` records jobs in SQLite and hands out in-memory locks — one fair permit per key: `try_lock` takes it if free, `lock_within` waits its turn for it within a budget — so the scheduler and a user cannot run the same sync twice; orphans left `running` by a crash are failed at startup. `scheduler.rs` honours each instance's own `sync_interval_minutes`.

**Everything on disk hangs off `Config::data_dir`** — the database, the master
key, the API key, the backups. It is derived once from `ROUTARR_DB_PATH`, whose
default (`./data/routarr.db`) is *relative*, so the working directory decides
which installation you open: `cargo run` from `backend/` and the release binary
from the repository root are two separate libraries. `main` logs the resolved
absolute directory at startup for that reason. It is derived once and never
inline from `db_path.parent()`, because `Path::new(":memory:").parent()` is
`Some("")` rather than `None` — an in-memory config would then produce
*relative* sibling paths and the scheduler suite would create `backend/backups`
in the source tree on every run. `set_db_path` moves both fields together;
assigning `db_path` alone leaves the keys behind.

### Data model conventions

**The fallback category is read through `AppState::default_category(pool)`** and nowhere else. Spelled at several sites it would acquire several fallbacks — `standard` in the engine and in `/media/{id}/explain`, the empty string in the category screen — and with no setting row the delete guard would compare a name against `""`, never fire, and leave the category routing actually lands in deletable. Migration 001 seeds the row and nothing removes it, so the fault would be latent; a test deletes the row and asserts the guard still refuses.

**Categories are free-form strings joined by name, with no foreign key** — `rules.target_category`, `root_folders.category`, `overrides.target_category` and `categories.name` all reference each other by value. Deleting a category in use is refused in `api/categories.rs` precisely because the database would not stop it. An unmapped category yields `action = 'skip'`, never an error. **The default category is stated once**, in the `default_category` setting — the badge and the delete guard read it too. Stating it twice, say with an `is_default` column beside it, gives two answers nothing keeps in step: the guard would protect the flagged category while the engine fell back to another one, therefore deletable. **Renaming** one is the mirror image: `categories::rename` updates the six places the name lives — this row, the four `%_category` columns, and the `default_category` setting — in one transaction, since nothing cascades. [src/tests/categories.rs](backend/src/tests/categories.rs) reads the columns out of `sqlite_master` rather than listing them, so a seventh one added by a migration fails there instead of leaving a screen pointing at a name nobody holds.

**Migrations are `include_str!`'d into the `MIGRATIONS` const in [db.rs](backend/src/db.rs)** — a new `.sql` file does nothing until it is listed there. Each runs in its own transaction, split by a parser that understands string literals, `''` escapes, line comments and `BEGIN … END` blocks.

**PRAGMAs live on `SqliteConnectOptions`**, not on one-off queries — otherwise they only configure whichever pooled connection served them.

sqlx is used **without compile-time macros**. Tuple `query_as` targets cap at 16 columns (that is why `DecisionRow` is a named `FromRow` struct), and column order must be kept in sync with the SQL by hand.

**A query string that is not `&'static str` needs `AssertSqlSafe`.** sqlx 0.9 refuses anything else at compile time, which is the right default: an interpolated value is how SQL injection happens. Every site here is one of three shapes, and all three are safe by construction — a `const` column list (`MEDIA_COLUMNS`, `RULE_COLUMNS`) spliced into a literal; filter fragments that are themselves literals, with every user value `.bind()`-ed; or a migration `include_str!`'d at compile time. The marker is deliberately written at each call site rather than hidden behind a helper: `grep AssertSqlSafe` is how a reviewer finds every string worth auditing. `maintenance::delete_older_than` shows the better answer where it is available — its parameter is `&'static str`, so no caller can reach it with a runtime string and there is nothing to audit.

**A source that knows none of our identifiers is resolved by search, once.** AniList and Jikan have no `tmdb_id`, `tvdb_id` or `imdb_id`, so enrichment searches by title and year and writes the answer to `source_identifiers` — *including when it found nothing*, which is what stops the next pass searching the whole library again for every live-action film. A match needs an identical normalised title and a year within one; anything less would attach another work's genres to a film, and a network error is never persisted as an answer. The key is the library's own identity (`tmdb:8392`, else `tvdb:…`, else `imdb:…`, else the normalised title and year), so the same film in two instances is searched for once.

**Each source answers in Routarr's vocabulary, not its own.** The Arrs report a language *name* (`Japanese`), TheTVDB a three-letter code (`jpn`), OMDb prose (`"Japan, United States"`); rules are written against `ja` and `JP`. [integrations/language.rs](backend/src/integrations/language.rs) converts, and
the two axes answer an unknown value differently on purpose. A **country** it
cannot map yields nothing: a wrong code silently misroutes, and an absent one
lets the source below answer. A **language** keeps its own lowercased name as a
last resort, because dropping it would lose a value a rule could still be
written against — the cost being that the value is then a name on an axis of
codes, and that `merge` takes it as the answer, so no source below fills the
gap. The table is thorough enough (53 languages, `flemish` and `chinese`
included) that the fallback is a rarity rather than the common case, which is
what makes the trade acceptable.

**"Has metadata" is asked in one place.** `api::media::metadata_predicate` is the SQL, and the library column, the diagnostics count and the warning beside it all splice it — three spellings of one question is how a badge ends up contradicting the number above it. It counts only the *enabled* sources, exactly as `routing::load_context` reads them; all three identifier namespaces, or a series enriched by TheTVDB and carrying no `tmdb_id` reads as undescribed for ever; and a matchable field, since a cached row is not a cached answer.

**"Has metadata" means *matchable*.** `MediaMetadata::merge` returns `None` unless a source contributed one of the five fields `MetadataField::ALL` names, and `api/media.rs`'s list predicate asks the same of a cache row: a synopsis, a status and a poster are readable by no condition, so an item holding only those is as blind to the engine as one holding nothing. Counting them made the library column promise metadata about an item no rule could touch — and the moment the pass stopped loading them, the column and the engine said opposite things about the same item.

**The library pass loads only what a rule can match on.** `MetadataField` names five fields, and `metadata::EVALUATED_COLUMNS` is what `load_cache` reads: the status, the synopsis and the poster are matchable by no condition, and read whole they were 53% of a cache row held in memory for every simulation. The per-item path behind the explanation panel keeps `CACHE_COLUMNS` and shows all three — one row is not worth a second query to trim. Neither list carries the three key columns: their only readers address a row by them and never read them back.

**The metadata cache is keyed by `(source, external_id, media_type)`**, the identifier being the one in *that* source's namespace — TMDb numbers them, OMDb keys on `tt…`. It is never keyed on `tmdb_id`: a cache keyed on one source's identifier makes a second provider impossible. The `arr` source is not in that table at all: its answer arrives with the sync and lives on the `media` row.

Adding a column to `media` means editing `MEDIA_COLUMNS` ([routing.rs](backend/src/services/routing.rs)) for the reads and `upsert_media` ([sync.rs](backend/src/services/sync.rs)) for the writes — one place each, on purpose. Spell either out at its call sites instead and `FromRow`, which maps by name, turns an omission into a runtime failure on the one path that runs it.

Each catalogue entry declares the **media types it can match on** — `tvdb_id_in`, `series_type_is` and `season_count_over` are `series` only, because Radarr's payload carries no `tvdbId`, no `seriesType` and no season list, so `upsert_media` leaves those columns null for every film. The builder narrows its *add* picker by the rule's own `media_type` and takes the **intersection** for a `both` rule, which is evaluated against films and series alike. It never narrows what it *renders*: a rule saved before its type was changed keeps its conditions on screen, since hiding one drops it from the next save without the user seeing what they lost. Two props, `addable` and `specs`, exist for exactly that distinction.

The catalogue lives in [api/conditions.rs](backend/src/api/conditions.rs), not in `api/rules.rs`: it is not a handler over the `rules` table but a static description of the vocabulary, and keeping it there makes `rules.rs` four unrelated things in one file.

`Condition` is a serde-tagged enum (`{"type": "genre_contains", "value": [...]}`) stored as JSON. Adding a variant means updating [models/rule.rs](backend/src/models/rule.rs) (`kind`, `is_empty`, `metadata_field`), `evaluate_single_condition`, and the `CONDITIONS` table in [api/conditions.rs](backend/src/api/conditions.rs) — the frontend rule builder is driven by that catalog, so it needs no change of its own.

**Four of those five are enforced, and by two different things.** `kind`,
`is_empty` and `evaluate_single_condition` are exhaustive matches, so the
compiler refuses a forgotten variant. So is `metadata_field`, now that it names
its seventeen media-row conditions instead of ending in a wildcard: falling
through to `None`, a new metadata-reading condition was silently exempt from the
warning that says no enabled source can answer it, and nothing asked the author
to decide. The catalogue is the one the compiler cannot reach — it is a `const`
in another module — so three tests in `api/conditions.rs` stand in for it: every
variant has an entry, every entry deserialises into the condition it names, and
`Spec.field` agrees with `Condition::metadata_field()`. The field is stated
twice on purpose, for two different readers, and disagreeing means the builder
offers a condition it calls fine while the evaluation reports it unanswerable.

**Several values in one condition are alternatives; several conditions are the AND.** `GenreContains(["Science Fiction", "Fantasy"])` matches either, and requiring both means two `genre_contains` conditions under `match_mode: all`. Nothing infers an AND from a separator, so a rule means the same thing however its values were typed. Values are compared through `rule_engine::normalise_value`, which folds case, accents and punctuation — `Science-Fiction` and `Science Fiction` are one value — and normalises rather than guessing: `Sci-Fi` is still not `Science Fiction`.

A `Spec` in the catalogue names the `/media/facets` axis its values come from (`suggestions`), and the builder renders a picker over that list instead of a text field. Empty where the values are not a closed set — a keyword, a title fragment, an external id — which is what selects the comma-separated input as the fallback.

Timestamps are TEXT (`datetime('now')` in SQL, `%Y-%m-%d %H:%M:%S` UTC from Rust). Booleans are INTEGERs.

`PUT /rules/reorder` rewrites priorities as `(index + 1) * 10`, so hand-set priorities in that range are clobbered by a UI drag-reorder.

**Rule names are not unique**, so the engine's ordering breaks ties on priority, *then* name, *then* id ([rule_engine.rs](backend/src/services/rule_engine.rs)). Name alone is not enough: the interface accepts the same name twice and duplication only appends "(copy)", so two rules can share both name and priority, and the winner would then be whatever SQLite returns first — for rules that may well target different categories.

### Outbound HTTP

One shared `reqwest::Client` ([http.rs](backend/src/http.rs)). Two properties are load-bearing and both are
tested: redirects are followed **only within the same origin** (scheme, host *and* port — every
homelab service sits on `localhost` behind a different port), because the Arr credential travels in
a custom `X-Api-Key` header that no client knows to strip on a cross-origin hop — the one exception
being an `http` → `https` upgrade on the same port or the canonical 80 → 443 (`stays_on_origin`),
since written as any `http` to any `https` it carried the key to whatever answers on another port;
and a transport
failure is described from the error's **source chain**, never from `reqwest`'s own `Display`, which
embeds the URL and therefore the TMDb `?api_key=`.

TLS verifies against the **platform's trust store**. reqwest 0.13.2 removed the `webpki-roots`
feature that compiled the Mozilla set into the binary, and the choice was made deliberately rather
than inherited: a homelab CA signing an internal Arr's certificate now works, which a compiled-in
root set refused. The price is that the roots come from the base image rather than from a Routarr
release, which makes `ca-certificates` in the [Dockerfile](Dockerfile) load-bearing — an image
without it fails *every* metadata call, not only the private ones. `query` is listed among the
features too: 0.13 moved query-string building behind its own.

### Shutdown

`db::checkpoint_and_close` runs after the server stops. In WAL mode a commit lands in
`routarr.db-wal`, and SQLite only checkpoints when the last connection closes — dropping the pool at
the end of `main` does not close its connections. Without this, a stopped Routarr left a 4 KB
database with no tables, and the documented backup restored exactly that.

### Secrets

Arr API keys are sealed with AES-256-GCM ([crypto.rs](backend/src/crypto.rs)) as `enc:v1:<base64>`. `SecretBox` holds an optional *previous* key (`ROUTARR_PREVIOUS_SECRET_KEY`) so values can be read with either during a rotation, and `maintenance::reseal_secrets` runs at startup to rewrite anything that is plaintext or still under the old key. A secret that opens with **neither** key is left untouched and logged — overwriting it would destroy the only copy.

Never log or return a decrypted key — `InstanceResponse` shows `•••• (encrypted)`, and only legacy plaintext keys get a partial mask.

**The metadata credentials are sealed the same way.** `tmdb_api_key`, `omdb_api_key` and `tvdb_api_key` are `Kind::Secret` settings: sealed in `api/settings.rs` on write so a plaintext value cannot reach the table whatever the caller sent, and never returned — `get_all` replaces them with an empty string and adds a `<key>_configured` boolean, which is what makes the field safe to leave untouched on the next save. `AppState::provider_key` resolves *saved first, environment as fallback*, so an existing deployment configured by its orchestrator keeps working and an empty saved value clears it back to the variable. `is_usable` takes the resolved map rather than `&Config` — a key from the interface counts exactly as much as one from the environment. `reseal_secrets` covers the `settings` table too; it scanned `instances` alone, and a master-key rotation would have left these unreadable, which surfaces only as conditions that quietly stop matching.

### Localization

Follows the Servarr convention: JSON dictionaries in [`backend/locales/`](backend/locales/) are embedded with `include_str!`, served by `GET /api/v1/localization` already merged over English, and referenced by key. The language is the `ui_language` **setting**, not a browser preference.

Anything the backend *produces* for a user is translated too — decision justifications, diagnostics, mapping conflicts, guardrail refusals, rule validation. **The line falls on who made the mistake.** A refusal that names something the operator *typed* and says what to change is content produced for them and is translated — it is read under the field that produced it, not in a response body. An id the interface itself sent is a client fault and stays English, as it does in Radarr and Sonarr; translating those would put the whole dictionary behind every 404. [src/tests/localization.rs](backend/src/tests/localization.rs) pins both sides of that line, or the next refusal added lands on whichever side its author happened to think of.

The rule engine never emits prose: `ConditionOutcome` carries a `key` plus `params`, and `Localizer::describe` renders it. Keep it that way — persisting pre-rendered sentences is what makes an explanation untranslatable.

**A size is named once, for both halves.** `localization::byte_units` and
`decimal_separator` sit beside `RTL_LANGUAGES` — locale *data*, not prose, so a
translator has nothing to decide and the dictionaries stay free of it. Both are
generated from `Intl.NumberFormat`, the same source
[api/format.ts](frontend/src/api/format.ts) reads at run time, which is what
keeps the two sides naming one figure one way: `executor::human_bytes` printed
`2.2 TiB` where the root-folders table printed `2,2 To` off the same division by
1024, on two screens an operator reads together. The byte symbol is *derived*
from the kilobyte's rather than asked for, on both sides — `Intl`'s narrow form
answers with a **word** in Dutch, Greek, Turkish, Korean and Traditional
Chinese, so every size under 1 KiB read `512 byte` in five of the languages
shipped.

**Counted strings carry no plural suffix.** `t()` substitutes `{placeholder}` and nothing else — no plural forms, by design — so a string like `{count} warning(s)` is a crutch that reads badly in English and is simply wrong in the languages with three or more forms, which Arabic, Russian, Polish, Czech and Slovak all are. Every count is therefore phrased so the number never sits against an inflected noun: `Warnings: {count}`, `Decisions applied: {applied} of {requested}`, `Age in days: {value} or less`. That construction is correct for every value in every language shipped, and it is what the better translators had already chosen unprompted. A new counted string follows it; a `(s)` anywhere is the shape to reject in review.

**A translation may ship incomplete.** Missing keys fall back to English key by key (`localization::lookup`) and the served dictionary is merged over English (`localization::merge`), so a gap renders as English, never as a blank or a raw key. `GET /localization/languages` reports each language's `completion` and the picker shows it, so an incomplete language is offered honestly rather than hidden. CI errors only on real bugs — a key English does not have, a `{placeholder}` mismatch, an empty value, an orphan — and on a file below 50%, which is broken rather than partial.

Editing a locale JSON does not reliably trigger a rebuild — `cargo build --release` can leave a new key out of the binary, and the dictionary is `include_str!`'d, so the running server then serves the old one. `touch backend/src/localization.rs` forces it.

Adding a string is therefore *not* blocked on translating it 25 times, though the shipped set is currently at 100% — `scripts/add-locale.py <code> < translations.json` merges a batch into one of them and refuses to write if a `{placeholder}` was lost or invented; `python3 scripts/check-locales.py` (run in CI) fails on a missing key, a parity gap, mismatched `{placeholder}` sets, or a key nobody references. Keys built at runtime (`ConditionLabel*`, `Job*`, `Trigger*`, `Status*`) are exempt from the orphan check by prefix.

The dictionary is a module-level rune in `frontend/src/lib/i18n.svelte.ts`, not a context: there is exactly one of it, fetched once, and every component wants the same one. `main.ts` awaits it before mounting — every screen reads `t()` synchronously, so mounting first painted a frame of raw keys.

### Errors

Every response carries an `X-Request-Id` (`SetRequestIdLayer` outermost in `main.rs`, so the trace span records it); `ApiError.requestId` carries it to the interface, and `describeError` appends it to a 5xx alone — a refusal already says what to change. Handlers return `AppResult<T>`; [error.rs](backend/src/error.rs) maps variants to status codes and a `{ error, message }` body the frontend client unwraps. Use `BadRequest`/`NotFound`/`Conflict` for user-facing failures — raw sqlx/reqwest errors become 500/502. `AppError` is not `Clone`; `integrations/adapter.rs` rebuilds the variants it needs to report a bulk failure per item.

Every `uses:` in a workflow is pinned to a **commit**, with its tag in a comment
beside it, and names the tool it installs rather than inferring it from the ref.
A tag is a pointer its owner can move, and `release.yml` holds `packages: write`
and pushes to GHCR. Dependabot updates the id and the comment together, so this
costs nothing to keep current.

**Three workflows, and the split is deliberate.**

`ci.yml` opens with a `changes` job that diffs against the base commit and gates
the other five on which of `backend/`, `frontend/` and `site/` a commit touched.
Beside it, unfiltered like `changes`, a `repository` job lints what no
deliverable's job reads — the workflows (actionlint), the shell harnesses
(shellcheck), the two Dockerfiles (hadolint, with `.hadolint.yaml`) and the
history (gitleaks, with `.gitleaksignore`) — from binaries fetched by release
and checked against the sums written in the workflow. The backend job runs
`cargo deny check` against [`backend/deny.toml`](backend/deny.toml), which is
what refuses a licence the GPL cannot ship beside or a crate from a source
nobody named, and the frontend job holds the entry bundle and the shared
runtime to the ceilings in [`scripts/check-bundle-size.mjs`](scripts/check-bundle-size.mjs).
The directory layout is what makes that possible: with `Cargo.toml` and
`locales/` at the root, no filter could tell the backend from the repository.
It **fails open**: a base commit it cannot resolve runs
everything, because a filter that skips too much lets a broken commit through
while one that runs too much costs minutes.

`docker.yml` is separate for one reason: `ci.yml` cancels in progress on a new
push, the image takes minutes and everything else takes one, so on an active day
the only thing that checks the Dockerfile would be killed before finishing. Its
group queues instead of cancelling. After the build it runs
[`scripts/smoke-image.sh`](scripts/smoke-image.sh), which starts the image with the
README's hardening and checks what a green build cannot: uid 1000, the
healthcheck, the frontend actually served, a clean exit on SIGTERM, and a key
that survives a restart. It runs the key command **read out of
`ApiKeyGate.svelte`**, so the instruction on the first-run screen is tested
rather than restated. Locally it needs Docker, and `SMOKE_PORT` when a
development Routarr holds 9876; it refuses to start beside an existing
container named `routarr` rather than remove somebody's installation.

`release.yml` refuses to publish a tag whose commit has a workflow that did not
succeed — `skipped` counts as success, since the path filters skip whole areas —
attaches a signed provenance attestation to the pushed digest, and creates the
GitHub release with generated notes. That last step is the one that would
otherwise be `gh release create` typed on a laptop.

Dependabot watches four directories: the actions, `backend/`, `frontend/` and
`site/`. A directory that does not match where the manifest actually sits fails
in a workflow nobody opens.

## Testing

The whole suite is offline and takes about twenty seconds. Two harnesses:

- **[src/tests/mod.rs](backend/src/tests/mod.rs)** — `TestApp` drives the real `Router` (middleware included) against an in-memory database via `tower::ServiceExt::oneshot`. `seed_library()` / `seed_anime_rule()` set up the canonical fixture; `seed_instance_at()` points an instance at a fake Arr.
- **[src/tests/fake_arr.rs](backend/src/tests/fake_arr.rs)** and **[fake_tmdb.rs](backend/src/tests/fake_tmdb.rs)** — a real Radarr/Sonarr stand-in on an ephemeral port. Use it for anything touching `integrations/`, `sync` or `executor`; it records the API keys, bodies and query strings the client sent, so tests assert on what actually went over the wire. `FakeArr::failing(status)` exercises error paths.

**Three suites exist beyond the ordinary ones**, all offline except the last:

- **[src/tests/security.rs](backend/src/tests/security.rs)** — penetration tests against the assembled `Router`: authentication cannot be walked around (every protected route, both header forms, near-miss keys, scheme confusion), user text is data not SQL (`LIKE` escaping, category allowlist, rule values), no response carries a decrypted secret, the body cap holds, and a webhook token fails closed as a 404 that does not confirm whether the instance exists.
- **[src/tests/webhook_fuzz.rs](backend/src/tests/webhook_fuzz.rs)** — the webhook is the one route an unauthenticated party reaches, so it gets a corpus of hostile bodies plus ~3000 generated JSON trees from a seeded xorshift (a failure names its iteration and reproduces). The invariant is that **no input produces a 500 or a panic**; a `502` is allowed because the test instance points at an unreachable Arr, and a positive control proves a real event still reaches `200`.
- **[src/tests/live_sources.rs](backend/src/tests/live_sources.rs)** — `#[ignore]`d, opt-in, hits the **real** AniList, Jikan, OMDb and TheTVDB. Run with `cargo test live_sources -- --ignored --nocapture`; AniList and Jikan need no credential, the other two skip themselves when their key is absent. Deliberately out of CI: a green build must never depend on a third party's uptime. Each client also carries captured-payload tests next to it, which catch wire-format drift in CI without the network.

**Coverage is measured on both sides and gated on both.** `cargo llvm-cov`
reports 92% of backend lines (floor 90); Vitest reports 82% of frontend
statements (floor 80), and its other three floors sit the same two points below
what they measure. The frontend figure needs `all: true` in
`vite.config.ts` — without it v8 counts only the files a test imported, which
reports a high number over a small fraction of the tree and leaves whole screens
out of the figure entirely. Raise a floor when the real figure
moves up; never lower one to make a build pass.

`db::test_pool()` and `AppState::for_tests()` are `#[cfg(test)]` helpers. Tests are named as claims about behaviour (`a_trailing_slash_does_not_create_a_phantom_move`, `an_empty_upstream_response_does_not_wipe_the_library`) — keep that style.

`offline_warnings` in [api/health.rs](backend/src/api/health.rs) is the single source for every warning, and `/status` and `/health` both return exactly it. The two findings only a probe can make — an unreachable Arr, a source that stopped answering — reach it through `probe_results`: a probe **writes down what it saw**, and the endpoint that may not probe reads it back. `/status` is polled and one dead host costs a full connect timeout, so it can never look for itself; without the table the dashboard reported a source that had stopped answering while the navigation beside it counted zero. A verdict is replaced, never accumulated, and nothing expires it — the diagnostics page states its own findings from a live probe, and the dashboard is the default route, so in practice the table is rewritten on every visit. Two hand-built lists drift **both** ways — the count showing a warning the page never shows, and missing one the page does — and the chrome then totals one warning above a page listing three.

The shell reads that list once, in `Layout`; the bar totals what needs acting on and the navigation carries each count on the entry that answers it — so they never disagree with each other, only with the page. A screen that maps a category, enables an instance or saves a metadata key has just removed a warning the shell is still showing, and until [lib/status.svelte.ts](frontend/src/lib/status.svelte.ts) nothing told it: the idle poll corrected it a minute later. Those three screens now bump a revision the shell reads through `createAsync`'s `deps`, which re-runs the one request it already owns. A second copy of the state beside it would be a second thing to keep in step; a shorter poll would hide the problem rather than fix it.

The shell is written in **logical** properties — `inset-inline-start`, `margin-inline-start`, `text-align: start` — so it mirrors for a right-to-left language on its own. The direction comes from the backend (`localization::direction`, `RTL_LANGUAGES`), not from a list kept in the frontend, so adding `ar` to `CATALOG` turns the interface around with nothing else to touch. `.dir-aware` mirrors the glyphs that mean "towards".

Arabic ships, so the mirroring is exercised rather than assumed. What the logical properties do **not** cover is the content: a cell mixes our translated headings with values we did not write, and laid out in the page's paragraph direction an English description loses its full stop to the left edge and a raw timestamp swaps its halves. `index.css` answers with two rules — `unicode-bidi: plaintext` on cells *and their descendants*, since the property does not inherit, so each value is resolved by its own first strong character; and `direction: ltr` on `.mono`, for the machine formats that carry no strong character at all and would otherwise fall back to the page. [e2e/rtl.spec.ts](frontend/e2e/rtl.spec.ts) pins both by measuring where the characters land, and both were checked by removing the rule and watching it fail. It complements the `right-to-left` block in `layout.spec.ts`, which flips `dir` on an English page and so sees geometry but never content.

The sidebar header and the top bar share `--chrome-height`: they draw one rule across the window, and two independent values there show up as a step in it.

**The bar states a mode and, when there is one, a reason to stop.** It carried up
to five badges, each a translated sentence — 878px of chrome on a 1280px laptop
in French, and 168px of horizontal *document* overflow at 360px, where they
wrapped to three lines and burst out of a 64px bar. Two objects replace them,
both of locale-independent width: a `.mode-chip` whose dot says whether a click
will write, and an `.attention` control that appears only when a move has failed
or a diagnostic is waiting. A mode is not an alert, so `LiveModeActive` — the
state the application is meant to run in — is never painted in the danger
colour. The four counts moved onto the navigation entry that answers each of
them, where they can be acted on, and the version moved to `.sidebar-foot`,
which is also the only place a phone can read it.

The `.attention` control lands on the screen that answers **what it counts** —
`/logs` when a move has failed, `/health` otherwise — since sending an operator
to a page that says nothing about the thing that just broke is worse than
saying nothing. Its two clauses are joined by `ListSeparator`, a dictionary
entry rather than an ASCII comma, because Arabic writes `،` and Japanese `、`;
`Intl.ListFormat` is the wrong tool, its narrow unit form joining two clauses
with nothing at all in both. The mode chip carries `role="status"`: a `<span>`
with no role maps to `generic`, where ARIA prohibits `aria-label` and the whole
sentence would reach nobody.

**Radii are 2, 3 and 4px**, for badges, controls and surfaces. `--radius-full`
is a closed list — the kind dot, the progress bar, the navigation count and the
scrollbar thumb — and a pill anywhere else is a badge pretending to be a state.
Shadows mark elevation and nothing else: overlays keep theirs, cards lost
theirs. `--border-strong` is the boundary of anything that takes a click,
measured at 3:1 against `--bg-input` because WCAG 1.4.11 asks that of a control
and `--border-subtle` reads 1.37:1.

**A source's credential lives in the source's own row** ([ProviderOrder](frontend/src/components/ProviderOrder.svelte)),
not in a field three blocks below it: a key is not a setting of the application,
it is a property of the source it unlocks, and stated apart, enabling TMDb meant
scrolling past the whole list, saving, scrolling back and saving again.
`Settings.svelte` therefore skips `SOURCE_KEYS` in its field loop while
`SECTIONS` still lists them, and `SOURCE_KEY_SETTING` ([lib/settings.ts](frontend/src/lib/settings.ts))
is where the source-to-setting map is stated — written in both places instead,
adding a source removes its field from one screen and adds it to neither.

The row renders that field **whether or not the source is switched on and
whether or not a key is already stored**, because it is the only field for it
anywhere: shown solely for a source that could not answer yet, a leaked key was
impossible to rotate and a stored one impossible to replace without editing the
database. The placeholder is what distinguishes the two cases, since the
backend never sends a sealed value back.

**A blank credential is left out of the payload.** The field is empty on every
load for that same reason, so sending it wrote that emptiness — and
`api/settings.rs` treats an empty secret as "clear this", which meant saving an
unrelated setting deleted all three keys, silently, with the sources simply
ceasing to answer on the next pass. Leaving the field alone has to mean leaving
the key alone, which is what the `<key>_configured` boolean is for. Clearing a
key deliberately has no affordance yet.

Below 560px the row **stacks**: the actions drop to a line of their own. Holding
three lanes, they kept the 190px that aligns their right edges, the field beside
them measured 51px, and *Disable* was clipped outside its own row — with the
document never scrolling, so only a measurement inside the row shows it.

**Confidence is a figure beside a mark, never a figure on one.** Centred over
the fill, its contrast ran from 16:1 against the track to 1.95:1 against the
amber, which is what the text shadow was hiding. The mark is a continuous ramp
rather than three bands: `confidence_for` scales differently per match mode — an
`all` rule with one condition is 60%, an `any` rule with two is 53% — so no
percentage threshold can order them, and the bands it replaced painted an
ordinary two-condition rule as a warning. The ramp runs from `--text-muted` to
success, not from warning: starting at the warning colour made the same claim
the bands did, in another form.

**The meter is the explanation panel's shape, the dot is the table's.** A column
gives the track 28px after the figure — a stub that measures nothing and reads
as a smudge — while the panel gives it 150px. `Confidence` takes a `meter` prop
for exactly that, and both shapes read one `--confidence-ramp`, stated once for
the two of them.

Scroll containers style their own bar — `scrollbar-width`/`scrollbar-color` for Firefox and modern Chromium, a `::-webkit-scrollbar` block for the rest, both from the same tokens so the light theme does not get a dark pill. The scroll shadows on `.table-container` are four background layers, two pinned to the content and two to the frame; the covers must be **opaque across the shadow's full width** before they fade, or the shadow shows through at half strength down both edges of every table that has nothing to scroll. [e2e/layout.spec.ts](frontend/e2e/layout.spec.ts) samples the pixels, since nothing about it is visible to a DOM query and it reads as a design choice to anyone reviewing the interface by eye.

**The metadata tab is two cards, not one.** The source list is a subject of its own, not a field: rendered inside the field loop it took a field's caption, so the card title, that caption and the *active / available* group labels competed at the same weight — and the sentence governing the whole thing sat under the rows it was meant to introduce. In its own card the sentence becomes a `.card-note` under the title, where it governs, and one heading level disappears. `SOURCE_KEYS` skips it in the loop exactly as it skips the three credentials, so `SECTIONS` still lists every key and the "covers every field exactly once" check is untouched; one Save still covers both cards, which is why they sit inside one `<form>`.

Settings is the one screen that is not a table, so it is the one screen with a measure: `.settings-column` caps the tab strip *and* the panel at 880px. Capping the cards alone would run a full-width rule over a narrow block, and it is the mismatch that reads as broken, not the width. A form does not want the window: a caption and its field stretched across 1400px stop reading as a pair.

`.btn` carries a **fixed** `height` and `align-self: center`, not a minimum and not the default stretch. Without it the same `btn btn-primary` measures 34px, 36px and 41px on three pages: a flex row stretches buttons to their tallest sibling, `<label class="btn">` inherits a line-height `<button>` resets, and a text label makes a taller line box than an icon. [e2e/layout.spec.ts](frontend/e2e/layout.spec.ts) measures every button on every page and fails if more than one height exists per size.

[src/tests/scheduler.rs](backend/src/tests/scheduler.rs) covers the unattended sweep — the only code that writes without anyone asking. Each test reads the `jobs` table, which is the same evidence the Tasks screen shows. Note `ready_to_apply`: the guardrail tests assert nothing was written, so one test proves the fixture *can* write, otherwise they would pass on an empty library. The fake Arr serves a third, accessible root folder for exactly that reason — with one usable destination, no move can ever be proposed.

[src/tests/scale.rs](backend/src/tests/scale.rs) guards the thing `routing.rs` was rewritten for: a pool that counts connection acquisitions proves a simulation issues the **same ten queries** for 200 items and for 2 000. Every other test seeds one media item, so a regression to a query per item would pass the whole suite.

The Settings page is grouped into sections (`SECTIONS` in [lib/settings.ts](frontend/src/lib/settings.ts), data rather than markup so a test can read it), addressed by URL hash and rendered as an ARIA tab strip with a roving tab order. The `draft` is shared across sections and the payload is built from *all* `FIELDS`, so edits survive switching tabs and one Save covers everything — `global_dry_run` and `auto_apply_enabled`, whose combination is what warns, sit in different sections. A field missing from every section would be unreachable and saved with its fallback, so a test asserts the grouping covers `FIELDS` exactly once.

Every response carries a CSP plus `nosniff`, `referrer-policy`, `permissions-policy` and
`cross-origin-opener-policy` (`security_headers` in [main.rs](backend/src/main.rs)), and the application makes **no external request**: fetching its fonts from Google would tell Google the address of every homelab that opened it, and would leave the typography broken on an air-gapped host. `style-src` keeps `'unsafe-inline'` for the five styles computed from data — the four bars whose width is a figure (`Confidence`, `LibraryFacets`, `Jobs`, `TableSkeleton`) and the width a screen hands `Modal` — every other style lives in `index.css`, and a per-response nonce on those five is what would let the exception go.

The palettes are checked against WCAG AA by [theme.contrast.test.ts](frontend/src/theme.contrast.test.ts), which parses `index.css` and composites the translucent badge backgrounds. **Three of them, not two:** the light palette is written twice, once for `[data-theme='light']` and once for `auto` on a light system, which stamps no attribute and so can share no selector. A token added to one and forgotten in the other keeps its *dark* value on a white ground — the navigation measured 1.86:1 that way, for every reader who never opened the setting. The test measures the media block too and compares the two token sets, so the omission fails rather than ships. The *dark* theme is where it earns its keep — muted text and the danger and info badges all sit close to the 3:1 line — because a screenshot is not a measurement.

The theme is the `ui_theme` **setting** (`dark` / `light` / `auto`), applied by stamping `data-theme` on the root element; `auto` stamps nothing, which is what lets the `prefers-color-scheme` block apply. `--accent-primary` is the accent as a *fill* and is shared by both themes; `--accent-strong` is the accent as *text* and darkens in light, where the orange reads at about 1.9:1.

[components/ApiKeyGate.svelte](frontend/src/components/ApiKeyGate.svelte) replaces the whole shell when `/status` answers 401. A key is generated at first start, so a browser without one is the *ordinary* first visit; mounting the thirteen pages behind it produced a failed request each and looked like a broken install. `createAsync` exposes the thrown `failure` alongside the rendered `error` message precisely so the shell can tell a 401 from any other fault. It names the **file** and the command that reads it rather than pointing at the log: the log line is printed once and goes with the container on the first image update, while the file is on the volume.

**Destructive actions ask through [lib/confirm.svelte.ts](frontend/src/lib/confirm.svelte.ts)**, never `window.confirm`. A module-level rune for the reason the dictionary is one — there is exactly one dialog and every caller wants the same one — with `ConfirmDialog` mounted once by `Layout`, outside the routed page so a question survives the navigation it may have triggered. `askConfirmation` is the yes/no shape; `ask` takes several choices, because the rule import is not a yes/no — squeezed into one it asks "replace everything?" and appends on Cancel. Button labels are dictionary **keys**, not rendered strings. Native `confirm()` costs three things: its buttons render in the *browser's* language whatever `ui_language` says, jsdom ships none so unit tests answer a stub of their own making, and a browser told to suppress dialogs returns `false` without asking — which on the apply guardrail means the action silently does not happen. Tests answer through [src/test/confirm.ts](frontend/src/test/confirm.ts), which returns the question asked; e2e clicks the real button rather than installing `page.on('dialog')`, a blanket handler that cannot tell the right question from any question.

Modals go through [components/Modal.svelte](frontend/src/components/Modal.svelte), a native `<dialog>` opened with `showModal()`: the top layer, the focus trap, the inert background and focus restoration come from the browser rather than from four hand-written approximations. It falls back to the `open` attribute where `showModal` is absent, which is not hypothetical — jsdom ships the element and neither of its methods, so that is the path every unit test takes. A `<svelte:boundary>` inside `Layout` wraps the routed page, keyed on the path so navigating away clears it, because a render-time throw otherwise blanks the whole tree.

Tests render through `renderWithI18n` ([frontend/src/test/render.ts](frontend/src/test/render.ts)), which seeds the dictionary *before* the component renders — `t()` is read synchronously on the first pass — and seeds only the strings they assert on. A component that takes a snippet cannot be handed one from a test, so `Layout`, `Modal`, `DecisionRow` and `poll` each have a two-line harness in `src/test/`.

**End-to-end** (`frontend/e2e/`): `npm run test:e2e` builds the release binary and the frontend, starts both plus a fake Radarr on throwaway ports, and drives Chromium through Playwright. It is the only layer that can see a broken route, a control wired to nothing, a table cell folding onto two lines, or a caption that labels nothing — jsdom computes neither geometry nor accessible names. The 80-odd journeys query by role and by label, never by implementation detail, which is what lets the interface be rewritten under them without editing a single spec. The two entry points select **by tag**, not by filename (`--grep-invert @subpath` and `--grep @subpath`): listing files by hand means a new spec is silently never run. `e2e/run.sh` starts the server directly rather than inside a subshell — inside one, `$!` is the subshell's pid, so the trap kills the wrapper and leaves the real server holding the port — and waits for each process to exit rather than only asking. Vitest excludes `e2e/`.

[e2e/accessibility.spec.ts](frontend/e2e/accessibility.spec.ts) **sweeps all
thirteen screens** rather than sampling two editors: no control without an
accessible name, no heading level skipped, no two buttons in one table body
answering to the same name, no `id` rendered twice, a caption on every table,
and a skip link as the first tab stop. Each catches what a hand-written test
cannot — an unnamed selection checkbox on the screen that moves files, an
`h1 → h3` jump, a table whose row actions are all called "Delete" or "Edit", a
component with a hard-coded id used twice on one form. A row action is named
`action — subject`; that convention is what the third check enforces. On top of
the checks this interface taught us to write, **axe-core** runs every screen at
WCAG 2.1 A and AA, so a failure names a rule rather than an opinion.

The scrollable box around a table is [`TableRegion`](frontend/src/components/TableRegion.svelte):
role, name and `tabindex` in one place, because axe's `scrollable-region-focusable`
and Svelte's `a11y_no_noninteractive_tabindex` contradict each other on exactly
that element and the reconciliation should be stated once, not sixteen times.

The screen sweep opens **nothing**, so every modal falls outside it — the two
named tests at the top of that file cover the rule and instance editors, and
check specific labels rather than completeness. A second sweep opens all seven
(both editors, the pin dialog, new and rename category, the explanation panel
and the confirmation) and applies the same three checks plus one: the dialog
itself must carry an accessible name, or it is announced as "dialog". It is a
regression guard, and it is verified by removing a `<label>` and watching it
report `unnamed INPUT.form-input`. Its list is not kept by hand: every entry
names the source file it opens, and [src/test/modals.test.ts](frontend/src/test/modals.test.ts) fails when a file
rendering `<Modal>` is not among them — the same lesson as selecting e2e specs
by tag rather than by filename.

`src/test/layout.test.ts` reads the screens as **source**, so its `pages()` glob is load-bearing:
a pattern that matches no file leaves every check iterating over an empty list and passing without
reading anything. It covers `components/` as well as `pages/`, since half the forms live there, and
each check is verified by breaking one thing and watching it fail.

Frontend: `npm test` runs Vitest with **jsdom**, not happy-dom, which does not drive a `<select>` the way Svelte's `bind:value` reads it — the option changes in the DOM, the bound variable never does, and a filter test passes its click then asserts against a request nobody made. Drive selects with `userEvent.selectOptions`, not a synthetic `change`. Pure logic lives in [api/format.ts](frontend/src/api/format.ts) and [api/conditions.ts](frontend/src/api/conditions.ts) precisely so it can be tested without rendering.

## Showcase site

[`site/`](site/) is a separate deliverable: an Astro build deployed to Cloudflare Pages, static, and
making no external request at run time — which is what keeps its CSP at `default-src 'none'`. It is
not served by the application and is excluded from the Docker context.

The build output is **not** committed any more: Cloudflare Pages runs `npm run build`. `site/dist`
is what every check reads, on purpose — checking the sources would test the intention, and what
matters here is the artefact: a CSP hash, a fingerprinted stylesheet, an image that exists in
`public/` and never reached the build. `serve.mjs` still exists rather than `astro preview` because
it parses the real `_headers`, and previewing without them hides a CSP that blocks the stylesheet.

Only one script is inline — the theme bootstrap, which has to run before first paint — and its
`sha256-` is pinned in `_headers`. `site.js` is served as a file with `is:inline` on its tag for
that reason: Astro would bundle a script that small into the page, and an inline script needs a hash
of its own.

```bash
npm --prefix site ci       # once
npm --prefix site run build   # Astro -> site/dist, the four pages plus /404
cd site && npx astro check   # types over the components and the catalogue (npm --prefix does not work: npx resolves the binary from the cwd)
node site/serve.mjs        # preview site/dist, applying the real _headers
node site/check.mjs        # origins, CSP hash, contrast, image sizes, dead links
node site/verify.mjs       # loads the built page in Chromium under that CSP
node site/icons.mjs        # re-render favicon.ico and the PNG icons from the SVG
bash site/screenshots/run.sh   # regenerate public/assets/shots from the real binary
```

**The hero is a split-flap departure board** ([Hero.astro](site/src/components/sections/Hero.astro)):
six departures from the demonstration library, one flap per character, the destination flipping from
the grey folder a title is in to the amber folder it is bound for, the gate being the Arr instance,
and a *Why* drawer under each row carrying the same `expected · observed` pairs the application's
explanation panel shows. Three things about it break quietly. The rows are real `<table>` rows laid
out as a grid sharing one `--cols` template, and the flap size `--flap` is what the breakpoints
change: the gate folds under the status below 1120px and a phone reads one departure per card, so a
new column is a change to `--cols` at every width. The CSP allows no inline `style`, so a row's place
in the flip cycle and a flap's stagger are keyed on `data-row` and `data-i` and stated in `site.css`.
And the board is a physical object, dark in both themes like the code blocks, with colours of its
own. `verify.mjs` measures every flap word, status and rule against its column on all four pages at
1280, 900 and 390px, because a status one word longer in German overflowed its column by 2px and
nothing else noticed; the count of words measured is the guard on the guard.

`serve.mjs` parses `_headers` on purpose: previewing with any other static server hides a CSP
that blocks the stylesheet. The screenshots are captured from a throwaway instance with its own
database and its own fake Radarr, Sonarr and TMDb, so no real library or API key can reach a
published image. Re-crop one and `check.mjs` fails until the `width`/`height` in the HTML match.

Each screenshot ships **twice**, AVIF and WebP, both encoded from the same PNG in
one pass of `run.sh` — never one from the other, since re-encoding lossy into
lossy compounds both sets of artefacts and these are pictures of text. The
`<img>` inside each `<picture>` stays the source of truth for `alt`, dimensions
and loading; the `<source>` above it is the only thing AVIF adds. It halves the
page: 480 kB down to 232 kB for a browser that takes it. `check.mjs` fails on a
lone file of either format, on a `<source>` pointing at nothing, and on an
extension `serve.mjs` has no MIME type for — the last one because a file served
untyped may be refused by exactly the browser that would have taken it, which is
visible only in preview.

The site ships in English, French, German and Spanish, from **one** set of Astro components rendered
four times — `src/layouts/Landing.astro` is the page, and `src/pages/index.astro` and
`src/pages/[lang]/index.astro` are the routes. `src/i18n/<code>.json` maps a key to a string and
`useTranslations` throws on a key no catalogue answers, so a missing translation fails the build
rather than rendering an empty element; `Key` is derived from `en.json`, so a key English does not
have fails `astro check` before that.

Two properties of the catalogues are worth stating, because losing either is silent. A value is a
**string, not an anchor**: it must not carry the opening tag it is meant to sit inside
(`'<span class="eyebrow">Features'`), or the markup leaves the template unbalanced. And a value must
not carry HTML entities — an `&amp;` lifted out of markup is double-escaped the moment it goes
through a template that escapes. Comparing the four rendered pages word for word is the check worth
repeating after any change to this layer.

`{t('key')}` inside an attribute must not be quoted: `alt="{t('x')}"` is a literal in Astro, not an
expression — it renders the literal text and breaks every `alt` on the page without a word of warning.

One placeholder is still waiting for a real value and is checked: the domain (`routarr.app`).
The repository URL is settled — `github.com/Routarr/Routarr`. `check.mjs` refuses more than
one site origin, so a half-finished rename fails rather than ships.

## Frontend

Each screen is its own chunk: `App.svelte` holds a route table of dynamic
imports and renders the match through `{#await}`, so opening the dashboard does
not download the rule builder, the log viewer and the settings form with it. The
entry bundle is 21 kB beside 67 kB of shared runtime — a figure nothing checks, so it is the kind that drifts; a size floor in CI is what would make it verifiable rather than remembered.

**The router is written, not installed** ([lib/router.svelte.ts](frontend/src/lib/router.svelte.ts), fifty lines) for one
reason specific to this application: the mount point is discovered at *runtime*
from the `<base href>` the backend injects, because a single image has to serve
any sub-path, and every SPA router worth taking wants its base at build time.
Links stay plain `<a href>` and one delegated listener intercepts them — a
component rendering `role="link"` loses middle-click, ctrl-click, the status bar
and the accessible name the e2e sweep queries by.

**The route table lives in [lib/routes.ts](frontend/src/lib/routes.ts)** as plain data, and
[lib/navigation.ts](frontend/src/lib/navigation.ts) dresses it in icons: three things read it now,
the navigation drawing thirteen destinations in five groups, the command palette searching them, and
the e2e sweeps opening every screen through [e2e/screens.ts](frontend/e2e/screens.ts) — the icons
are Svelte components, which nothing without a Svelte runtime can import, and a list kept by hand
beside the table lacked a screen. A second copy is how a destination ends up reachable from one and
not the other. A group heading is a label and never a heading level — the accessibility sweep fails
a screen that skips one, and a navigation is not an outline.

**[CommandPalette.svelte](frontend/src/components/CommandPalette.svelte) is deliberately narrow.**
`Ctrl/⌘K` opens one field over the destinations and the library, and the library is asked 200 ms
after the typing stops, because that request goes to somebody's own host over their own network. It
answers "why is this here" **in place**, fetching the explanation rather than navigating: neither
the explanation panel nor the rule editor is addressable by URL, so sending someone to `/media`
would be step one of the four steps the palette exists to replace. It carries **nothing that
writes** — applying a move, reverting one and deleting a rule each keep a guardrail on the screen
that owns it, and a palette is built to be fast, which is the opposite of what a write to somebody's
library wants. It is a combobox, so the focus never leaves the field and `aria-activedescendant`
is the only thing saying where the arrows are; an `option` may therefore hold no interactive
content, which is why the row carries its own click instead of wrapping a button that would also be
a tab stop. Its trigger is named *Quick search* rather than *Search*: it sits on every screen, and
the library has a Search button of its own.

Anything that refreshes on a timer goes through [lib/poll.svelte.ts](frontend/src/lib/poll.svelte.ts), never a bare
`setInterval`: it stops while `document.visibilityState` is `hidden` and reloads
immediately on return. Its interval and its `active` flag are read as getters,
so a caller can change the pace or stop entirely and the timer follows. The Tasks screen polls every three seconds while a job runs,
which in a forgotten background tab was twelve hundred requests an hour against
a machine that is also running Radarr and Sonarr.

**A code is shown with what it means, where that is not in dispute.**
[integrations/certification.rs](backend/src/integrations/certification.rs) names
`U`, `TP` and `TV-PG`, which say nothing to most readers, and the closed
vocabularies name a language or a country. In every case the *value* stays the
code — it is what the engine matches — and the name goes in `label` beside it,
which is why the panel and the picker both render `label ?? value` and a rule
written from either carries `ja`. Only codes whose meaning agrees across the
systems that issue them are named: `12` is twelve-and-over for the BBFC, the FSK
and the CNC alike, while `M` is fifteen-and-over in Australia and something else
in the United States, so `M` is left bare. The media row carries the
certification and not the system that issued it, so nothing can disambiguate —
which is why the list is restricted to what needs none. A wrong name on a right
value is worse than no name.

`GET /media/facets` counts what the library actually carries per axis a condition reads — genres, original languages, certifications, tags, series types, root folders — plus the items carrying no metadata at all. It reads the media row **and** `metadata_cache`, for the enabled sources only and counting distinct media: the rule builder offers this list, so it has to hold what the engine can match, and a genre TMDb supplied is missing from a list read off `media` alone. A source the user switched off contributes to neither, exactly as `routing::load_context` reads them. Without it, writing a rule means guessing what is present: a rule written against a value the library does not hold matches nothing and reads on screen exactly like a rule that correctly matches nothing. Rendered above the rule table as small multiples — one card per axis, a proportional bar behind each row and the figure beside it. Rows of identical pills would give a value seen 27 times and one seen once the same weight, and put the only number that matters in the smallest element on screen. The bar is **neutral**, not the accent: the accent is this interface's action colour, and spending it on passive reference data claims an emphasis the panel does not have — at 16% over the dark ground it also came out a muddy brown. The cards flow in CSS **columns** rather than a grid, because a grid row waits for its tallest member and these axes differ from one value to eleven, which left a hole under every short card.

**`frontend/src/api/types.ts` mirrors the backend payloads, and `scripts/check-api-types.py` is what keeps it honest.** The contract is written twice in two languages and nothing else joins them: rename a field in a `Serialize` struct and everything compiles, `cargo test` passes, `svelte-check` passes, and the interface renders `undefined` — only an end-to-end journey that happens to read that field would notice. The script compares the serialized field names of each pair in its `PAIRS` table and runs in **both** CI jobs, because the contract breaks from either side and the path filters gate the two separately. It compares names, not types. A `#[serde(flatten)]` is **resolved** rather than skipped — the flattened type sits in the same map of parsed structs, and the three composite responses are exactly the ones this file warns to consume as flat objects, so they were both the hardest to get right by reading and the ones the check gave up on while its summary still counted them as agreeing. The summary states what it *compared* and names anything it skipped.

`client.ts` `client.ts` is the single flat `api` object and attaches the browser-stored `X-Api-Key`. Neither knows anything about the view layer, which is the point: they are the part of the frontend a change of framework does not touch. [lib/async.svelte.ts](frontend/src/lib/async.svelte.ts) provides loading/error state — do not swallow errors in `console.error`, a failed request must render a banner. Pass `deps` as a getter to re-run on a filter change; a generation counter drops a response that arrives after the inputs moved on.

`tsconfig.json` sets **`erasableSyntaxOnly`**, which is the type-level counterpart of `isolatedModules`: Vite strips types file by file and emits nothing else, so the source may not use the parts of TypeScript that compile to *code*. Constructor parameter properties are the one that comes up — `ApiError` declares its fields and assigns them instead. `noUnusedLocals` and `noUnusedParameters` are on too, which is what makes an orphaned import or argument a failed check rather than something a reviewer has to spot. [eslint.config.js](frontend/eslint.config.js) covers what a type checker cannot — a promise left unawaited, a `console.log` in a screen, a value import that should have been a type import, a mutable `Set` where `SvelteSet` is meant — and is type-aware, which is why `e2e/` carries a `tsconfig.json` of its own: the root one excludes it so `svelte-check` does not type the Playwright suite, and a file no project claims cannot be linted with types.

The app shell reads `/api/v1/status`, never `/health`: the latter probes every Arr instance over the network. **The dashboard makes both calls** — `/health?probe=false` answers from the database in milliseconds and renders the page, `/health` probes and upgrades the instance table's status column behind it. Probing an unreachable Arr costs the full connect timeout, which measured 5 042 ms against 4 ms, on the one screen somebody opens *because* an Arr is unreachable. Without a probe an instance reports `status: "unchecked"` rather than a guess, and the column says so. Pages under `pages/` map 1:1 to sidebar routes in `App.svelte`, with one exception: `NotFound` has no menu entry, and is what an unmatched path renders — inside the shell, with the address left alone, because a miss is a typed address or an old bookmark and both are worth correcting rather than silently rewriting. Styling is one hand-written dark theme in `index.css` (Servarr-inspired) with `@lucide/svelte` icons, re-exported through [lib/icons.ts](frontend/src/lib/icons.ts) under the names the screens use, since Lucide renames glyphs between versions — `History` is the dangerous one: the obvious guess is `Clock` and the right answer is `RotateCcwClock`. No state library.

Note the API flattens nested response structs with `#[serde(flatten)]` (`RootFolderWithInstance`, `OverrideWithMedia`, `CategoryWithUsage`) — consume them as flat objects.
