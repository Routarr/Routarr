# Routarr 🔀

[![CI](https://github.com/Routarr/Routarr/actions/workflows/ci.yml/badge.svg)](https://github.com/Routarr/Routarr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Routarr/Routarr?sort=semver)](https://github.com/Routarr/Routarr/releases)
[![Licence](https://img.shields.io/badge/licence-GPL--3.0-blue)](LICENSE)

[**routarr.app**](https://routarr.app) shows what it does, how it decides, and what it refuses
to do.

**Routarr** sorts your **Radarr** and **Sonarr** libraries into the right root folder
(`/movies/anime`, `/movies/kids`, `/series/documentaries`) from rules you write, and it explains
every move before it makes one.

> [!NOTE]
> Nothing is moved until you turn the global dry-run off. Back up your `data/` directory before
> upgrading from one version to the next.

![The simulation screen: moves proposed for the library, each with its source and target folder,
the rule that matched, a confidence and a justification.](.github/assets/simulation.webp)

## Features

- **Rules** — 28 conditions (genres, keywords, language, country, certification, tags, paths, size…),
  ALL/ANY, exclusions, priorities, manual overrides, and an impact preview before saving.
- **Explained decisions** — for every title, what each condition expected and what it found,
  including the rules that matched and lost.
- **Safe by default** — six gates in a fixed order, the first to object stopping the rest: global
  dry-run, batch cap, reachability, capacity, a confirmation threshold you set, and a revalidation
  at the moment of writing. Every applied move can be reverted.
- **Metadata without API keys** — Radarr and Sonarr themselves, AniList and Jikan, with TMDb,
  OMDb and TheTVDB optional, in the order you choose.
- **Real time** — Radarr/Sonarr webhooks, and optional routing of new titles before they download.
- **Several instances** of Radarr and Sonarr v3.
- **Authentication on by default** — API key, a single account, or OpenID Connect.
- **Backups**, configuration export, Prometheus metrics, and notifications to Discord, Gotify, ntfy
  or Apprise.
- **26 languages**.

## Quick start

```yaml
services:
  routarr:
    image: ghcr.io/routarr/routarr:latest
    container_name: routarr
    ports:
      - "9876:9876"
    volumes:
      - ./data:/data
    restart: unless-stopped
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
```

```bash
mkdir -p data && sudo chown 1000:1000 data   # the container runs as uid 1000
docker compose up -d
docker exec routarr cat /data/routarr.api_key  # the API key generated on first start
```

Open **http://localhost:9876** and paste the key. Images are published for `linux/amd64` and
`linux/arm64`. `latest` follows the newest release, `0.1` follows the patch releases of 0.1, and
a full version such as `0.1.0` pins one. From 1.0.0 a major tag (`1`) follows a major line too.

Back up the whole `data/` directory: the database cannot be read without the `routarr.key` file
beside it. Routarr also archives itself into `data/backups/`, every day unless you change the interval.

## Getting started

1. **Instances** — add Radarr or Sonarr, test the connection, sync.
2. **Root Folders** — create categories, map the folders your Arrs report to them, and declare
   any destination they do not list.
3. **Rules** — write rules and preview their impact.
4. **Simulation** — run it, read the justifications, apply what you agree with.
5. **Settings** — turn off the global dry-run once you trust the result.

## Configuration

| Variable | Default | |
|---|---|---|
| `ROUTARR_AUTH` | `apikey` | `apikey`, `forms`, `oidc`, `external` or `none` |
| `ROUTARR_API_KEY` | *(generated)* | Sets the API key instead of generating one |
| `ROUTARR_SECRET_KEY` | *(generated)* | Encrypts the stored Radarr/Sonarr keys |
| `ROUTARR_BASE_PATH` | *(empty)* | Sub-path behind a reverse proxy, e.g. `/routarr` |
| `ROUTARR_LOG_LEVEL` | `info` | Log verbosity |

Every variable is listed and commented in [`backend/.env.example`](backend/.env.example). Everything
else is set from the **Settings** page.

Behind a reverse proxy in `forms` or `oidc` mode, forward the public host and scheme
(`X-Forwarded-Host`, `X-Forwarded-Proto`): the first is what a write from the browser is checked
against, the second is what marks the session cookie `Secure`. In `oidc` mode the provider and the
redirect URL have to be `https://`, except on `localhost`.

## Contributing

Rust (Axum, SQLx) and Svelte 5, one SQLite database, one image. [`CONTRIBUTING.md`](CONTRIBUTING.md)
covers the setup, the dev container and the checks CI runs. Security issues go through
[`SECURITY.md`](SECURITY.md).

## Licence

[GPL-3.0-only](LICENSE), like Radarr and Sonarr. Routarr is not affiliated with the Servarr project.
