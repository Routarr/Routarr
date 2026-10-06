# Security

Routarr holds credentials for the Radarr and Sonarr instances it manages, and it
writes to them. That makes two things worth stating plainly before anything
else: how to tell us about a problem privately, and what the application already
does about the obvious ones.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private reporting instead:

> **Security** → **Report a vulnerability**, at
> <https://github.com/Routarr/Routarr/security/advisories/new>

That opens a private advisory only you and the maintainer can read.

This is a single-maintainer project, so please do not expect a same-day reply.
What you can expect: an acknowledgement that the report was received and, if
it is a real issue, a fix and a release with the advisory published alongside
it, crediting you unless you would rather not be.

Useful in a report: what an attacker needs to be able to do first (reach the
port? already have the API key? control a Radarr the instance talks to?), and
what they get out of it. A concrete request or a short reproduction is worth
more than a scanner's output.

## Supported versions

Only the newest release is supported and there is no back-porting: a fix
ships in the next release. Write a report against that version, or against
`main`.

| Version        | Supported |
| -------------- | --------- |
| newest `0.x.y` | ✅ |
| older releases | ❌ (upgrade to the newest) |

## What is already in place

Stated so a report can skip what is covered, and so a gap is easier to see.

- **Authentication is on by default.** With no `ROUTARR_API_KEY`, one is
  generated at first start into `routarr.api_key` (0600, beside the database),
  whose path the log names. Two modes ask for no credential, and each has to be asked
  for: `ROUTARR_AUTH=none` runs open, and `ROUTARR_AUTH=external` leaves the
  sign-in to a reverse proxy, so its port must reach that proxy alone.
- **The API key can be replaced or withdrawn without a restart.**
  `POST /auth/api-key` mints a new one and returns it exactly once (no route
  reads a key back), and `DELETE /auth/api-key` removes it, refusing in
  `apikey` mode where it is the only way in. Both are refused while
  `ROUTARR_API_KEY` is set: the variable wins, so a key minted here would live
  until the next restart and no further. A leaked key is therefore revoked in a
  click rather than a maintenance window.
- **An application key reaches only what its scopes grant, in every mode.**
  The owner makes one per application on the Applications screen. It reads,
  and may also operate (sync, simulate, apply, revert, run the rule tests,
  take a backup), write (exceptions) or configure (rules, categories, folder
  mappings, declared destinations, rule tests), each granted on its own. It
  reads an instance without its Arr key or its webhook address. A route no
  scope names is refused to every such key: the settings, adding or changing
  an instance, a backup archive and the keys themselves stay the owner's,
  since an archive holds the key that seals every secret. It answers only the
  guardrails it was given and moves files only if allowed, so a question it
  may not answer comes back marked for a person. Its token is shown once and stored as a SHA-256
  hash, and a revoked or unknown one is refused even in `none` and `external`,
  where a request with no key at all is let through.
- **An application key is bounded in what it can make Routarr spend.** It asks
  at most ten times a second past a burst of fifty, and is answered `429` with
  `Retry-After` beyond. A title no Arr knew is not asked about again for ten
  minutes, since each lookup reaches the Arr's own metadata service. A
  backup it takes is named apart, kept under a count of its own and taken ten
  minutes after the last at the earliest, so a loop removes none of the owner's
  archives and does not hold the schedule off. A pin's reason holds 500
  characters and an installation 1000 rules, whoever writes them.
- **What an application wrote is its own by its key, not its name.** Each task,
  proposal, move and pin records the key that asked beside its name, and a key
  reads its own name there and nobody else's: a key made under the name of a
  revoked one reads the revoked one's records as anybody's. A key cannot carry
  the names History shows for the account or the master key.
- **A notification can be signed.** With a signing secret, generated in the
  settings and shown once, every delivery carries Standard Webhooks headers:
  an HMAC-SHA256 of its id, its timestamp and its body, so a receiver can
  refuse a message nobody signed or one replayed later. The secret is sealed
  like an Arr's key and left out of a configuration export, and the one it
  replaces signs beside it for a day.
- **The generated key and the generated password stay out of the log.** The
  log names the file holding each, which only the container's user reads, and
  `docker exec routarr cat /data/routarr.api_key` prints the key: a line in
  `docker compose logs` lives as long as the logs do, and a log shipper keeps
  every line it is sent.
- **`/auth/login` bounds what it can be made to spend, and locks nobody out.**
  It is public and argon2id is deliberately expensive, in time and in memory
  (about 19 MiB per check), so the cost that makes a password hard to guess also
  makes the endpoint expensive to serve. At most two checks run at once and at
  most ten requests sit inside the endpoint. The rest are answered `503` with a
  `Retry-After` of one second. The hash runs on a
  blocking thread, never on the async runtime, so a burst of sign-ins cannot
  stall the interface, the scheduler or the webhooks. A password shorter than
  the twelve-character minimum is refused without hashing, since it cannot be
  the stored one. The permit and the queue slot are held by the *hash* rather
  than by the request: a blocking task cannot be cancelled, so a client that
  hangs up would otherwise return them while the work it started ran on, and a
  flood of abandoned connections would start one argon2 each up to the size of
  the blocking pool.

  The sustained rate is bounded per address, never per account. After five
  failed sign-ins within fifteen minutes, an address waits thirty seconds
  before its next attempt, and every failure after that doubles the wait, up to
  fifteen minutes. An attempt sent while waiting answers `429` with
  `Retry-After` and is checked against nothing, the right password included. A
  success forgets the failures, and so do fifteen quiet minutes past the last
  wait. An IPv6 client counts by its /64, the block one subscriber is given, for
  this and for its share of the queue. A lockout of the account itself would
  hand anybody who reaches the port a way to deny sign-in to the only account
  there is: the owner signing in from another address does not wait. Behind a
  reverse proxy, list it in `ROUTARR_TRUSTED_PROXIES`, an address or a range
  such as `172.18.0.0/16`, or every client shares the proxy's address and its
  wait.
- **The `forms` mode stores its single password with argon2id** and opens
  opaque server-side sessions, never signed tokens: revoking one is a delete,
  which a self-validating token cannot offer. A session is stored by the
  SHA-256 digest of its id, so a copy of the database opens none. Changing the
  password ends every session it had opened and removes `routarr.password`,
  which held the first one. The cookie is `HttpOnly`, `SameSite=Lax` and scoped to
  the mount point, and a write carrying it is refused when the request states an
  origin that is not this one.
- **In `none` and `external` modes a write with no API key is refused when the
  request states an origin that is not this one.** Nothing else stands between
  a page of another site and the API there: `none` asks for nothing, and in
  `external` the proxy in front has already signed the browser in.
- **In `none` mode a request sent to a host name Routarr does not know is
  refused.** A page of another site can make its own name resolve to Routarr's
  address (DNS rebinding), and the browser then calls that origin its own.
  Routarr answers to an address, `localhost` and the names
  `ROUTARR_ALLOWED_HOSTS` lists. In `external` mode the proxy decides which names
  reach it, so the port is bound to the proxy alone.
- **The `oidc` mode lets in the people the operator names, and nobody else.**
  A provider left at its defaults lets every account of its directory use
  every client, and a family member's account for another service would sign
  in with full access. Routarr compares the token's `sub`, which the provider
  never changes, with `ROUTARR_OIDC_ALLOWED_SUBJECTS`, and its groups with
  `ROUTARR_OIDC_ALLOWED_GROUPS`. Naming neither stops the start, unless
  `ROUTARR_OIDC_ALLOW_ANYONE=true` says that every account of the provider may
  sign in, which Diagnostics then states. A session records the person's name
  beside their `sub`, since many providers let a person change the name.
- **The `oidc` mode runs the authorization code flow with PKCE**, checks the
  issuer, the audience, the expiry and the nonce it generated. An attempt
  travels in a cookie sealed with the master key, so the public route that
  starts one writes nothing on the server and a flood of them evicts nobody's
  sign-in. The provider spends a code at its first exchange and the callback
  clears the attempt's cookie, so a code cannot be presented twice. The ID
  token's signature is deliberately not verified: it arrives in the body of a
  request this server made to the token endpoint over TLS with its client
  secret, which OpenID Connect Core §3.1.3.7 accepts in place of the signature
  for exactly this flow.
- **The API key and the webhook token are compared in constant time**
  (`subtle::ConstantTimeEq`), on both accepted header forms. A webhook token
  that does not match answers 404 rather than 401, so it does not confirm
  whether the instance exists.
- **One webhook delivery is handled at a time per instance, and at most four
  wait behind it.** That route is the only one an unauthenticated party
  reaches, and each accepted call costs a request to the Arr, a metadata
  fetch, a simulation and, with automatic application armed, a write. The
  token travels in the URL, so it is in the reverse proxy's log and in the
  Arr's own. An Arr delivers sequentially and waits for each response, so a
  season import never has two in flight. A delivery that finds the instance
  busy takes a place in its queue and waits up to twenty seconds for its
  turn, first come first served, since an Arr does not retry what it
  considers delivered. Anything past the four places is acknowledged without
  work. A flood therefore costs one worker and four waiters, and a newcomer
  never passes a delivery already waiting. It does not bound a slow sequential
  caller, which costs exactly what a real import costs: rotate the token from
  the Instances screen if one is suspected.
- **Arr API keys are sealed with AES-256-GCM** and never returned. The interface
  shows `•••• (encrypted)`. A passphrase given as `ROUTARR_SECRET_KEY` is
  stretched with Argon2id and a salt the installation keeps in its database, so
  a copy of the database or of a backup costs an Argon2id computation per
  phrase guessed, and one phrase is another key on another installation.
- **A start never replaces a master key it has lost.** With the database
  holding sealed values and `routarr.key` missing or empty, as when
  `routarr.db` is copied alone to a new volume, the start stops and names the
  file: a new key would open none of them, and every instance and source would
  fail behind a start that looked clean. `ROUTARR_ALLOW_NEW_MASTER_KEY=true`
  makes a new key anyway, and the credentials are entered again. The key files
  are written beside, flushed and renamed into place, so a power cut leaves the
  previous key or the new one, never an empty file. Only a plaintext key left by an older version gets a
  partial mask, and that is a prompt to re-save. A configuration bundle carries
  none of them, sealed or not: ciphertext is meaningless under another
  installation's master key, so the destination would store a blob it can never
  open while reporting the source as configured.
- **A backup that does not carry the master key says so before it is applied.**
  An installation whose key lives in `ROUTARR_SECRET_KEY` has no key file, so
  its archives contain none, and restoring one elsewhere leaves every sealed
  credential unreadable. The manifest records it, the restore returns it, and
  the interface warns on it rather than reporting a plain success.
- **A restore brings back no credential withdrawn since the backup.** An
  application key revoked, a signing secret replaced, the master API key
  rotated or the password changed stay as they are today, and every session
  the archive held is closed. Going back to last night undoes the damage of a
  leaked key, not its revocation. The database and master key a restore
  replaces are kept beside them, as `routarr.db.pre-restore` and
  `routarr.key.pre-restore`, until the next restore: the key copy is as secret
  as the key.
- **The application makes no outbound request nobody asked for.** No telemetry,
  no fonts from a CDN, no analytics. Every response carries a CSP whose
  `connect-src` is `'self'`.
- **Redirects are followed only within the same origin** (scheme, host *and*
  port), because the Arr credential travels in a custom `X-Api-Key` header that
  no HTTP client knows to strip on a cross-origin hop. The one exception is an
  `http` → `https` upgrade **on the same port, or the canonical 80 → 443**.
  Written as any `http` to any `https`, it would carry the key from an Arr on
  one port to whatever answers on another, on the host where every homelab
  service lives. A downgrade to cleartext is refused even when the host and the
  port are unchanged. The policy is applied when the client is built, or the
  server does not start: reqwest's defaults follow ten redirects anywhere with
  no timeout, so a fallback would silently discard both properties on the one
  path that carries a credential.
- **Nothing is sent to a link-local address** (169.254.0.0/16, fe80::/10),
  where a cloud host's metadata service hands out the machine's credentials.
  The check is made on the address a connection is about to use, so a name
  that resolves there later is refused as well as an address typed in. The
  loopback and private ranges, where Arrs live, stay reachable.
- **Transport errors are described from the error's source chain**, never from
  `reqwest`'s own `Display`, which embeds the URL and therefore any `?api_key=`
  in it.
- **The image runs as a non-root user** and carries a HEALTHCHECK.
- **The image runs under gVisor**, for a host that wants a kernel between
  Routarr and its own (see the README). On every change to the image, CI runs
  the whole smoke test under it against an amd64 build. A release runs the same
  test, without gVisor, on each architecture before publishing it.
- **Every GitHub action is pinned to a commit**, not to a movable tag.

Adversarial tests live in two files.
[`backend/src/tests/security.rs`](backend/src/tests/security.rs) holds that
authentication cannot be walked around, that user text is data and not SQL, and
that no response carries a decrypted secret.
[`backend/src/tests/webhook_fuzz.rs`](backend/src/tests/webhook_fuzz.rs) throws
about 3 000 generated JSON trees at the one route an unauthenticated party can
reach, on the invariant that no input produces a panic or a 500.

## Known limits, deliberately

Not vulnerabilities to report, but decisions, with reasons.

- **`style-src` keeps `'unsafe-inline'`.** A few elements take a size or a
  colour mix computed from data as an inline style (`Confidence`, `LibraryFacets`,
  `Jobs`, `TableSkeleton`, and the size a screen hands `Modal`), and CSP does not
  distinguish a style attribute from an injected `<style>` block. Styles cannot
  exfiltrate `localStorage`. Scripts can, and `script-src 'self'` allows none.
- **The API key is a full-access credential.** Whoever holds it can download a
  backup, which carries the master key, and can point a connection test or the
  outbound notification at any `http(s)` address the server can reach, the
  local network included, link-local addresses aside. Treat it as you would the Arr's own, and give another
  application a key of its own instead, which reaches none of that.
- **That includes replacing the key itself.** `POST /auth/api-key` sits behind
  the same middleware, so a stolen key can rotate itself. In `apikey` mode,
  where it is the only credential, that locks the operator out of the interface:
  their browser holds the value that has just stopped working. The way back in
  is to read `data/routarr.api_key`, which the rotation has already written.
  Refusing this would protect nothing, since the same key already downloads a
  backup containing the master key and with it every stored Arr credential.
- **The API key lives in the browser's `localStorage`.** That is what makes
  `script-src` the directive that actually protects it, which is why it is as
  tight as it is.
- **Routarr trusts the Arrs it is pointed at.** It reads what they return and
  acts on it. Pointing it at a hostile server is pointing it at a hostile
  server.
- **There is no user model.** One operator, and the application keys they
  issue, each limited to its scopes. It is a self-hosted tool for a household,
  not a multi-tenant service. What a session mode adds is
  a *name*: the account or the provider's subject is stored beside every
  decision and every write it causes, so "who moved this" has an answer past
  "somebody, manually". The key is one such name, `apikey`, which is how a
  script is told apart from a person, and an application key writes its
  application's name, under the trigger `api`.
