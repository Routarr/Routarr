# Contributing

Thanks for looking. A few things are worth knowing before you spend time on a
change, because some of them are unusual.

## Start with the architecture

[`CLAUDE.md`](CLAUDE.md) covers the architecture, the conventions, and the
reasons behind the ones that look odd. It is the right starting point for
almost any change, and it will save you from undoing something on purpose.

A few features have been considered and deliberately left out, because they
complicate the engine or the interface without serving what Routarr is for —
deciding which root folder a media item belongs in, and getting it there
safely. Signal weighting, nested condition groups, temporary overrides and a
persistent job queue are the ones that come up. Please open an issue before
building one of those rather than after.

## Getting set up

The dev container ([`.devcontainer/`](.devcontainer/)) has everything: the
pinned Rust toolchain, Node 24, python3, `cargo-audit`, `gh`, and the Playwright
browser. Open the repository in it and `postCreateCommand` does the rest.

Without it, you need Rust 1.98 (rustup reads
[`backend/rust-toolchain.toml`](backend/rust-toolchain.toml)) and Node 24.

Inside it, the Rust build tree lives in a 16 GiB RAM disk at `/ramdisk` rather
than on your disk — `CARGO_TARGET_DIR` points there, and everything in it is
gone when the container stops, at the price of one cold `cargo build` when it
comes back. Nothing on your machine has to be prepared for that: the container
runtime makes the mount. Three commands are worth knowing:

```bash
bash .devcontainer/prune.sh --status   # how full it is, and what is using it
bash .devcontainer/prune.sh            # reclaim the regenerable parts now
cargo clean                            # start the build tree over
```

A build that reports **no space left on device** is that RAM disk filling, not
your disk. `--status` says so, and a plain `prune.sh` usually fixes it.

```
backend/     the Rust crate
frontend/    the interface, built into frontend/dist and served by the backend
site/        the showcase site, deployed separately, never in the image
```

## What CI will check

Run these before opening a pull request; they are the same gates, and they are
all fast except the last. CI only runs the areas a commit touches — a site-only
change does not pay for the Rust suite — but it runs everything when it cannot
work out what changed, so do not rely on that to skip a check locally.

```bash
# backend/
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
cargo test --locked
cargo llvm-cov --summary-only --fail-under-lines 90 --ignore-filename-regex 'src/(tests/|main\.rs)'
cargo audit
python3 ../scripts/check-locales.py
python3 ../scripts/check-api-types.py   # the response structs against types.ts
python3 ../scripts/check-versions.py    # every place a version is written

# frontend/
npm run format:check
npm run check          # svelte-check
npm run lint           # ESLint, type-aware
npm run coverage       # vitest, with floors
npm audit --audit-level=high
npm run test:e2e       # builds the release binary; a few minutes
npm run test:e2e:base  # the same journeys under ROUTARR_BASE_PATH=/routarr

# site/
npm run build && node check.mjs && node verify.mjs
npx astro check        # types over the components and the catalogue

# repository root, with Docker — starts the image and checks it serves
docker build -t routarr:smoke . && bash scripts/smoke-image.sh routarr:smoke
```

Coverage has floors on both sides — 90 % of backend lines, and 80 / 68 / 76 /
80 for frontend statements, branches, functions and lines. **Raise one when the real figure moves up; never lower one to make a
build pass.**

## What a good change looks like

- **Tests are claims about behaviour**, and named that way:
  `an_empty_upstream_response_does_not_wipe_the_library`, not `test_sync_2`.
  Before you trust a new test, break the thing it guards and watch it fail — a
  test that passes for the wrong reason is worse than none.
- **Comments explain the constraint, not its history.** Write, in the present
  tense, what would break if the code changed: the failure a guard exists for,
  the external limitation being worked around, the invariant being held. What
  was tried first, what a measurement read on the day, and which change
  introduced it belong in the Git history, an issue or an architecture note —
  not beside the code. Prefer a clearer name, a named constant or an extracted
  function to a comment explaining an unclear one.
- **Anything the backend produces for a user is translated.** Strings are keys
  in [`backend/locales/`](backend/locales/); the rule engine never emits prose.
  Adding a key to `en.json` is enough — the other 25 languages fall back to
  English key by key, and `scripts/check-locales.py` will tell you if you have
  broken parity or left a key nobody references.
- **A new endpoint touches `api/` *and* the route table in `main.rs`.** They are
  separate on purpose and neither is generated.

## Things that will be turned down

Not because they are bad ideas, but because they are outside what this is:

- a permission model — there is one level of access, whichever mode lets
  someone in;
- anything that phones home, including opt-in telemetry;
- a second database engine;
- fetching anything from a third party at run time, fonts included.

## Commits

Messages follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
fix(auth): honour a session only under the mode that opened it

The body, wrapped at 72 columns, says why the change was needed: the
failure it prevents, the constraint it answers. A body that only
restates the diff says what the diff already says.
```

- **type** — `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`
  or `chore`, one per commit: split work that needs two.
- **scope** — the area touched: `backend`, `frontend`, `site`, `auth`,
  `docker`, `devcontainer`, `release`… Leave it out when nothing narrower fits.
- **subject** — imperative, lower-case, no full stop, 72 characters at most.
- A change that breaks an existing installation says so in a
  `BREAKING CHANGE:` footer, whatever its type.

## Releasing

One tag, one image, one release. There is no release branch and no bot: a
version is published because somebody decided to publish it.

1. Set the version in the three files that carry it — `backend/Cargo.toml`,
   `frontend/package.json`, `site/package.json` — and run
   `python3 scripts/check-versions.py`, which fails naming any file left behind.
   `npm version X.Y.Z --no-git-tag-version --ignore-scripts` in `frontend/` and
   `site/` moves each lockfile with its manifest, and any `cargo` command run
   in `backend/` does the same for `Cargo.lock`; CI builds with `--locked`.
2. Commit, push, and let CI finish. The release refuses a commit whose workflows
   are not green, so tagging ahead of them only wastes a tag.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`.

The tag builds the image for both architectures, pushes it to GHCR with a signed
provenance attestation, and opens a **draft** release carrying the generated
list of commits. The image is published under four tags derived from the git
tag — `X.Y.Z`, `X.Y`, `X` (from 1.0.0 on, since 0.x promises nothing across
minors) and `latest` — and a tag with a hyphen in it (`v1.2.0-rc.1`) publishes
its exact version only.

4. Write, at the top of that draft, the few lines saying what changed for
   someone running Routarr. The generated list stays underneath for whoever
   wants the detail. Then publish it.

The draft is the step that is easy to forget and the only one a person has to
do: the image is on GHCR from step 3, so nothing is blocked while it waits — but
until it is published there is no release to point anyone at.

To undo, delete the draft and the tag. A version already published is corrected
by tagging the next patch, never by moving a tag: `latest` follows the newest
stable tag, and an image somebody has already pulled cannot be recalled.

The provenance attestation needs the repository to be **public**: GitHub does
not issue one for a private repository outside Enterprise, and the release job
fails on that step.

## Licence

Routarr is under the [GNU General Public License, version 3](LICENSE), as the
Servarr applications it works with are. Opening a pull request means you agree
your contribution ships under it — there is no separate agreement to sign, and
nothing is asked of you beyond that sentence.
