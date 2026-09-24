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
---
# CI, scripts, Docker and the dev container

## Workflows

- Pin every `uses:` to a full commit SHA with its tag in a trailing comment (`@<sha> # v7.0.1`).
  A tag can be moved, and `release.yml` holds `packages: write`.
- The three workflows are split on purpose, each header saying why: `ci.yml` cancels a superseded
  run, `docker.yml` queues so a started image build finishes, `release.yml` runs on a `v*` tag.
- In `ci.yml`, `changes` gates every job except `repository`, and those two are the only
  unfiltered jobs. A check that must see every commit goes in one of them, as
  `scripts/check-versions.py` does in `changes`: a commit touching only a Dockerfile or
  `.devcontainer/` matches no filter.
- A commit touching only `scripts/` wakes the backend jobs, not the frontend one, which alone runs
  `scripts/check-bundle-size.mjs`. Run that script yourself after `npm --prefix frontend run build`.

## Docker image

- A file the root `Dockerfile` copies from outside `backend/` and `frontend/`, as it copies
  `LICENSE`, goes into both `paths` lists of `docker.yml`. Otherwise a change to it is first
  built by `release.yml`.
- The first-run command, `docker exec routarr cat /data/routarr.api_key`, depends on
  `container_name` in `docker-compose.yml` and on `ROUTARR_DB_PATH`, set in the `Dockerfile`
  and again in `docker-compose.yml`.
  `scripts/smoke-image.sh` runs the copy in `ApiKeyGate.svelte` against the image, and nothing
  checks the copies in `README.md` and `site/src/components/sections/Start.astro`.

## Checks that run only in CI

- The dev container has no Docker, shellcheck, hadolint, actionlint, gitleaks or cargo-deny. The
  image build, `scripts/smoke-image.sh`, `cargo deny` and the `repository` job run only in CI.
  That job holds every tracked `*.sh` to `shellcheck -S style`, both Dockerfiles to hadolint under
  `.hadolint.yaml`, and the workflows to actionlint.
- gitleaks scans every commit and no `.gitleaksignore` exists: a secret-shaped literal, a
  realistic fake key in a test included, fails the job even after a later commit deletes it.
- Nothing in CI builds `.devcontainer/Dockerfile`: hadolint and `check-versions.py` are its only
  checks. It builds with the root `.dockerignore`, which excludes `.devcontainer/`, so it cannot
  `COPY` a file from its own directory.

## Toolchain and versions

- A script that builds or runs the binary does `(cd "$ROOT/backend" && cargo build ...)` and finds
  it under `${CARGO_TARGET_DIR:-$ROOT/backend/target}`. rustup reads `rust-toolchain.toml` from the
  working directory, so `--manifest-path` from elsewhere compiles with the default toolchain, and
  `CARGO_TARGET_DIR` is `/ramdisk/cargo-target` in the dev container but unset in CI.
- No workflow installs or pins a Rust toolchain: rustup reads `backend/rust-toolchain.toml`.
- A Rust, Node, Playwright or Routarr version is written in several files.
  `python3 scripts/check-versions.py` names every copy a bump left behind.
