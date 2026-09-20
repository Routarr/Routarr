# Dev container for Routarr
# The source arrives through the bind mount on /workspaces/Routarr: nothing is
# copied into the image, only the system tooling is installed.
#
# Aligned with every other environment: the Rust version comes from
# rust-toolchain.toml, Node 24 (CI and the production image) and python3
# (scripts/check-locales.py, which CI runs).
FROM rust:1.98.1@sha256:462a9af3c54fb4718850d3c602fc0e54452c20b1c12a4e4080fdb001d4b9acbf
# Every RUN below pipes; without this the right-hand side's exit code masks a
# failing download on the left.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# Node 24 from the official tarball, pinned to a version and to its checksum:
# the Debian package is older and ships without npm, while CI and the
# production Dockerfile are on Node 24 — a dev/CI gap of that kind is only ever
# discovered by the first broken build. NodeSource's setup script was a
# `curl | bash` of unpinned code run as root in an image that pins every other
# tool it installs.
#
# The setup script exits 0 when its own `apt update` fails — it reports `$?`
# from inside `if !`, which is always 0 — and leaves the repository unwritten.
# `apt-get install nodejs` then takes Debian's package without a word, and the
# build fails much later on `npx: not found`, naming neither the mirror nor
# Node. The `test -f` stops it at the cause; `npx --version` checks the outcome.
#
# psmisc provides `fuser`, which frontend/e2e/run.sh uses to free its ports
# before starting. A run killed before its trap fires leaves a server holding
# one, and the next run then silently talks to the stale process instead of
# failing.
#
# imagemagick and libavif-bin are what site/screenshots/run.sh encodes with.
# Both, because an ImageMagick built without an AVIF delegate does not fail: it
# writes a PNG under the .avif name, which the browser refuses and which nothing
# downstream notices, the name being right and the pair complete.
#
# Comments stay outside the RUN: a `#` inside a backslash continuation is not
# reliably stripped by every Docker parser version.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    pkg-config \
    python3 \
    git \
    bash \
    xz-utils \
    psmisc \
    imagemagick \
    libavif-bin \
  && rm -rf /var/lib/apt/lists/*

# `scripts/check-versions.py` reads the major out of NODE_VERSION, against CI
# and the production image. The sums are the two lines of the release's
# SHASUMS256.txt this image can run; a bumped version without bumped sums
# fails the build, which is the point.
ARG NODE_VERSION=24.21.0
ARG NODE_SHA256_X64=fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6
ARG NODE_SHA256_ARM64=6ad1325edbdb5649c379b75a237147a666c95d4f9ae8d340fef2d1575d289ad2
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) node_arch=x64; sum="$NODE_SHA256_X64" ;; \
       arm64) node_arch=arm64; sum="$NODE_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/node.tar.xz \
     "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" \
  && echo "$sum  /tmp/node.tar.xz" | sha256sum -c - \
  && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
     --exclude='*/CHANGELOG.md' --exclude='*/LICENSE' --exclude='*/README.md' \
  && rm /tmp/node.tar.xz \
  && node --version && npm --version && npx --version
# Neither libssl-dev (reqwest uses rustls; there is no openssl in Cargo.lock)
# nor libsqlite3-dev (libsqlite3-sys is built in bundled mode).

# cargo-audit is a blocking CI gate: the prebuilt binary avoids the ~5 minutes
# `cargo install` would cost on every container rebuild.
# The release publishes no checksum file, so the sums are recorded here from
# the archives themselves: a replaced asset under the same name fails the
# build instead of running.
ARG CARGO_AUDIT_VERSION=0.22.2
ARG CARGO_AUDIT_SHA256_X64=7fb9497f8594b389e5fce5ef9b92db08432996895b2e0c5a0167a69ed445c428
ARG CARGO_AUDIT_SHA256_ARM64=c6603814ddaa45e51263dafd31c0ac98808f688d26f7395804f9670b0fd599dd
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; sum="$CARGO_AUDIT_SHA256_X64" ;; \
       arm64) target=aarch64-unknown-linux-gnu; sum="$CARGO_AUDIT_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/cargo-audit.tgz \
     "https://github.com/rustsec/rustsec/releases/download/cargo-audit%2Fv${CARGO_AUDIT_VERSION}/cargo-audit-${target}-v${CARGO_AUDIT_VERSION}.tgz" \
  && echo "$sum  /tmp/cargo-audit.tgz" | sha256sum -c - \
  && tar -xzf /tmp/cargo-audit.tgz -C /tmp \
  && mv "/tmp/cargo-audit-${target}-v${CARGO_AUDIT_VERSION}/cargo-audit" /usr/local/bin/ \
  && rm -rf /tmp/cargo-audit.tgz "/tmp/cargo-audit-${target}-v${CARGO_AUDIT_VERSION}" \
  && cargo-audit --version

# cargo-llvm-cov, for the same reason and from its own release: `cargo install`
# builds it from source on every container rebuild. In the image rather than in
# the `/cargo` volume, so a fresh volume cannot lose a tool CI gates on.
#
# The check is `cargo llvm-cov`, not `cargo-llvm-cov`. This binary is a cargo
# subcommand and nothing else: cargo passes the subcommand as its first
# argument, so its parser *requires* that word and `cargo-llvm-cov --version`
# fails with "expected subcommand 'llvm-cov'". Going through cargo also checks
# the one thing worth checking, that cargo resolves the subcommand from
# /usr/local/bin. It needs no llvm-tools, so it holds before the rustup step.
ARG CARGO_LLVM_COV_VERSION=0.9.0
ARG CARGO_LLVM_COV_SHA256_X64=bc2122fd7bd9b6d3eb371690ed085f82e5a8bf1851f4645ef521d12ed04b2616
ARG CARGO_LLVM_COV_SHA256_ARM64=3c299780e109d59fd77044e64734421d43c0067b92937a49be302fee66d04727
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; sum="$CARGO_LLVM_COV_SHA256_X64" ;; \
       arm64) target=aarch64-unknown-linux-musl; sum="$CARGO_LLVM_COV_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/cargo-llvm-cov.tar.gz \
     "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v${CARGO_LLVM_COV_VERSION}/cargo-llvm-cov-${target}.tar.gz" \
  && echo "$sum  /tmp/cargo-llvm-cov.tar.gz" | sha256sum -c - \
  && tar -xzf /tmp/cargo-llvm-cov.tar.gz -C /usr/local/bin \
  && rm /tmp/cargo-llvm-cov.tar.gz \
  && cargo llvm-cov --version

# The base image ships a bare toolchain (no rustfmt, no clippy, no rust-analyzer),
# so the first `cargo fmt` — or the first rust-analyzer start, which is worse
# because nothing reports it — stalls on a rustup download in a fresh
# container. `rustup show` inside a directory holding the toolchain file
# installs exactly what that file declares, which keeps rust-toolchain.toml the
# only place a component is named; the grep turns a missing one into a failed
# build instead of a slow editor.
#
# This runs *before* CARGO_HOME is repointed: rustup writes its proxies into
# $CARGO_HOME/bin, and /cargo is a volume that would shadow them at run time.
# `backend/`, not the root: that is where the toolchain file lives, and a path
# pointing anywhere else fails the container rebuild on a missing file.
COPY backend/rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
# `cd` rather than WORKDIR: the directory is removed in the same step, and a
# WORKDIR would leave the image standing in a directory that no longer exists.
# hadolint ignore=DL3003
RUN cd /tmp/toolchain \
  && rustup show \
  && rustup component list --installed | grep -q '^rust-analyzer' \
  && rustup component list --installed | grep -q '^llvm-tools' \
  && rm -rf /tmp/toolchain

# RTK (Rust Token Killer): a CLI proxy that compresses command output before an
# AI coding agent reads it. Installed here rather than by `cargo install` for the
# same reason as cargo-audit — the prebuilt binary costs seconds instead of
# minutes on every rebuild.
#
# Two things are deliberate. The download is verified against the checksum
# published with the release and **pinned here**, not fetched alongside the
# archive: a checksum that travels with the file it vouches for proves only that
# the download completed. And the source is the GitHub release, never
# `cargo install rtk` — the `rtk` name on crates.io belongs to an unrelated
# project ("Rust Type Kit"), which the upstream README warns about.
ARG RTK_VERSION=0.46.0
ARG RTK_SHA256_AMD64=79aa5b89c69566bbfeceb66c8a27cfbe52237fc7ee3e683115f43745a3262d21
ARG RTK_SHA256_ARM64=e8c2e1787f46017ea7c5a711b2bc6a7f7cf61c7ad69385b4c1e4daff1135dcd1
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; sha="$RTK_SHA256_AMD64" ;; \
       arm64) target=aarch64-unknown-linux-gnu;  sha="$RTK_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL "https://github.com/rtk-ai/rtk/releases/download/v${RTK_VERSION}/rtk-${target}.tar.gz" \
     -o /tmp/rtk.tar.gz \
  && echo "${sha}  /tmp/rtk.tar.gz" | sha256sum -c - \
  && tar -xz -C /usr/local/bin -f /tmp/rtk.tar.gz rtk \
  && rm -f /tmp/rtk.tar.gz \
  && chmod 0755 /usr/local/bin/rtk \
  && rtk --version

# The GitHub CLI. Same reasoning as cargo-audit and rtk above: the release
# tarball with a checksum pinned here, rather than the vendor apt repository,
# which would add a signing key and a second package index to every build.
#
# Its credentials live in ~/.config/gh, which devcontainer.json mounts as a
# volume. Without it every rebuild would demand a fresh `gh auth login` — the
# same friction the /cargo and /playwright volumes exist to avoid.
ARG GH_VERSION=2.98.0
ARG GH_SHA256_AMD64=3b8ac6b30336802fc1a858d7c084e11cdf24ac1a761ca90b68022d7d729208de
ARG GH_SHA256_ARM64=cf689084f3a3618f7eae4a2420d335d74626d65f5e594b9828d125d69f800d86
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) sha="$GH_SHA256_AMD64" ;; \
       arm64) sha="$GH_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${arch}.tar.gz" \
     -o /tmp/gh.tar.gz \
  && echo "${sha}  /tmp/gh.tar.gz" | sha256sum -c - \
  && tar -xz -C /usr/local/bin --strip-components=2 -f /tmp/gh.tar.gz "gh_${GH_VERSION}_linux_${arch}/bin/gh" \
  && rm -f /tmp/gh.tar.gz \
  && chmod 0755 /usr/local/bin/gh \
  && gh --version

# Playwright's system libraries, installed here rather than by
# `playwright install --with-deps` in postCreateCommand, which would re-run
# apt-get on every rebuild. Keep in step with @playwright/test in
# frontend/package.json; only the browser download stays a create step.
ARG PLAYWRIGHT_VERSION=1.62.1
RUN npx --yes playwright@${PLAYWRIGHT_VERSION} install-deps chromium \
  && rm -rf /root/.npm /var/lib/apt/lists/*

# Persistent cargo cache (volume mounted by devcontainer.json).
ENV CARGO_HOME=/cargo
RUN mkdir -p /cargo

# Playwright browsers live here rather than under the (non-persisted) home
# directory, so a rebuilt container does not re-download 115 MB of Chromium.
ENV PLAYWRIGHT_BROWSERS_PATH=/playwright
RUN mkdir -p /playwright

CMD ["bash"]
