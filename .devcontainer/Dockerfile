# Dev container for Routarr: system tooling only, the source comes through the
# bind mount. Rust follows rust-toolchain.toml, Node the major CI and the image use.
FROM rust:1.99.0@sha256:81015672d6939b0265f09adf96d5de125482bb1330106076956b0518e74edc90
# Without pipefail, a download failing on the left of a pipe goes unnoticed.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# psmisc gives `fuser`, which frontend/e2e/run.sh frees its ports with.
# imagemagick encodes the README's screenshot (site/screenshots/run.sh).
# Comments stay outside RUN: a `#` in a continued line is not portable.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    python3 \
    git \
    bash \
    xz-utils \
    psmisc \
    imagemagick \
  && rm -rf /var/lib/apt/lists/*

# Node from its tarball, pinned with the release's SHASUMS256.txt: Debian's is
# older and has no npm. check-versions.py holds the major to CI's and the image's.
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
# No libssl-dev (rustls) and no libsqlite3-dev (SQLite is bundled).

# cargo-llvm-cov, CI's coverage gate, from its release. It publishes no checksum
# file: the sums are those taiki-e/install-action records for ci.yml's version.
# It answers only as `cargo llvm-cov`, its parser requiring the subcommand word.
ARG CARGO_LLVM_COV_VERSION=0.9.1
ARG CARGO_LLVM_COV_SHA256_X64=3fca950394a3c49457657c158b1619cec8dfd2647ae5b48746734c0ab969a522
ARG CARGO_LLVM_COV_SHA256_ARM64=49c0085628392a8d727c4464d91ec417c613c5015f5197cd16d5c05aa3fb5ad1
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

# cargo about and cargo auditable at the root Dockerfile's versions and sums
# (check-versions.py), so a new crate's licence is met before the image build.
ARG CARGO_ABOUT_VERSION=0.9.2
ARG CARGO_ABOUT_SHA256_X64=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af
ARG CARGO_ABOUT_SHA256_ARM64=af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8
ARG CARGO_AUDITABLE_VERSION=0.7.7
ARG CARGO_AUDITABLE_SHA256_X64=7676443367c5e33d9fb22acef63cccb5e9202de12dae1b0579995eb3e2bc7a18
ARG CARGO_AUDITABLE_SHA256_ARM64=c324af56990601354bbf40080f582ba46f8ec8b807a1388e25bb7e429d285c18
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; about="$CARGO_ABOUT_SHA256_X64"; auditable="$CARGO_AUDITABLE_SHA256_X64" ;; \
       arm64) target=aarch64-unknown-linux-musl; about="$CARGO_ABOUT_SHA256_ARM64"; auditable="$CARGO_AUDITABLE_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/cargo-about.tgz \
     "https://github.com/EmbarkStudios/cargo-about/releases/download/${CARGO_ABOUT_VERSION}/cargo-about-${CARGO_ABOUT_VERSION}-${target}.tar.gz" \
  && echo "$about  /tmp/cargo-about.tgz" | sha256sum -c - \
  && tar -xzf /tmp/cargo-about.tgz -C /usr/local/bin --strip-components=1 \
     "cargo-about-${CARGO_ABOUT_VERSION}-${target}/cargo-about" \
  && curl -fsSL -o /tmp/cargo-auditable.txz \
     "https://github.com/rust-secure-code/cargo-auditable/releases/download/v${CARGO_AUDITABLE_VERSION}/cargo-auditable-${target}.tar.xz" \
  && echo "$auditable  /tmp/cargo-auditable.txz" | sha256sum -c - \
  && tar -xJf /tmp/cargo-auditable.txz -C /usr/local/bin --strip-components=1 \
     "cargo-auditable-${target}/cargo-auditable" \
  && rm /tmp/cargo-about.tgz /tmp/cargo-auditable.txz \
  && cargo about --version && cargo auditable --version

# The components rust-toolchain.toml names, so the first `cargo fmt` or
# rust-analyzer start does not stall on a download. Before CARGO_HOME moves:
# the /cargo volume would shadow rustup's proxies.
COPY backend/rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
# `cd`, not WORKDIR: the directory goes in the same step.
# hadolint ignore=DL3003
RUN cd /tmp/toolchain \
  && rustup show \
  && rustup component list --installed | grep -q '^rust-analyzer' \
  && rustup component list --installed | grep -q '^llvm-tools' \
  && rm -rf /tmp/toolchain

# The GitHub CLI from its release, not the vendor apt repository. Its
# credentials are in ~/.config/gh, a volume, so a rebuild keeps them.
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

# The linters, the secret scanner, the licence gate and the contract gate CI
# runs, at ci.yml's versions and amd64 sums (check-versions.py). The arm64 sums
# are recorded from the release archives.
ARG ACTIONLINT_VERSION=1.7.12
ARG ACTIONLINT_SHA256_X64=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
ARG ACTIONLINT_SHA256_ARM64=325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) sum="$ACTIONLINT_SHA256_X64" ;; \
       arm64) sum="$ACTIONLINT_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/actionlint.tgz \
     "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_linux_${arch}.tar.gz" \
  && echo "$sum  /tmp/actionlint.tgz" | sha256sum -c - \
  && tar -xzf /tmp/actionlint.tgz -C /usr/local/bin actionlint \
  && rm /tmp/actionlint.tgz \
  && actionlint -version

ARG SHELLCHECK_VERSION=0.11.0
ARG SHELLCHECK_SHA256_X64=8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198
ARG SHELLCHECK_SHA256_ARM64=12b331c1d2db6b9eb13cfca64306b1b157a86eb69db83023e261eaa7e7c14588
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64; sum="$SHELLCHECK_SHA256_X64" ;; \
       arm64) target=aarch64; sum="$SHELLCHECK_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/shellcheck.txz \
     "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VERSION}/shellcheck-v${SHELLCHECK_VERSION}.linux.${target}.tar.xz" \
  && echo "$sum  /tmp/shellcheck.txz" | sha256sum -c - \
  && tar -xJf /tmp/shellcheck.txz -C /usr/local/bin --strip-components=1 \
     "shellcheck-v${SHELLCHECK_VERSION}/shellcheck" \
  && rm /tmp/shellcheck.txz \
  && shellcheck --version

ARG HADOLINT_VERSION=2.15.1
ARG HADOLINT_SHA256_X64=c7187db94eeeeca956519a6af171adc31453941a1e777961f6e680f697c8c507
ARG HADOLINT_SHA256_ARM64=f6198ef8090f404dbb771abfee086eb8c48ac177f30da7fd3510aca35b344b5d
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64; sum="$HADOLINT_SHA256_X64" ;; \
       arm64) target=arm64; sum="$HADOLINT_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /usr/local/bin/hadolint \
     "https://github.com/hadolint/hadolint/releases/download/v${HADOLINT_VERSION}/hadolint-linux-${target}" \
  && echo "$sum  /usr/local/bin/hadolint" | sha256sum -c - \
  && chmod 0755 /usr/local/bin/hadolint \
  && hadolint --version

ARG GITLEAKS_VERSION=8.30.1
ARG GITLEAKS_SHA256_X64=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
ARG GITLEAKS_SHA256_ARM64=e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x64; sum="$GITLEAKS_SHA256_X64" ;; \
       arm64) target=arm64; sum="$GITLEAKS_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/gitleaks.tgz \
     "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/gitleaks_${GITLEAKS_VERSION}_linux_${target}.tar.gz" \
  && echo "$sum  /tmp/gitleaks.tgz" | sha256sum -c - \
  && tar -xzf /tmp/gitleaks.tgz -C /usr/local/bin gitleaks \
  && rm /tmp/gitleaks.tgz \
  && gitleaks version

ARG OASDIFF_VERSION=1.32.1
ARG OASDIFF_SHA256_X64=7c8939fc49b75ee11fec66a5b83b37a2fca6aee109fed85013b1ba2ac2a1ee7f
ARG OASDIFF_SHA256_ARM64=32fff58a120f75a723d6c2422444691c37fa6813fed61d23f53dbcb604b30f6d
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) sum="$OASDIFF_SHA256_X64" ;; \
       arm64) sum="$OASDIFF_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/oasdiff.tgz \
     "https://github.com/oasdiff/oasdiff/releases/download/v${OASDIFF_VERSION}/oasdiff_${OASDIFF_VERSION}_linux_${arch}.tar.gz" \
  && echo "$sum  /tmp/oasdiff.tgz" | sha256sum -c - \
  && tar -xzf /tmp/oasdiff.tgz -C /usr/local/bin oasdiff \
  && rm /tmp/oasdiff.tgz \
  && oasdiff --version

ARG RUFF_VERSION=0.16.10
ARG RUFF_SHA256_X64=ad1b2138407a0c53b936524df99d87211333f7bc40478d6860f42994597f7ff0
ARG RUFF_SHA256_ARM64=38ebc0ec3e0de538d1c399729ca04e124749211d66aa134c24ad0c1f09a935b9
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; sum="$RUFF_SHA256_X64" ;; \
       arm64) target=aarch64-unknown-linux-musl; sum="$RUFF_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/ruff.tgz \
     "https://github.com/astral-sh/ruff/releases/download/${RUFF_VERSION}/ruff-${target}.tar.gz" \
  && echo "$sum  /tmp/ruff.tgz" | sha256sum -c - \
  && tar -xzf /tmp/ruff.tgz -C /usr/local/bin --strip-components=1 "ruff-${target}/ruff" \
  && rm /tmp/ruff.tgz \
  && ruff --version

ARG CARGO_DENY_VERSION=0.20.2
ARG CARGO_DENY_SHA256_X64=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
ARG CARGO_DENY_SHA256_ARM64=995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d
RUN arch="$(dpkg --print-architecture)" \
  && case "$arch" in \
       amd64) target=x86_64-unknown-linux-musl; sum="$CARGO_DENY_SHA256_X64" ;; \
       arm64) target=aarch64-unknown-linux-musl; sum="$CARGO_DENY_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && curl -fsSL -o /tmp/cargo-deny.tgz \
     "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/cargo-deny-${CARGO_DENY_VERSION}-${target}.tar.gz" \
  && echo "$sum  /tmp/cargo-deny.tgz" | sha256sum -c - \
  && tar -xzf /tmp/cargo-deny.tgz -C /usr/local/bin --strip-components=1 \
     "cargo-deny-${CARGO_DENY_VERSION}-${target}/cargo-deny" \
  && rm /tmp/cargo-deny.tgz \
  && cargo-deny --version

# Playwright's system libraries, at frontend/package.json's version. Only the
# browsers download at create time.
ARG PLAYWRIGHT_VERSION=1.64.0
RUN npx --yes playwright@${PLAYWRIGHT_VERSION} install-deps chromium firefox webkit \
  && rm -rf /root/.npm /var/lib/apt/lists/*

# Persistent cargo cache (volume mounted by devcontainer.json).
ENV CARGO_HOME=/cargo
RUN mkdir -p /cargo

# On a volume, so a rebuild does not download the browsers again.
ENV PLAYWRIGHT_BROWSERS_PATH=/playwright
RUN mkdir -p /playwright

CMD ["bash"]
