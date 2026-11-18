<h1>
  <img src=".github/assets/logo.svg" width="26" height="26" alt="">
  Routarr
</h1>

[![CI](https://github.com/Routarr/Routarr/actions/workflows/ci.yml/badge.svg)](https://github.com/Routarr/Routarr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Routarr/Routarr?sort=semver)](https://github.com/Routarr/Routarr/releases)
[![Licence](https://img.shields.io/badge/licence-GPL--3.0-blue)](LICENSE)

[**routarr.app**](https://routarr.app) shows what it does, how it decides, and what it refuses
to do.

**Routarr** sorts your **Radarr** and **Sonarr** libraries into the right root folder
(`/movies/anime`, `/movies/kids`, `/series/documentaries`) from rules you write, and it explains
every move before it makes one.

Every title your Arrs fetch already carries its genres, its original language and its
certification, and nothing reads any of it to pick a root folder. Routarr is that missing
decision, made with the folders your Arrs already have.

> [!NOTE]
> Nothing is moved until you turn the global dry-run off. Before upgrading from one version to the
> next, download a backup from **Settings → Maintenance**, or stop Routarr and copy its `config/`
> folder.

![The simulation screen: moves proposed for the library, each with its source and target folder,
the rule that matched, a confidence and a justification.](.github/assets/simulation.webp)

## Features

- **Rules**: 35 conditions (genres, language, scores, tags, size…), and per-title exceptions.
- **Explained decisions**: for every title, what each condition expected and what it found.
- **Safe by default**: six gates before anything is written, and every applied move can be reverted.
- **Metadata without API keys**: the Arrs, AniList and Tenrai, with TMDB and others optional.
- **Real time**: Radarr/Sonarr webhooks, and optional routing of new titles before they download.
- **Several instances** of Radarr 4.2 or later and Sonarr 4 or later, on Linux or Windows.
- **Authentication on by default**: an API key, a single account, or OpenID Connect.
- **Keys for other applications**, each limited to what you allow: [routarr.app/api](https://routarr.app/api/).
- **Where would it go?** A request bot asks by TMDB, TheTVDB or IMDb id, before the title is added.
- **Backups** every day, a configuration export, and Prometheus metrics with a Grafana dashboard.
- **Notifications** written for Discord, ntfy, Gotify or Apprise, or as JSON for any other receiver.
- **26 languages**, decision justifications included.

## Quick start

```yaml
services:
  routarr:
    image: ghcr.io/routarr/routarr:0.3
    container_name: routarr
    ports:
      - "9876:9876"
    volumes:
      - ./config:/config
    restart: unless-stopped
    stop_grace_period: 30s
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:size=64m,noexec,nosuid
```

```bash
mkdir -p config && sudo chown 1000:1000 config   # the container runs as uid 1000
docker compose up -d
docker exec routarr cat /config/routarr.api_key  # the API key generated on first start
```

Open **http://localhost:9876** and paste the key. Images are published for `linux/amd64` and
`linux/arm64`. `stop_grace_period` gives a stop the time to record the moves in flight and close
the database.

The image tag says what an update brings: `X.Y`, as above, follows the patch releases of one
minor, `X.Y.Z` pins one release, and `latest` follows the newest. From 1.0.0, `X` follows a major
line too. Before 1.0, a minor release may break a setting or the API: stay on the minor line, and
move to the next one after reading its release notes, under "Before you upgrade".

## Getting started

On a fresh install the dashboard walks through these steps and ticks each one once it is done.

1. **Instances**: add Radarr or Sonarr. The form tries the address and the key, and saving reads
   the library at once.
2. **Categories and folders**: create categories, map the folders your Arrs report to them, and declare
   any destination they do not list. A declared destination is written as the Arr sees it:
   `/data/movies/4k` for an Arr in a Linux container, `D:\Movies\4K` or `\\nas\films` for one on
   Windows.
3. **Metadata sources** (optional): Radarr and Sonarr already supply genres, language and
   certification. Enable TMDB or another source there for keywords and origin country.
4. **Rules**: write rules and preview their impact.
5. **Simulation**: run it, read the justifications, apply what you agree with.
6. **Settings** (optional): turn off the global dry-run once you trust the result.

## Configuration

| Variable | Default | |
|---|---|---|
| `ROUTARR_AUTH` | `apikey` | `apikey`, `forms`, `oidc`, `external` or `none` |
| `ROUTARR_ALLOWED_HOSTS` | *(empty)* | Under `none`, the host names Routarr answers to beside its addresses |
| `ROUTARR_API_KEY` | *(generated in `apikey` mode)* | Sets the API key instead of generating one. In another mode there is none until Settings makes one or this sets it |
| `ROUTARR_SECRET_KEY` | *(generated)* | Seals the stored credentials: the Arr and metadata keys, the notification address, the signing secret and the backup passphrase |
| `ROUTARR_BASE_PATH` | *(empty)* | Sub-path behind a reverse proxy, e.g. `/routarr` |
| `ROUTARR_BACKUP_COPY_DIR` | *(empty)* | A second folder each backup archive is copied to, on a NAS or another disk |
| `ROUTARR_LOG_LEVEL` | `info` | Log verbosity |
| `TZ` | *(UTC)* | The time zone the log lines are dated in, and the dates a notification or a warning writes, e.g. `Europe/Paris`. The interface shows your browser's |

Every variable is listed and commented in [`backend/.env.example`](backend/.env.example). Everything
else is set in the interface, under **Settings** and **Metadata sources**.

## Running it

[`OPERATING.md`](OPERATING.md) is the guide to running Routarr once it is installed:

- [Upgrading](OPERATING.md#upgrading): the new image, the archive taken first, and going back.
- [Backups and restore](OPERATING.md#backups-and-restore): the archives, a copy off the machine, a restore.
- [Install notes](OPERATING.md#install-notes): without Compose, on a NAS, your own CA, the browsers and Arrs supported.
- [Without Docker](OPERATING.md#without-docker): the deb and rpm repositories, and the tarball.
- [Behind a reverse proxy](OPERATING.md#behind-a-reverse-proxy): the headers to forward, and a sub-path.
- [Sign-in modes](OPERATING.md#sign-in-modes): OpenID Connect, a lost password, and the open mode.
- [When something goes wrong](OPERATING.md#when-something-goes-wrong): the log, and what a bug report takes.
- [Stronger isolation](OPERATING.md#stronger-isolation): the same image under gVisor.
- [Monitoring](OPERATING.md#monitoring): Prometheus metrics, a Grafana dashboard and alert rules.

## Contributing

Rust (Axum, SQLx) and Svelte 5, one SQLite database, one image. [`CONTRIBUTING.md`](CONTRIBUTING.md)
covers the setup, the dev container and the checks CI runs. Security issues go through
[`SECURITY.md`](SECURITY.md).

Questions and ideas go to [Discussions](https://github.com/Routarr/Routarr/discussions), bugs,
translation fixes and accessibility barriers to [the issue forms](https://github.com/Routarr/Routarr/issues/new/choose).
[`ACCESSIBILITY.md`](ACCESSIBILITY.md) says what the interface aims at and how it is checked.

## Licence

Copyright (C) 2026 the Routarr contributors.

[GPL-3.0-only](LICENSE), like Radarr and Sonarr. Routarr is not affiliated with the Servarr project.
The licences of the components it is built with ship in the image, and the About dialog links them.
The logo is Lucide's `git-branch` icon, under the ISC licence in
[`LICENSES/ISC-Lucide.txt`](LICENSES/ISC-Lucide.txt).

## Data sources

The metadata sources you enable are credited in the interface, on the Metadata sources screen and
in About, as their terms ask.

- Routarr uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by
  [TMDB](https://www.themoviedb.org).
- Metadata provided by [TheTVDB](https://thetvdb.com). Please consider adding missing information
  or subscribing.
- Data from [OMDb](https://www.omdbapi.com), licensed under
  [CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/).
- Data from [AniList](https://anilist.co), and from [MyAnimeList](https://myanimelist.net), through
  [Tenrai](https://tenrai.org) or MyAnimeList's own API.
