# syntax=docker/dockerfile:1

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
RUN apk add --no-cache musl-dev pkgconfig

WORKDIR /app

# Compile the dependency graph against a stub binary first: a source-only change
# then reuses this layer instead of rebuilding every dependency.
COPY backend/Cargo.toml backend/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY backend/migrations ./migrations
# The locale dictionaries are include_str!'d from src/localization.rs, so the
# compile fails without them.
COPY backend/locales ./locales
COPY backend/src ./src
# Touch so cargo does not mistake the stub's mtime for an up-to-date build.
RUN touch src/main.rs && cargo build --release --locked

# ---------------------------------------------------------------- runtime
FROM alpine:3.24@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS runner
# `ca-certificates` is load-bearing, not hygiene: reqwest verifies TLS against
# the *platform's* trust store, so without this package every call to TMDb,
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
    ROUTARR_DB_PATH=/data/routarr.db \
    ROUTARR_FRONTEND_DIR=/app/frontend/dist \
    ROUTARR_LOG_LEVEL=info

COPY --from=backend-builder /app/target/release/routarr /app/routarr
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

# GPLv3 §4 and §6: a copy of the licence travels with the program. The image is
# how almost everyone will receive Routarr, so this is the copy that counts:
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

# The binary probes itself: it reads the port and the mount point as the server
# does, so `ROUTARR_BASE_PATH=routarr` and a changed ROUTARR_PORT probe the
# address actually served. A wrong address would report a healthy container as
# unhealthy, and an orchestrator restarts it in a loop. `/api/v1/ping` is
# deliberately unauthenticated, so this works with an API key set.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/routarr", "healthcheck"]

CMD ["/app/routarr"]
