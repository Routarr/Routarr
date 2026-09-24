# CLAUDE.md

Routarr is a self-hosted routing layer for the Servarr ecosystem. It reads movies and series
from Radarr and Sonarr, enriches them from an ordered list of metadata sources, evaluates the
rules a user writes, and moves each item to the right root folder through the Arr APIs. Rust and
Axum backend, Svelte 5 and Vite frontend, shipped as one Docker image.

Each area has its own rules in `.claude/rules/`: `backend.md`, `frontend.md`, `site.md` and
`ci.md`. A rule loads when Claude reads a file its `paths:` list matches with the Read tool.
Reading through Bash (`cat`, `grep`, `sed`) loads none, so open a file with Read before
changing it.

## Layout

```
backend/     the Rust crate: src/, migrations/, locales/, Cargo.toml
frontend/    the interface, built into frontend/dist and served by the backend
site/        the showcase site, deployed to Cloudflare, never in the image
scripts/     the consistency checks CI runs, the locale tooling, the image smoke test
Dockerfile   builds backend and frontend into one image, `site/` is in .dockerignore
```

The crate stays in `backend/`: the Dockerfile, the CI path filters, the shell harnesses and the
documentation links all address that shape, so moving it is not a rename.

## Commands

Backend, from `backend/`:

```bash
cargo run                                   # http://0.0.0.0:9876, reads .env
cargo test                                  # offline, in-memory SQLite
cargo test services::rule_engine            # one module, or one test by name
cargo test live_sources -- --ignored --nocapture   # the real AniList, Jikan, OMDb, TheTVDB
cargo fmt
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
cargo llvm-cov --summary-only --ignore-filename-regex 'src/(tests/|main\.rs)'   # floor 90%
python3 ../scripts/check-locales.py         # dictionaries: keys, placeholders, orphans
python3 ../scripts/check-api-types.py       # response structs against frontend/src/api/types.ts
python3 ../scripts/check-versions.py        # every place a toolchain version is written
```

Frontend, from `frontend/`:

```bash
npm run dev                  # Vite on :3000, proxies /api to :9876
npm run build                # svelte-check, then vite build into frontend/dist
npm run check                # svelte-check alone
npm run lint                 # type-aware ESLint
npm run format:check         # Prettier
npm run coverage             # Vitest with its floors, over the whole tree
npm run test:e2e             # builds everything, drives Chromium through Playwright
npm run test:e2e:base        # the specs tagged @subpath, under the /routarr mount point
npm audit --audit-level=high
```

Full stack: `docker compose up -d --build`. The site's commands are in `.claude/rules/site.md`.

The Rust build tree is a RAM tmpfs (`CARGO_TARGET_DIR` under `/ramdisk`), so nothing may assume
where the binary is. `.devcontainer/prune.sh` runs at every container start and attach and may
delete build artefacts, so a slower rebuild now and then is expected. On "no space left on
device", run `bash .devcontainer/prune.sh --status` first.

A release is a `v*` tag pushed on a commit whose CI is green, as `CONTRIBUTING.md` describes.
`release.yml` refuses any other commit and opens a draft release.

## Writing

- Code, comments and every document are in English.
- A comment states a constraint in the present tense: what breaks if the code changes. Never
  the history of the code (what was tried, what changed, what used to happen) and never a
  figure measured on a given day. That belongs in the commit message.
- A comment that exists because the code is unclear is a naming problem: rename or extract,
  then delete it. Nothing describes what a line does.
- No em dashes, no semicolons and no curly quotes in prose.
- Commits follow Conventional Commits: an imperative subject of 72 characters at most, a body
  wrapped at 72. One coherent lot per pull request, merged with a merge commit.
- This file and each rule stay under 200 lines of at most 100 characters, and every rule opens
  with a `paths:` list of quoted globs (`scripts/check-claude-md.py`). A line stays only if
  Claude would make a mistake without it. A constraint lives in the code comment where it acts,
  an area's rule in `.claude/rules/`.

## Architecture

`api/` handlers, then `services/`, then `integrations/`, then SQLite through `sqlx`. CRUD
resources query from their handler on purpose. What would still be logic without HTTP
(simulation, apply, sync, enrichment, rule evaluation, backup) lives in `services/`.

- `backend/src/main.rs` holds the whole route table: `public`, `protected` behind the API key
  middleware, and `webhooks`, authenticated by a per-instance token in the path. A new endpoint
  touches its `api/` module and this table.
- `ROUTARR_AUTH` is `apikey` (the default), `forms`, `oidc`, `external` (a reverse proxy in
  front authenticates) or `none` (open). With no `ROUTARR_API_KEY`, a key is generated into
  `routarr.api_key` beside the database.

Invariants a change must keep:

- `services/executor.rs` is the only code that writes to an Arr, behind its guardrails.
- Categories are strings joined by name, with no foreign key. The fallback category is read
  through `AppState::default_category` and nowhere else, and a rename goes through
  `api::categories::rename`.
- Everything on disk hangs off `Config::data_dir`, never off `db_path.parent()`.
- A decrypted secret is never logged nor returned. Arr and metadata keys are sealed at rest.
- The application makes no outbound request nobody asked for: no web fonts, no telemetry.
- Every text a user reads comes from `backend/locales/`, referenced by key, on both sides. The
  rule engine emits keys and parameters, never prose. `t()` and `translate` substitute
  `{placeholder}` and know no plural, so a count reads `Warnings: {count}`, never
  `{count} warning(s)`. A key built at run time needs its prefix in `DYNAMIC_PREFIXES`
  (`scripts/check-locales.py`), and a new language needs its `CATALOG` entry in
  `backend/src/localization.rs`, which no check reads.
- `frontend/src/api/types.ts` mirrors the backend payloads, and `check-api-types.py` holds the
  field names in step from both sides.

## Testing

- The backend suite is offline, and no test depends on a third party answering.
- A test is named as a claim about behaviour: `a_trailing_slash_does_not_create_a_phantom_move`.
- A regression test is seen to fail before its fix, or with the check it guards removed.
- Coverage floors: 90% of backend lines, and 80% of frontend statements with the other Vitest
  floors in `frontend/vite.config.ts`. Raise a floor when the figure rises, never lower one to
  pass a build.
