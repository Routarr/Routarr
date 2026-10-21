# Contributing

Thanks for looking. A few things are worth knowing before you spend time on a
change, because some of them are unusual.

## Start with the architecture

[`CLAUDE.md`](CLAUDE.md) covers the architecture and the conventions, each area's
rules live in [`.claude/rules/`](.claude/rules/), and the reason behind a line that
looks odd is in the comment beside it. It is the right starting point for
almost any change, and it will save you from undoing something on purpose.

A few features have been considered and deliberately left out, because they
complicate the engine or the interface without serving what Routarr is for:
deciding which root folder a media item belongs in, and getting it there
safely. Signal weighting, nested condition groups, temporary exceptions and a
persistent task queue are the ones that come up. Please open an issue before
building one of those rather than after.

## Getting set up

The dev container ([`.devcontainer/`](.devcontainer/)) ships everything the
checks below run except Docker: the pinned Rust toolchain, Node, Python,
cargo-llvm-cov, cargo-deny, cargo-about, cargo-auditable, actionlint,
shellcheck, hadolint, gitleaks, oasdiff, ruff, `gh`, and Playwright with its
browsers, each at the version CI pins. Open the repository in it and
`postCreateCommand` does the rest. At every start it names a tool it lacks or
holds at another version than its Dockerfile pins, which a rebuild settles.

The container runs as root. Under rootless Podman, root inside it is your own
user on the host, so what it writes into the checkout is yours. Under a Docker
daemon running as root, those files belong to root on the host, and
`sudo chown -R "$USER" .` in the checkout gives them back. Under Podman, set
`BUILDAH_FORMAT=docker` where VS Code runs it: Podman builds in the OCI format
by default, which drops the Dockerfile's `SHELL` line, so every `RUN` would go
through `sh` without `pipefail` and say so in a warning.

Without the container, development is supported on Linux, where CI runs. On
macOS or Windows, use the dev container. On Linux you need:

- rustup, which reads [`backend/rust-toolchain.toml`](backend/rust-toolchain.toml)
- Node 24, the major [`site/.node-version`](site/.node-version) names
- Python 3.11 or later
- curl, and `fuser` (psmisc) or `lsof`, which the end-to-end harness frees its
  ports with
- Docker, for the image check
- the CI tools at the versions of the `ARG` lines of
  [`.devcontainer/Dockerfile`](.devcontainer/Dockerfile)

Then install the two trees and a browser:

```bash
(cd frontend && npm ci && npx playwright install --with-deps chromium)
(cd site && npm ci)
```

### Running Routarr while you work

```bash
(cd backend && cargo run)    # the API on :9876, with the interface `npm run build` last built
(cd frontend && npm run dev) # the interface with live reload on :3000, /api/ sent to :9876
```

`cargo run` keeps its library in `backend/data/`, and with no key set it writes
one to `backend/data/routarr.api_key` on its first start. Copy
`backend/.env.example` to `backend/.env` to change a setting: every variable is
described there.

Inside it, the Rust build tree lives in a RAM disk at `/ramdisk` rather than
on your disk. The RAM disk is half your machine's memory, and the build tree is
kept under 24 GiB of it. `CARGO_TARGET_DIR` points there, and everything in it
is gone when the container stops, at the price of one cold `cargo build` when it
comes back. Nothing on your machine has to be prepared for that: the container
runtime makes the mount. On a machine with less than 16 GB of RAM, the build
tree goes to disk instead and survives a restart. Three commands are worth
knowing:

```bash
bash .devcontainer/prune.sh --status   # how full it is, and what is using it
bash .devcontainer/prune.sh            # reclaim the regenerable parts now
(cd backend && cargo clean)            # start the build tree over
```

A build that reports **no space left on device** is that RAM disk filling, not
your disk. `--status` says so, and a plain `prune.sh` usually fixes it.

```
backend/     the Rust crate
frontend/    the interface, built into frontend/dist and served by the backend
site/        the showcase site, deployed separately, never in the image
```

## What CI will check

Run the checks before opening a pull request. `scripts/check.sh` reads the steps of
`.github/workflows/ci.yml` and runs them here, in each step's directory, for the areas the
branch changed since main, uncommitted work included, as CI decides them. It never writes to
the tree, installs nothing, and ends on what passed, failed and was skipped. The dev container
ships every tool at the version CI pins, Docker apart.

```bash
bash scripts/check.sh                    # the areas this branch changed, as CI wakes them
bash scripts/check.sh all                # every area
bash scripts/check.sh backend site       # the areas named: repository, backend, coverage,
                                         # frontend, site, e2e
bash scripts/check.sh --engines chromium,firefox,webkit e2e   # every browser CI runs
bash scripts/check.sh --fail-fast        # stop at the first failure
bash scripts/check.sh --list             # each step, its directory and its command
```

It exits 0 when every step passed, 1 when one failed, and 3 when the rest passed but a step was
skipped: a linter the machine lacks, or the image, which needs Docker. The backend's coverage
gate is `--fail-under-lines 96` (`cargo llvm-cov`), and the image is checked, with Docker, by:

```bash
docker build -t routarr:smoke . && bash scripts/smoke-image.sh routarr:smoke
```

gitleaks reads the whole history: a secret-shaped literal, a realistic fake key
in a test included, keeps failing it after a later commit deletes it. Write a
fake key that no scanner takes for a real one (`test-key`, `x` repeated).

Coverage has floors on both sides: 96 % of backend lines, and 95 / 85 / 93 / 95 for frontend
statements, branches, functions and lines, with 30 % of each of the four in every frontend file,
so a screen with no test fails the run. **Raise one when the real figure moves up, never
lower one to make a build pass.**

## What a good change looks like

- **Tests are claims about behaviour**, and named that way:
  `an_empty_upstream_response_does_not_wipe_the_library`, not `test_sync_2`.
  Before you trust a new test, break the thing it guards and watch it fail: a
  test that passes for the wrong reason is worse than none.
- **Comments explain the constraint, not its history.** Write, in the present
  tense, what would break if the code changed: the failure a guard exists for,
  the external limitation being worked around, the invariant being held. What
  was tried first, what a measurement read on the day, and which change
  introduced it belong in the Git history, an issue or an architecture note,
  not beside the code. Prefer a clearer name, a named constant or an extracted
  function to a comment explaining an unclear one.
- **Anything the backend produces for a user is translated.** Strings are keys
  in [`backend/locales/`](backend/locales/), and the rule engine never emits
  prose. Adding a key to `en.json` is enough: the other 25 languages fall back
  to English key by key, and `scripts/check-locales.py` will tell you if you
  have broken parity or left a key nobody references.
- **A new endpoint touches `api/` *and* the route table in `main.rs`.** They are
  separate on purpose and neither is generated.
- **The API other applications call is a contract.** The routes an application
  key may reach are described in
  [`backend/src/api/contract.rs`](backend/src/api/contract.rs) and pinned in
  [`backend/openapi/v1.json`](backend/openapi/v1.json), which `cargo test` holds
  to the code. A change to a type those routes take or return changes the file:
  pin it again with `ROUTARR_WRITE_CONTRACT=1 cargo test contract` and review
  the difference. Adding is free. Removing, renaming or retyping anything
  documented breaks a client, and CI refuses it against the last release.
  Before 1.0 a minor release may break one on purpose: the line oasdiff prints
  goes in `scripts/api-breaks/since-<last tag>.txt`, and the release notes name
  it.
- **A path is renamed through an alias, never moved.** Add the old and new
  templates to `DEPRECATED_PATHS` in
  [`backend/src/api/deprecated.rs`](backend/src/api/deprecated.rs) and mount the
  old one with the new one's handlers: the contract documents it as deprecated,
  and every answer to it carries `Deprecation` and `Link` headers. An alias
  stays for at least two minor releases and six months. The release before its
  removal sets a `Sunset` date, and the removal itself answers `410 Gone` for
  one release, naming the new path.

## Improving a translation

The 25 languages beside English were translated without native speakers, so a
reader of one of them is exactly who can tell where it reads wrong. Either way
below is welcome, and neither needs the toolchain.

- **Report it** with the
  [Translation](https://github.com/Routarr/Routarr/issues/new?template=translation.yml)
  issue form: the language, where the words show and what a native speaker
  would write instead.
- **Or change it.** Each language is one file in
  [`backend/locales/`](backend/locales/), in the key order of `en.json`. Change
  the value, keep every `{placeholder}` as it is, and run
  `python3 scripts/check-locales.py`, which names a placeholder, a core term or
  a key order that moved. `python3 scripts/add-locale.py <code> < batch.json`
  merges a batch of keys and refuses a lost or invented placeholder, and
  `python3 scripts/translation-context.py batch.json <code>` lists the words
  the file already uses for the batch's terms.

A few things hold every language together:

- **One word per core term.** [`scripts/glossary.json`](scripts/glossary.json)
  gives each term (apply, move, exception, root folder…) the one word each
  language says it with, and the check holds every sentence to it. A better
  word changes the glossary and every sentence using it, in one change.
- **The file's own conventions**: its form of address, its quotation marks,
  and in French a no-break space before `? ! : % »` and after `«`. No em dash,
  no semicolon and no curly quotes, in any language.
- **No plural.** A count is never inflected by its number: the dictionaries
  write `Rules: {count}`, never a noun that agrees with it.
- **The Arrs' menu paths stay in English** (`Settings → General → Security →
  API Key`): they are exact for an Arr in its default language.

## Things that will be turned down

Not because they are bad ideas, but because they are outside what this is:

- A permission model: no user roles beyond the owner and application keys
  limited to their scopes.
- Anything that phones home, including opt-in telemetry.
- A second database engine.
- Fetching anything from a third party at run time, fonts included.

## Commits

Messages follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
fix(auth): honour a session only under the mode that opened it

The body, wrapped at 72 columns, says why the change was needed: the
failure it prevents, the constraint it answers. A body that only
restates the diff says what the diff already says.
```

- **type**: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`
  or `chore`, one per commit, so split work that needs two.
- **scope**: the area touched (`backend`, `frontend`, `site`, `auth`,
  `docker`, `devcontainer`, `release`…). Leave it out when nothing narrower
  fits.
- **subject**: imperative, lower-case, no full stop, 72 characters at most.
- A change that breaks an existing installation says so in a
  `BREAKING CHANGE:` footer, whatever its type.

## Releasing

One tag, one image, one release. There is no release branch and no bot: a
version is published because somebody decided to publish it.

1. Set the version in the three files that carry it (`backend/Cargo.toml`,
   `frontend/package.json` and `site/package.json`) and run
   `python3 scripts/check-versions.py`, which fails naming any file left behind.
   `npm version X.Y.Z --no-git-tag-version --ignore-scripts` in `frontend/` and
   `site/` moves each lockfile with its manifest, and any `cargo` command run
   in `backend/` does the same for `Cargo.lock`. CI builds with `--locked`.
   When the interface changed since the last release, refresh the README's
   screenshot with `bash site/screenshots/run.sh`, which writes
   `.github/assets/simulation.webp` from a throwaway instance.
2. Commit, merge to `main`, and let CI finish. The release refuses a commit that
   is not on `main`, a tag naming another version than `backend/Cargo.toml`, a
   commit CI or the image build did not pass on a push, and a version the
   registry already holds (`scripts/release-guard.sh`, which a dry run can
   call), so tagging ahead of them only wastes a tag.
3. `git tag -a vX.Y.Z -m "Routarr X.Y.Z" && git push origin vX.Y.Z`. Annotated:
   the tag records who made it and when, which a lightweight one does not.

The tag builds the image for both architectures, starts each one through
`scripts/smoke-image.sh`, pushes it to GHCR with the list of its packages
(an SBOM), attests its provenance in GitHub's signed store (SECURITY.md says
how to check it), and opens a **draft** release carrying the generated list of
commits. The image is published under four tags derived from the git tag:
`X.Y.Z`, `X.Y`, `X` (from 1.0.0 on, since 0.x promises nothing across minors)
and `latest`. A tag with a hyphen in it (`v1.2.0-rc.1`) publishes its exact
version only.

4. Write, at the top of that draft, what changed for someone running Routarr,
   under the same two headings every release:

   ```markdown
   ## Before you upgrade

   What to do or know first: each API break `scripts/api-breaks/` lists, a
   setting that moved, a release to pass through. "Nothing." when there is none.

   ## What changed

   A few lines for someone running Routarr, not the list of commits.
   ```

   The generated list stays underneath for whoever wants the detail, without
   Dependabot's updates (`.github/release.yml`). Then publish it, which also
   deploys the showcase site from the tag (`.github/workflows/site.yml`), and
   delete the `scripts/api-breaks/` list: CI refuses one kept past its release.

The draft is the step that is easy to forget and the only one a person has to
do: the image is on GHCR from step 3, so nothing is blocked while it waits, but
until it is published there is no release to point anyone at.

To undo before publishing, delete the draft and the tag: the repository's tag
rules let an administrator alone delete or move a `v*` tag. The image and its
tags stay on GHCR, and the release refuses that version again until it is
deleted from the package's page on GitHub. A published release cannot be
undone: GitHub locks it and its tag (immutable releases). A version already
published is corrected by tagging the next patch, never by moving a tag:
`latest` follows the newest stable tag, and an image somebody has already
pulled cannot be recalled.

The provenance attestation needs the repository to be **public**: GitHub does
not issue one for a private repository outside Enterprise, and the release job
fails on that step.

## Versions

Routarr follows [Semantic Versioning](https://semver.org). A version number
makes its promise about four things a person or another application depends
on: the image and its tags, the API under `/api/v1` as
[`backend/openapi/v1.json`](backend/openapi/v1.json) describes it, the
`ROUTARR_*` environment variables, and the database each release opens.

- **A patch release** (0.2.0 to 0.2.1) breaks none of them. It carries fixes,
  the base image's among them: a security fix in Alpine ships in a patch
  release rather than waiting for the next minor one.
- **Before 1.0, a minor release** (0.2 to 0.3) may break one of them, and its
  notes say so under "Before you upgrade". From 1.0 on, only a major release
  does.
- **An API break is announced one release ahead.** What is to be removed or
  changed is marked `deprecated` in the contract in one release, and changes
  in the next at the earliest. Before 1.0 the break lands in `/api/v1` and
  `scripts/api-breaks/` lists it. From 1.0 on, it goes in `/api/v2`, never in
  `/api/v1`.
- **The database moves forward.** A release migrates the database it opens,
  and an older one refuses a database a newer release migrated further, naming
  that release. Going back to a release with the same schema needs nothing.
  Going back further means restoring the backup taken before the upgrade.
- **Only the newest release is supported**, as [SECURITY.md](SECURITY.md)
  says: a fix ships in the next release, never in an older line.

## Licence

Routarr is under the [GNU General Public License, version 3](LICENSE), as the
Servarr applications it works with are. Opening a pull request means you agree
your contribution ships under it. There is no separate agreement to sign, and
nothing is asked of you beyond that sentence.
