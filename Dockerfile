# syntax=docker/dockerfile:1

# Every `FROM` is `image:tag@digest`. The digest is what Docker pulls — the tag
# beside it is ignored for resolution — which is the pinning `ci.yml` already
# applies to every `uses:`, for the reason stated there: a tag is a mutable
# pointer its owner can move. It matters more here. An action contributes one
# build step; a base image contributes the whole filesystem of what
# `release.yml` pushes to GHCR under a signed provenance attestation, and an
# attestation over a silently republished base certifies exactly that.
#
# The tag stays for two readers. Dependabot tracks it and bumps tag and digest
# in one change; a bare `image@digest` has no tag to follow and would receive no
# pull request at all. And `scripts/check-versions.py` reads it. It cannot move
# into a comment beside the `FROM`: Dockerfile has no trailing comments, and
# anything after the image is parsed as arguments.
#
# The digests are the multi-architecture index, not one platform's manifest —
# the image is built for amd64 and arm64.

# ---------------------------------------------------------------- frontend
FROM node:24-alpine@sha256:50c8e8ca1d27439048670df5883f32d57cf81cff6233222c893fd0d9884cbd81 AS frontend-builder
WORKDIR /app/frontend

# Dependencies are their own layer so editing a component does not reinstall npm.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
RUN npm run build

# ---------------------------------------------------------------- backend
FROM rust:1.98.1-alpine@sha256:1716b3aa042d735f4566d14dc54e8037de9d69556e2d5dd58131d93a613d173d AS backend-builder
RUN apk add --no-cache musl-dev pkgconfig

WORKDIR /app

# Compile the dependency graph against a stub binary first: a source-only change
# then reuses this layer instead of rebuilding ~200 crates.
COPY backend/Cargo.toml backend/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY backend/migrations ./migrations
# The locale dictionaries are include_str!'d from src/localization.rs — without
# them the compile fails, silently making i18n a host-only feature.
COPY backend/locales ./locales
COPY backend/src ./src
# Touch so cargo does not mistake the stub's mtime for an up-to-date build.
RUN touch src/main.rs && cargo build --release --locked

# ---------------------------------------------------------------- runtime
FROM alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b AS runner
# `ca-certificates` is load-bearing, not hygiene: reqwest verifies TLS against
# the *platform's* trust store, so without this package every call to TMDb,
# OMDb, TheTVDB, AniList and Jikan fails certificate validation — and an Arr
# reached over HTTPS with it. It is also what makes a homelab CA work: mount it
# into /usr/local/share/ca-certificates and run update-ca-certificates.
# No `wget` package: the HEALTHCHECK's `wget -qO-` is BusyBox's applet, which
# Alpine ships in the base image, and the GNU one only added a binary.
RUN apk add --no-cache ca-certificates tzdata

# What the image is, in the vocabulary a registry reads. `licenses` is the one
# that matters here: a published image that declares nothing is a published
# image whose terms nobody can see without finding the repository first.
LABEL org.opencontainers.image.title="Routarr" \
      org.opencontainers.image.description="A routing and classification layer for the Servarr ecosystem" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/Routarr/Routarr" \
      org.opencontainers.image.url="https://github.com/Routarr/Routarr"

# Run unprivileged: a bug in the routing engine should not be able to touch
# anything outside the data volume. Matches the Servarr convention of uid 1000.
RUN addgroup -g 1000 routarr && adduser -D -u 1000 -G routarr routarr

WORKDIR /app
ENV ROUTARR_PORT=9876 \
    ROUTARR_HOST=0.0.0.0 \
    ROUTARR_DB_PATH=/data/routarr.db \
    ROUTARR_FRONTEND_DIR=/app/frontend/dist \
    ROUTARR_LOG_LEVEL=info

COPY --from=backend-builder /app/target/release/routarr /app/routarr
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

# GPLv3 §4 and §6: a copy of the licence travels with the program. The image is
# how almost everyone will receive Routarr, so this is the copy that counts —
# the one in the repository reaches whoever already found the repository.
COPY LICENSE /app/LICENSE

# Only the volume is the process's own. `/app` stays root's, read-only to
# uid 1000: nothing under it is written at run time, and a process that can
# rewrite its own binary or the `index.html` it serves turns any code
# execution into a persistent one.
RUN mkdir -p /data && chown routarr:routarr /data
# Numeric, so a host without the image's /etc/passwd still resolves it.
USER 1000:1000

EXPOSE 9876
VOLUME ["/data"]

# `/api/v1/ping` is deliberately unauthenticated so this works with an API key
# set. The port and the sub-path are read from the environment rather than
# hard-coded: with ROUTARR_PORT changed, a fixed 9876 here reports a healthy
# container as unhealthy, and an orchestrator restarts it in a loop.
# Shell form on purpose: the port and the base path come from the environment,
# which the exec form would not expand.
# hadolint ignore=DL3025
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- "http://127.0.0.1:${ROUTARR_PORT:-9876}${ROUTARR_BASE_PATH:-}/api/v1/ping" || exit 1

CMD ["/app/routarr"]
