# Every `FROM` is `image:tag@digest`: Docker pulls the digest, so a republished tag cannot change
# what `release.yml` pushes and attests. The tag is for Dependabot, which bumps both, and for
# `scripts/check-versions.py`, and cannot sit in a comment: a Dockerfile has no trailing one.
# Each digest is the multi-architecture index, the image being built for amd64 and arm64.

# ---------------------------------------------------------------- frontend
FROM node:24-alpine@sha256:ebfe2f90462722a7a4de65e91990e97fe0d401c70e0e762c5b53302f905ec1c1 AS frontend-builder
WORKDIR /app/frontend

# No install scripts: the build needs none, and one would run whatever a compromised dependency
# ships inside the image build.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --ignore-scripts

COPY frontend/ ./
RUN npm run build

# ---------------------------------------------------------------- backend
FROM rust:1.99.1-alpine@sha256:557a94c82b485a14b569120e00867267108c734efe33f10cc31ca0cd5dd1eca9 AS backend-builder
RUN apk add --no-cache musl-dev
# The RUN below pipes, and without this the right-hand side's exit code masks
# a failing check on the left.
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

# Pinned to a release and to its archive's sum. `cargo auditable` writes the crate list into the
# binary, for scanners and the SBOM, and `cargo about` the crates' licence notices. The dev
# container installs the same versions (`scripts/check-versions.py`).
ARG CARGO_AUDITABLE_VERSION=0.7.7
ARG CARGO_AUDITABLE_SHA256_X64=7676443367c5e33d9fb22acef63cccb5e9202de12dae1b0579995eb3e2bc7a18
ARG CARGO_AUDITABLE_SHA256_ARM64=c324af56990601354bbf40080f582ba46f8ec8b807a1388e25bb7e429d285c18
ARG CARGO_ABOUT_VERSION=0.9.2
ARG CARGO_ABOUT_SHA256_X64=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af
ARG CARGO_ABOUT_SHA256_ARM64=af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8
RUN arch="$(uname -m)" \
  && case "$arch" in \
       x86_64) auditable="$CARGO_AUDITABLE_SHA256_X64"; about="$CARGO_ABOUT_SHA256_X64" ;; \
       aarch64) auditable="$CARGO_AUDITABLE_SHA256_ARM64"; about="$CARGO_ABOUT_SHA256_ARM64" ;; \
       *) echo "unsupported arch: $arch" && exit 1 ;; \
     esac \
  && wget -qO /tmp/cargo-auditable.txz \
     "https://github.com/rust-secure-code/cargo-auditable/releases/download/v${CARGO_AUDITABLE_VERSION}/cargo-auditable-${arch}-unknown-linux-musl.tar.xz" \
  && echo "$auditable  /tmp/cargo-auditable.txz" | sha256sum -c - \
  && tar -xJf /tmp/cargo-auditable.txz -C /usr/local/bin --strip-components=1 \
     "cargo-auditable-${arch}-unknown-linux-musl/cargo-auditable" \
  && wget -qO /tmp/cargo-about.tgz \
     "https://github.com/EmbarkStudios/cargo-about/releases/download/${CARGO_ABOUT_VERSION}/cargo-about-${CARGO_ABOUT_VERSION}-${arch}-unknown-linux-musl.tar.gz" \
  && echo "$about  /tmp/cargo-about.tgz" | sha256sum -c - \
  && tar -xzf /tmp/cargo-about.tgz -C /usr/local/bin --strip-components=1 \
     "cargo-about-${CARGO_ABOUT_VERSION}-${arch}-unknown-linux-musl/cargo-about" \
  && rm /tmp/cargo-auditable.txz /tmp/cargo-about.tgz \
  && cargo auditable --version && cargo about --version

WORKDIR /app

# Compile the dependency graph against a stub binary first: a source-only change
# then reuses this layer instead of rebuilding every dependency.
COPY backend/Cargo.toml backend/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo auditable build --release --locked \
    && rm -rf src

# The licences of the crates the binary links on either platform (`about.toml`),
# beside the dependency layer, since only the lockfile changes them.
# `cargo fetch` brings the other platform's crates, so `--frozen` reads
# nothing but the sources on disk.
COPY backend/about.toml backend/about.hbs ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo fetch --locked \
    && cargo about generate --frozen about.hbs -o crates-notices.txt \
    && rm -rf src

COPY backend/migrations ./migrations
# The locale dictionaries are include_str!'d from src/localization.rs, so the
# compile fails without them.
COPY backend/locales ./locales
COPY backend/src ./src
# Touch so cargo does not mistake the stub's mtime for an up-to-date build.
RUN touch src/main.rs && cargo auditable build --release --locked

# ---------------------------------------------------------------- notices
# One file for everything the image carries besides Routarr: the crates, then
# the packages the interface bundles, which Vite lists in the same file name.
FROM backend-builder AS notices
COPY --from=frontend-builder /app/frontend/dist/third-party-notices.txt packages-notices.txt
RUN cat crates-notices.txt packages-notices.txt > third-party-notices.txt

# ---------------------------------------------------------------- runtime
FROM alpine:3.24@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS runner
# reqwest verifies TLS against the platform's trust store: without `ca-certificates` every
# metadata source and every Arr over HTTPS fails. A homelab CA is mounted beside it, both named in
# SSL_CERT_DIR: the read-only root leaves update-ca-certificates nothing it may write.
RUN apk add --no-cache ca-certificates tzdata

LABEL org.opencontainers.image.title="Routarr" \
      org.opencontainers.image.description="A routing and classification layer for the Servarr ecosystem" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/Routarr/Routarr" \
      org.opencontainers.image.url="https://github.com/Routarr/Routarr"

# Unprivileged, as uid 1000 like the Servarr images.
RUN addgroup -g 1000 routarr && adduser -D -u 1000 -G routarr routarr

WORKDIR /app
ENV ROUTARR_PORT=9876 \
    ROUTARR_HOST=0.0.0.0 \
    ROUTARR_DB_PATH=/config/routarr.db \
    ROUTARR_FRONTEND_DIR=/app/frontend/dist \
    ROUTARR_LOG_LEVEL=info

COPY --from=backend-builder /app/target/release/routarr /app/routarr
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

# GPLv3 §4 and §6: a copy of the licence travels with the program.
COPY LICENSE /app/LICENSE
# The notices the MIT, Apache, BSD, ISC and Unicode licences ask to travel with
# every copy. In place of the interface's own list, served beside it, which
# its About dialog links.
COPY --from=notices /app/third-party-notices.txt /app/frontend/dist/third-party-notices.txt
# Then the packages of the Alpine base, from this layer's own package database
# so the list is what ships, the GPL-2.0 ones among them, with where their
# sources are.
RUN release="$(cut -d. -f1,2 /etc/alpine-release)" \
  && { printf '\n# Packages of the Alpine base\n\n'; \
       printf 'The image runs on Alpine Linux %s. The source of each package below is in the\n' "$release"; \
       printf 'aports repository, https://gitlab.alpinelinux.org/alpine/aports, on its %s-stable\n' "$release"; \
       printf 'branch, and the upstream archives it builds from are at\n'; \
       printf 'https://distfiles.alpinelinux.org/.\n\n'; \
       awk -F: '/^P:/ { name = $2 } /^V:/ { version = $2 } /^L:/ { print "## " name " - " version " (" substr($0, 3) ")" }' \
         /lib/apk/db/installed; } \
     >> /app/frontend/dist/third-party-notices.txt

# Only /config is the process's own: `/app` stays root's, so code execution cannot rewrite
# the binary or the `index.html` it serves.
RUN mkdir -p /config && chown routarr:routarr /config
# Numeric, so a host without the image's /etc/passwd still resolves it.
USER 1000:1000

EXPOSE 9876
# `/config`, as Radarr and Sonarr keep theirs. A volume mounted at `/data` instead is opened
# there while `/config` holds no database.
VOLUME ["/config"]

# The binary probes itself with the server's own host, port and base path, so a changed one
# never reads as unhealthy and restarts in a loop. `/api/v1/ping` needs no key. Liveness alone:
# a database at fault shows in the warnings and on `/api/v1/health`, not here.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/routarr", "healthcheck"]

CMD ["/app/routarr"]
