---
paths:
  - ".github/**"
  - "scripts/**"
  - ".devcontainer/**"
  - "Dockerfile"
  - "docker-compose*.yml"
  - ".dockerignore"
  - ".hadolint.yaml"
  - "**/*.sh"
  - "README.md"
---
# CI, scripts, Docker and the dev container

## Workflows

- Pin every `uses:` to a full commit SHA with its tag in a trailing comment (`@<sha> # v7.0.1`).
  A tag can be moved, and `release.yml` holds `packages: write`. The repository settings refuse
  an unpinned action and any action outside GitHub's, `docker/*`, `Swatinem/rust-cache` and
  `taiki-e/install-action`: a new third-party action is allowed there by the owner first.
- Every checkout sets `persist-credentials: false` (`gh` takes `GH_TOKEN`), and `release.yml`
  grants nothing at the top (`permissions: {}`), each job naming what it needs.
- The six workflows are split on purpose, each header saying why: `ci.yml` cancels a superseded
  run, `docker.yml` queues so a started image build finishes, `release.yml` runs on a `v*` tag,
  `site.yml` deploys the site from the latest release's tag (secrets `CLOUDFLARE_API_TOKEN` and
  `CLOUDFLARE_ACCOUNT_ID`) and compares production with it every week, `mutants.yml` runs
  cargo-mutants every Saturday, and `live-arrs.yml` drives a real Radarr and Sonarr every
  Wednesday. Neither of the last two gates anything.
- In `ci.yml`, `changes` gates every job except `repository`, and those two are the only
  unfiltered jobs. A check that must see every commit goes in one of them, as
  `scripts/check-versions.py` does in `changes`: a commit touching only a Dockerfile or
  `.devcontainer/` matches no filter.
- Which areas a commit wakes is `scripts/changed-areas.sh`, which a dry run calls from a clone
  (`bash scripts/changed-areas.sh BASE HEAD`). A push to main is compared with the last commit of
  main whose run succeeded, so a failed or cancelled run leaves its areas to the next one.
  `docker.yml` reads its `image` and `devcontainer` lines in a `changes` job of its own. No
  workflow takes a `paths` filter: a check it skips never reports, and a required one then
  blocks every pull request.

## Docker image

- A file the root `Dockerfile` copies from outside `backend/` and `frontend/`, as it copies
  `LICENSE`, goes into the `image` pattern of `scripts/changed-areas.sh`. Otherwise a change to
  it is first built by `release.yml`.
- The first-run command, `docker exec routarr cat /config/routarr.api_key`, depends on
  `container_name` in `docker-compose.yml` and on `ROUTARR_DB_PATH`, set in the `Dockerfile`
  and again in `docker-compose.yml`. `scripts/smoke-image.sh` runs the copy in
  `ApiKeyGate.svelte` against the image and holds it to `docker-compose.yml`, and
  `site/check.mjs` holds the copies in `README.md`, `site/public/llms.txt` and
  `site/src/components/sections/Start.astro` to it.
- `docker.yml` runs `scripts/smoke-image.sh` again under gVisor, installed by
  `scripts/install-runtime.sh` at a pinned version and checksum, and `release.yml` runs it on
  both architectures before pushing. A check added to the smoke test has to hold in all three.
- No release build restores a cache: anything that runs on main can write the one `docker.yml`
  keeps, and `release.yml` signs what it builds. Every `setup-buildx-action` names BuildKit by
  digest (`driver-opts`), and `check-versions.py` holds the copies equal.
- The image serves `third-party-notices.txt`: `cargo about` (`backend/about.toml`) lists the
  crates and Vite (`build.license`) the bundled packages. A licence a new dependency brings goes
  into `backend/deny.toml` and `about.toml` together, which `scripts/check-licences.py` holds.

## The repository job and the dev container

- The dev container has no Docker: the image build and `scripts/smoke-image.sh` run only in CI.
  It installs the other tools CI runs (shellcheck, hadolint, actionlint, gitleaks, cargo-deny,
  cargo-llvm-cov, oasdiff, ruff) at the versions `ci.yml` pins, and `cargo about` and
  `cargo auditable` at those of the root `Dockerfile`, which `check-versions.py` holds in step,
  so bump a tool in both files. Run `bash scripts/check.sh repository` before pushing a script,
  a workflow, a Dockerfile or a contract change.
- `scripts/check.py` runs the `run:` steps of `ci.yml` themselves, mapped to areas by job
  (`AREAS`): a new job needs its line there, and a step only CI can take (an install, a filter)
  its start in `CI_ONLY_STEPS`. `scripts/tests/test_check.py` fails on either missing.
- gitleaks scans every commit and no `.gitleaksignore` exists: a secret-shaped literal, a
  realistic fake key in a test included, fails the job even after a later commit deletes it.
- `docker.yml` builds `.devcontainer/Dockerfile` on amd64 and arm64 when its `devcontainer` line
  wakes, so a wrong sum for either architecture fails there. It builds with the root
  `.dockerignore`, which excludes `.devcontainer/`, so it cannot `COPY` a file from its own
  directory.

## Toolchain and versions

- A script that builds or runs the binary does `(cd "$ROOT/backend" && cargo build ...)` and finds
  it under `${CARGO_TARGET_DIR:-$ROOT/backend/target}`. rustup reads `rust-toolchain.toml` from the
  working directory, so `--manifest-path` from elsewhere compiles with the default toolchain, and
  `CARGO_TARGET_DIR` is `/ramdisk/cargo-target` in the dev container but unset in CI.
- No workflow installs or pins a Rust toolchain: rustup reads `backend/rust-toolchain.toml`.
- A Rust, Node, Playwright or Routarr version is written in several files.
  `python3 scripts/check-versions.py` names every copy a bump left behind.
