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
> Nothing is moved until you turn the global dry-run off. Before upgrading from one version to the
> next, download a backup from **Settings → Maintenance**, or stop Routarr and copy its `config/`
> folder.

![The simulation screen: moves proposed for the library, each with its source and target folder,
the rule that matched, a confidence and a justification.](.github/assets/simulation.webp)

## Features

- **Rules**: 35 conditions (genres, keywords, language, country, certification, scores, tags,
  quality profile, runtime, studio, paths, size…),
  ALL/ANY, exclusions, priorities, manual exceptions, and an impact preview before saving.
- **Explained decisions**: for every title, what each condition expected and what it found,
  including the rules that matched and lost.
- **Safe by default**: six gates in a fixed order, the first to object stopping the rest (global
  dry-run, batch limit, reachability, capacity, a confirmation threshold you set, and a revalidation
  at the moment of writing, which sets aside a move the rules no longer justify). Every applied
  move can be reverted.
- **Metadata without API keys**: Radarr and Sonarr themselves, AniList and MyAnimeList through
  Tenrai, with TMDB, OMDb, TheTVDB and MyAnimeList's own API optional, in the order you choose.
  MyAnimeList's takes a client ID you make for yourself, as its terms ask of each installation.
- **Real time**: Radarr/Sonarr webhooks, and optional routing of new titles before they download.
- **Several instances** of Radarr 4.2 or later and Sonarr 4 or later, on Linux or Windows: a
  path on a drive letter or a network share is compared as Windows compares it, whatever its case
  or separator. Sonarr 3 works too, a new series routed at the next sync rather than before it
  downloads, and its original language read from another source than Sonarr, which reports it
  from 4.
- **Authentication on by default**: API key, a single account, or OpenID Connect, where the
  people of a group can be let in to read alone.
- **Keys for other applications**, each limited to what you allow. Every key reads, and may also
  operate (sync, simulate, apply, revert, run the rule tests, take a backup), set exceptions, or
  configure the rules, categories and folders, with the guardrails it may answer on its own.
  What they may call is a contract, served by your instance at `/api/v1/openapi.json` and listed
  on its API reference screen: from 1.0 nothing documented in it is removed, renamed or retyped,
  and until then a minor release may break it, its notes naming each break. A first call is at
  [routarr.app/api](https://routarr.app/api/).
- **Where would it go?** Ask by TMDB, TheTVDB or IMDb id, before the title is even added: a
  request bot learns the folder the rules choose, and nothing is stored.
- **Backups**, configuration export, Prometheus metrics, and notifications written for Discord,
  ntfy, Gotify or Apprise, or as JSON for any other receiver, signed as Standard Webhooks
  specifies when a receiver checks where they come from. A test message checks the address.
- **26 languages**.

## Quick start

```yaml
services:
  routarr:
    image: ghcr.io/routarr/routarr:0.2
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

### Upgrading

```bash
docker compose pull && docker compose up -d
```

A start that applies new migrations archives the database first, once per release, as
`routarr-backup-<date>-pre-<version>.zip`, kept until you delete it: no retention count prunes
it. Going back to an earlier release, by naming its version in `image:`, works while it knows
every migration the database holds. When a start refuses the database, it names an archive it
can open: restore that one with the server stopped, as below.

### Backups and restore

Routarr archives itself into `backups/` beside its database, every day unless you change the
interval, and **Settings → Maintenance** lists the archives and downloads any of them, once the
password or the API key is typed again. An archive is a consistent copy taken while Routarr runs:
the database, the `routarr.key` without which the Arr and metadata keys, the notification address,
the signing secret and the backup passphrase stored in it cannot be read (unless
`ROUTARR_SECRET_KEY` replaces it), and the API key. Keep copies of the archives off the machine:
with `ROUTARR_BACKUP_COPY_DIR` naming a folder mounted from a NAS or another disk, each archive is
copied there, and **Settings → Maintenance** marks each one copied. Copy the `config/` folder itself
only with Routarr stopped: a copy of `routarr.db` taken while it runs can miss every recent change.

An archive carries the master key: set a backup passphrase in **Settings → Maintenance** to encrypt
every one, so that a copy of the folder opens nothing without it. A restore asks for it, and
`routarr decrypt-backup <archive> <zip>` writes one opened, to read it by hand.

To restore an archive:

- **On the same host:** **Restore** beside it in **Settings → Maintenance**, then
  `docker compose restart routarr`. Until a start applies it, the card shows the restore staged
  and can cancel it. The database it replaces stays listed there until you delete it.
- **On a new host:** start Routarr once, copy the archive into `config/backups/` with its name
  unchanged and owned by uid 1000, then restore it from **Settings → Maintenance** and restart. A
  copy folder holding the lost installation's archives is left untouched until the Backups card
  is told to start the copy there.
- **With the server stopped**, when the interface cannot be reached:
  `docker compose run --rm routarr /app/routarr restore routarr-backup-<date>.zip`, then start
  Routarr. An encrypted archive asks for its passphrase when the database in place does not hold
  it. After losing the disk of `config/`, give `restore` the path of an archive in the copy
  folder instead, such as `/backups-copy/routarr-backup-<date>.zip`.

### Install notes

- **Without Compose.** The same container, hardened the same way:

  ```bash
  docker run -d --name routarr -p 9876:9876 -v "$PWD/config:/config" \
    --restart unless-stopped --stop-timeout 30 --cap-drop ALL \
    --security-opt no-new-privileges:true --read-only --tmpfs /tmp:size=64m,noexec,nosuid \
    ghcr.io/routarr/routarr:0.2
  ```

  On Unraid, add it from the Docker tab, the port and the folder in their own fields and the other
  flags under Extra Parameters. Portainer, Synology's Container Manager and TrueNAS take the
  compose file as a stack, a project or a custom app.
- **The folder's owner.** The container runs as uid 1000. On a NAS whose app folders belong to
  another user, run Routarr as that user rather than changing the folder's owner, with
  `user: "99:100"` on Unraid or `user: "1026:100"` on Synology under `routarr:` in the compose file,
  or `--user` with `docker run`. `ls -ln` shows the numbers a folder belongs to. The image reads no
  `PUID` or `PGID`, and the start says so when either is set.
- **A local disk.** Keep `config/` on a disk of the host, not on an NFS or SMB share: SQLite's
  write-ahead log does not work over a network filesystem. The archives in `config/backups/` may be
  copied anywhere.
- **The backup copy folder.** Mount it beside `config/`, for instance
  `- /mnt/nas/routarr-backups:/backups-copy` with `ROUTARR_BACKUP_COPY_DIR=/backups-copy`, writable
  by uid 1000. Its first copy leaves a `.routarr-backup-copy` file in it. Afterwards, a folder
  without that file reads as a share not mounted, its empty mount point on the local disk, and
  nothing is copied until **Start the copy here** on the Backups card says the folder changed on
  purpose.
- **64-bit only.** A Raspberry Pi needs a 64-bit system.
- **A recent browser.** The interface is made for current Chrome, Edge and Firefox, Firefox's
  extended support release included, and Safari of the last two years. An older browser may not
  show all of it.
- **Your own certificate authority.** An Arr served over HTTPS with a certificate from a homelab
  CA is reached once Routarr trusts that CA. Mount the folder holding its PEM file read-only, for
  instance `- ./certs:/certs:ro`, and set `SSL_CERT_DIR=/etc/ssl/certs:/certs`. Every folder it
  names replaces the image's own store, so `/etc/ssl/certs` comes first and keeps the metadata
  sources trusted. `update-ca-certificates` is not needed, and cannot run in the read-only
  container.
- **No media folder.** Routarr asks Radarr and Sonarr to move the files and reads none of them, so
  it mounts nothing but `config/`. Its `/config` has nothing to do with the `/data` media folder the
  TRaSH guides set up for the Arrs.

### Behind a reverse proxy

Have the proxy forward the public host and scheme (`X-Forwarded-Host`, `X-Forwarded-Proto`), and
the port in `X-Forwarded-Port` when it is not 80 or 443: the host is what a write from the
browser is checked against, the scheme what marks the session cookie `Secure`. Caddy and Traefik
forward them unasked. nginx does not, so its block sets them, and lets through the largest file
Routarr takes, the anime identifier table, where nginx refuses a body over 1 MB by default.
Without `X-Forwarded-Host`, nginx names the upstream as the host and Routarr refuses every write
from the browser:

```nginx
location / {
    proxy_pass http://routarr:9876;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Host $host;
    proxy_set_header X-Forwarded-Port $server_port;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    client_max_body_size 64m;
}
```

Under a sub-path, set `ROUTARR_BASE_PATH=/routarr` and forward the path whole, prefix included:
nginx's `location /routarr/` with no path after the address in `proxy_pass`, which would remove
the prefix, Caddy's `handle /routarr* { reverse_proxy routarr:9876 }` and not `handle_path`,
which removes it, and Traefik's ``PathPrefix(`/routarr`)`` rule without a `StripPrefix`
middleware.

List the proxy in `ROUTARR_TRUSTED_PROXIES`, so the client it forwards in `X-Forwarded-For` is
counted, not the proxy. List its own address, fixed on its Docker network (`ipv4_address`), or
put Routarr and the proxy on a network nothing else joins: every peer inside a listed range, such
as `172.18.0.0/16`, chooses the address its sign-ins count against. The session cookie reaches
every service of the same host name, whatever its port: give Routarr a host name of its own, such
as `routarr.nas.lan`, when the other services of the host are not trusted.

### Sign-in modes

In `oidc` mode the provider and the redirect URL have to be `https://`, except on `localhost`, and
`ROUTARR_OIDC_ALLOWED_SUBJECTS` or `ROUTARR_OIDC_ALLOWED_GROUPS` names who may sign in: a provider
left at its defaults would let every account of its directory in. `ROUTARR_OIDC_READ_SUBJECTS` and
`ROUTARR_OIDC_READ_GROUPS` name who may sign in to read alone what a key's read scope reaches: the
library, proposals, history, tasks, rules and their tests, categories, folders, exceptions,
sources, backup list, metrics and instances without their keys. Never a move, a setting or an
archive, and no one's name but their own. Someone the first two let in as well has full access.
A start with other lists ends every session the earlier ones opened. A group is read from the ID
token: Authelia keeps groups in userinfo unless a claims policy adds them, and Keycloak sends them
through a group membership mapper.

In `external` mode, publish the port to the proxy alone: the proxy is what signs people in.

In `forms` mode, a lost password is reset with `docker exec routarr /app/routarr reset-account`,
which prints a new one and signs everyone out. Add `--revoke-keys` to revoke every application key
and replace the API key, if there is one, as well: the running server accepts the old key until it
restarts. In another mode the command refuses, unless `--force` resets the account for a later
switch to `forms`. `docker exec routarr /app/routarr --help` lists every command.

In `none` mode Routarr answers to an address, `localhost` and the names `ROUTARR_ALLOWED_HOSTS`
lists, read from the `Host` header: a page of another site can make its own name point at
Routarr's address, and the browser would then let it act as Routarr. Reached by a name, such as
`nas.lan`, list it there. Behind a reverse proxy the name that counts is the one the proxy sends
as `Host`, `routarr` for nginx's default `proxy_pass http://routarr:9876`: list it, or have the
proxy send the public name (`proxy_set_header Host $host`, as the block above does) and list that
one.

### When something goes wrong

Routarr writes its log to the container's output: `docker logs --tail 100 routarr` prints the
end of it, as does the log button of a NAS's container screen, and `ROUTARR_LOG_LEVEL=debug`
writes more.

- **It stops at start, or restarts in a loop:** its last lines say why and what to change, a
  folder it cannot write or a database a newer release migrated, for instance.
- **Every save answers 403 behind a proxy:** the proxy does not forward the public host, see
  [Behind a reverse proxy](#behind-a-reverse-proxy).
- **The interface sends you to the server log:** the same command shows the line it means.

A bug report takes those lines, with any key or password removed.

### Stronger isolation

The container already runs as uid 1000 with every capability dropped. To put a kernel of its own
between Routarr and your host as well, run the same image under
[gVisor](https://gvisor.dev/docs/user_guide/install/), which answers every system call in user
space, on an amd64 Linux host, a NAS or a VPS included. Install it as its guide says (its
installer registers `runsc` with Docker), add `runtime: runsc` under `routarr:` in the compose
file, and run `docker compose up -d` again. `docker exec routarr uname -r` then prints gVisor's
kernel, not the host's. CI runs the image's whole start-up check under gVisor on every change to
the image, on amd64 only: the arm64 image is never run under it.

It protects the host from a compromised Routarr: reaching the host no longer takes one flaw in
your kernel. It does not protect what Routarr holds, the Arr keys and the `config/` folder, which
a compromised Routarr reads either way. Disk access is slower under gVisor, which a single SQLite
file barely feels.

### Monitoring

`/api/v1/metrics` serves Prometheus gauges: the instances, the library, the metadata sources, the
backups, the disk, and every warning the interface shows, by code and severity. A counter adds the
calls to each deprecated API path, so a script still on one shows up. Scrape it with an
application key holding the read scope, made on the Applications screen, as a bearer token:

```yaml
scrape_configs:
  - job_name: routarr
    metrics_path: /api/v1/metrics
    authorization:
      credentials: rtr_...
    static_configs:
      - targets: ["routarr:9876"]
```

[`contrib/monitoring/`](contrib/monitoring/) holds a Grafana dashboard to import and Prometheus
alert rules to load from `rule_files`, both held to the metrics by Routarr's tests.

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
