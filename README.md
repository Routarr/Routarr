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

> [!NOTE]
> Nothing is moved until you turn the global dry-run off. Back up your `data/` directory before
> upgrading from one version to the next.

![The simulation screen: moves proposed for the library, each with its source and target folder,
the rule that matched, a confidence and a justification.](.github/assets/simulation.webp)

## Features

- **Rules**: 28 conditions (genres, keywords, language, country, certification, tags, paths, size…),
  ALL/ANY, exclusions, priorities, manual exceptions, and an impact preview before saving.
- **Explained decisions**: for every title, what each condition expected and what it found,
  including the rules that matched and lost.
- **Safe by default**: six gates in a fixed order, the first to object stopping the rest (global
  dry-run, batch limit, reachability, capacity, a confirmation threshold you set, and a revalidation
  at the moment of writing). Every applied move can be reverted.
- **Metadata without API keys**: Radarr and Sonarr themselves, AniList and Jikan, with TMDb,
  OMDb and TheTVDB optional, in the order you choose.
- **Real time**: Radarr/Sonarr webhooks, and optional routing of new titles before they download.
- **Several instances** of Radarr 4.2 or later and Sonarr 4 or later, on Linux or Windows: a
  path on a drive letter or a network share is compared as Windows compares it, whatever its case
  or separator. Sonarr 3 works too, a new series routed at the next sync rather than before it
  downloads.
- **Authentication on by default**: API key, a single account, or OpenID Connect.
- **Keys for other applications**, each limited to what you allow. Every key reads, and may also
  operate (sync, simulate, apply, revert, run the rule tests, take a backup), set exceptions, or
  configure the rules, categories and folders, with the guardrails it may answer on its own.
  What they may call is a stable contract, served by your instance at `/api/v1/openapi.json`
  and listed on its API reference screen. A first call is at
  [routarr.app/api](https://routarr.app/api/).
- **Where would it go?** Ask by TMDb, TheTVDB or IMDb id, before the title is even added: a
  request bot learns the folder the rules choose, and nothing is stored.
- **Backups**, configuration export, Prometheus metrics, and notifications written for Discord,
  ntfy, Gotify or Apprise, or as JSON for any other receiver, signed as Standard Webhooks
  specifies when a receiver checks where they come from. A test message checks the address.
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
    read_only: true
    tmpfs:
      - /tmp:size=64m,noexec,nosuid
```

```bash
mkdir -p data && sudo chown 1000:1000 data   # the container runs as uid 1000
docker compose up -d
docker exec routarr cat /data/routarr.api_key  # the API key generated on first start
```

Open **http://localhost:9876** and paste the key. Images are published for `linux/amd64` and
`linux/arm64`. `latest` follows the newest release, `0.1` follows the patch releases of 0.1, and
a full version such as `0.1.0` pins one. From 1.0.0 a major tag (`1`) follows a major line too.

Back up the whole `data/` directory: the Arr and metadata keys, the notification address and the
signing secret stored in the database cannot be read without the `routarr.key` file beside it, or
the `ROUTARR_SECRET_KEY` that replaces it. Routarr also archives itself into `data/backups/`, every day unless you change the interval.
An archive carries the master key: set a backup passphrase in **Settings** to encrypt every one,
in the [age](https://age-encryption.org) format, so that a copy of the folder opens nothing without
it. A restore asks for it, and `age -d` opens an archive where Routarr cannot run.

A start that applies new migrations archives the database first. Going back to an earlier release
works while it knows every migration the database holds. When a start refuses the database, it
names an archive it can open: restore it with the server stopped, through
`docker compose run --rm routarr /app/routarr restore routarr-backup-<date>.zip`, then start
Routarr. An encrypted archive asks for its passphrase when the database in place does not hold it.

## Stronger isolation

The container already runs as uid 1000 with every capability dropped. To put a kernel of its own
between Routarr and your host as well, run the same image under
[gVisor](https://gvisor.dev/docs/user_guide/install/), which answers every system call in user
space and runs on any Linux host, a NAS or a VPS included. Install it as its guide says (its
installer registers `runsc` with Docker), add `runtime: runsc` under `routarr:` in the compose
file, and run `docker compose up -d` again. `docker exec routarr uname -r` then prints gVisor's
kernel, not the host's. CI runs the image's whole start-up check under gVisor on every change to
the image.

It protects the host from a compromised Routarr: reaching the host no longer takes one flaw in
your kernel. It does not protect what Routarr holds, the Arr keys and the `data/` directory, which
a compromised Routarr reads either way. Disk access is slower under gVisor, which a single SQLite
file barely feels.

## Getting started

On a fresh install the dashboard walks through these steps and ticks each one once it is done.

1. **Instances**: add Radarr or Sonarr. The form tries the address and the key, and saving reads
   the library at once.
2. **Categories and folders**: create categories, map the folders your Arrs report to them, and declare
   any destination they do not list. A declared destination is written as the Arr sees it:
   `/data/movies/4k` for an Arr in a Linux container, `D:\Movies\4K` or `\\nas\films` for one on
   Windows.
3. **Metadata sources** (optional): Radarr and Sonarr already supply genres, language and
   certification. Enable TMDb or another source there for keywords and origin country.
4. **Rules**: write rules and preview their impact.
5. **Simulation**: run it, read the justifications, apply what you agree with.
6. **Settings** (optional): turn off the global dry-run once you trust the result.

## Configuration

| Variable | Default | |
|---|---|---|
| `ROUTARR_AUTH` | `apikey` | `apikey`, `forms`, `oidc`, `external` or `none` |
| `ROUTARR_ALLOWED_HOSTS` | *(empty)* | Under `none`, the host names Routarr answers to beside its addresses |
| `ROUTARR_API_KEY` | *(generated)* | Sets the API key instead of generating one |
| `ROUTARR_SECRET_KEY` | *(generated)* | Encrypts the stored Radarr/Sonarr keys |
| `ROUTARR_BASE_PATH` | *(empty)* | Sub-path behind a reverse proxy, e.g. `/routarr` |
| `ROUTARR_LOG_LEVEL` | `info` | Log verbosity |

Every variable is listed and commented in [`backend/.env.example`](backend/.env.example). Everything
else is set in the interface, under **Settings** and **Metadata sources**.

Behind a reverse proxy in `apikey`, `forms` or `oidc` mode, forward the public host and scheme
(`X-Forwarded-Host`, `X-Forwarded-Proto`), and the port in `X-Forwarded-Port` when it is not 80 or
443: the host is what a write from the browser is checked against, the scheme what marks the
session cookie `Secure`. A refused sign-in is logged, sign-ins are checked at most three at a
time per client address, and an address that fails five times waits before its next attempt.
List the proxy in `ROUTARR_TRUSTED_PROXIES`, by address or by range such as `172.18.0.0/16`, so
the client it forwards in `X-Forwarded-For` is counted, not the proxy. In `oidc` mode the provider
and the redirect URL have to be `https://`, except on `localhost`, and
`ROUTARR_OIDC_ALLOWED_SUBJECTS` or `ROUTARR_OIDC_ALLOWED_GROUPS` names who may sign in: a provider
left at its defaults would let every account of its directory in. In `external` mode, publish the port to
the proxy alone: the proxy is what signs people in. In `forms` mode, a lost password is reset with
`docker exec routarr /app/routarr reset-account`, which prints a new one and signs everyone out.
Add `--revoke-keys` to revoke every application key and replace the API key as well.

In `none` mode Routarr answers to its addresses and `localhost`. Reached by a name, such as
`nas.lan`, list it in `ROUTARR_ALLOWED_HOSTS`: a page of another site can make its own name point at
Routarr's address, and the browser would then let it act as Routarr.

## Contributing

Rust (Axum, SQLx) and Svelte 5, one SQLite database, one image. [`CONTRIBUTING.md`](CONTRIBUTING.md)
covers the setup, the dev container and the checks CI runs. Security issues go through
[`SECURITY.md`](SECURITY.md).

## Licence

[GPL-3.0-only](LICENSE), like Radarr and Sonarr. Routarr is not affiliated with the Servarr project.
