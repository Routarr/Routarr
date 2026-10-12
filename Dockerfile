# Every `FROM` is `image:tag@digest`. The digest is what Docker pulls, and the
# tag beside it is ignored for resolution. That is the pinning `ci.yml` applies
# to every `uses:`, for the reason stated there: a tag is a mutable pointer its
# owner can move. It matters more here. An action contributes one build step,
# while a base image contributes the whole filesystem of what `release.yml`
# pushes to GHCR under a signed provenance attestation, and an attestation over
# a silently republished base certifies exactly that.
#
# The tag stays for two readers. Dependabot tracks it and bumps tag and digest
# in one change, where a bare `image@digest` has no tag to follow and would
# receive no pull request at all. And `scripts/check-versions.py` reads it. It
# cannot move into a comment beside the `FROM`: Dockerfile has no trailing
# comments, and anything after the image is parsed as arguments.
#
# The digests are the multi-architecture index, not one platform's manifest,
# since the image is built for amd64 and arm64.

# ---------------------------------------------------------------- frontend
FROM node:24-alpine@sha256:ebfe2f90462722a7a4de65e91990e97fe0d401c70e0e762c5b53302f905ec1c1 AS frontend-builder
WORKDIR /app/frontend

# Dependencies are their own layer so editing a component does not reinstall npm.
# No install scripts: the build needs none, and one would run whatever a
# compromised dependency ships inside the image build.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --ignore-scripts

COPY frontend/ ./
RUN npm run build

# ---------------------------------------------------------------- backend
FROM rust:1.98.1-alpine@sha256:7cc1c22d77d9432f7fe012a70e6d3e555af54c2a6832700ed7d553f1769ae89f AS backend-builder
RUN apk add --no-cache musl-dev
# The RUN below pipes, and without this the right-hand side's exit code masks
# a failing check on the left.
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

# Two cargo subcommands, pinned to a release and to the sum of its archive as
# `ci.yml` pins its linters, so a replaced asset fails the build rather than
# running in it. `cargo auditable` writes the crate list into the binary, where
# an image scanner and the SBOM attestation read it. `cargo about` writes the
# licence notices of the crates. The dev container installs the same versions,
# which `scripts/check-versions.py` holds in step.
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
# `ca-certificates` is load-bearing, not hygiene: reqwest verifies TLS against
# the *platform's* trust store, so without this package every call to TMDB,
# OMDb, TheTVDB, AniList and Jikan fails certificate validation, and so does
# every Arr reached over HTTPS. It is also what makes a homelab CA work: mount
# it into /usr/local/share/ca-certificates and run update-ca-certificates.
RUN apk add --no-cache ca-certificates tzdata

# What the image is, in the vocabulary a registry reads. `licenses` is the one
# that matters here: a published image that declares nothing is a published
# image whose terms nobody can see without finding the repository first.
LABEL org.opencontainers.image.title="Routarr" \
      org.opencontainers.image.description="A routing and classification layer for the Servarr ecosystem" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/Routarr/Routarr" \
      org.opencontainers.image.url="https://github.com/Routarr/Routarr"

# Run unprivileged, as the Servarr convention's uid 1000: with the read-only
# root the compose file sets, the data volume and a small /tmp are all the
# process can write.
RUN addgroup -g 1000 routarr && adduser -D -u 1000 -G routarr routarr

WORKDIR /app
ENV ROUTARR_PORT=9876 \
    ROUTARR_HOST=0.0.0.0 \
    ROUTARR_DB_PATH=/config/routarr.db \
    ROUTARR_FRONTEND_DIR=/app/frontend/dist \
    ROUTARR_LOG_LEVEL=info

COPY --from=backend-builder /app/target/release/routarr /app/routarr
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

# GPLv3 §4 and §6: a copy of the licence travels with the program. The image is
# how almost everyone will receive Routarr, so this is the copy that counts:
# the one in the repository reaches whoever already found the repository.
COPY LICENSE /app/LICENSE
# The notices the MIT, Apache, BSD, ISC and Unicode licences ask to travel with
# every copy. In place of the interface's own list, served beside it, which
# its About dialog links.
COPY --from=notices /app/third-party-notices.txt /app/frontend/dist/third-party-notices.txt

# Only the volume is the process's own. `/app` stays root's, read-only to
# uid 1000: nothing under it is written at run time, and a process that can
# rewrite its own binary or the `index.html` it serves turns any code
# execution into a persistent one.
RUN mkdir -p /config && chown routarr:routarr /config
# Numeric, so a host without the image's /etc/passwd still resolves it.
USER 1000:1000

EXPOSE 9876
# `/config`, as Radarr and Sonarr keep theirs. A volume still mounted at `/data`
# keeps its database: Routarr opens it there while `/config` holds none.
VOLUME ["/config"]

# The binary probes itself: it reads the host, the port and the mount point as
# the server does, so `ROUTARR_BASE_PATH=routarr`, a changed ROUTARR_PORT and a
# ROUTARR_HOST naming one address probe the address actually served. A wrong
# address would report a healthy container as unhealthy, and an orchestrator
# restarts it in a loop. `/api/v1/ping` is deliberately unauthenticated, so this
# works with an API key set.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/routarr", "healthcheck"]

CMD ["/app/routarr"]
