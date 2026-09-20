# Security

Routarr holds credentials for the Radarr and Sonarr instances it manages, and it
writes to them. That makes two things worth stating plainly before anything
else: how to tell us about a problem privately, and what the application already
does about the obvious ones.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private reporting instead:

> **Security** → **Report a vulnerability** —
> <https://github.com/Routarr/Routarr/security/advisories/new>

That opens a private advisory only you and the maintainer can read.

This is a single-maintainer project, so please do not expect a same-day reply.
What you can expect: an acknowledgement that the report was received, and — if
it is a real issue — a fix and a release with the advisory published alongside
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
| older releases | ❌ — upgrade to the newest |

## What is already in place

Stated so a report can skip what is covered, and so a gap is easier to see.

- **Authentication is on by default.** With no `ROUTARR_API_KEY`, one is
  generated at first start into `routarr.api_key` (0600, beside the database)
  and logged once. `ROUTARR_AUTH=none` is the only way to run open, and it
  has to be asked for.
- **The API key can be replaced or withdrawn without a restart.**
  `POST /auth/api-key` mints a new one and returns it exactly once — there is no
  route that reads a key back — and `DELETE /auth/api-key` removes it, refusing
  in `apikey` mode where it is the only way in. Both are refused while
  `ROUTARR_API_KEY` is set: the variable wins, so a key minted here would live
  until the next restart and no further. A leaked key is therefore revoked in a
  click rather than a maintenance window.
- **The generated key and the generated password are printed once**, at the
  moment they are created, beside the path of the file holding them. That is a
  deliberate trade: without it a first run needs shell access into the
  container, which on a NAS appliance is a real obstacle. The line survives in
  `docker compose logs` for as long as the logs do, so an installation that
  ships its logs off the host should regenerate the key from Settings once, and
  change the password, after the first start.
- **`/auth/login` bounds what it can be made to spend, and locks nobody out.**
  It is public and argon2id is deliberately expensive — measured at 355 ms and
  about 19 MiB per check, against 1 ms for `/ping` — so the cost that makes a
  password hard to guess also makes the endpoint expensive to serve. At most two
  checks run at once and at most ten requests sit inside the endpoint; the rest
  are answered `503` with a `Retry-After` of one second. The hash runs on a
  blocking thread, never on the async runtime, so a burst of sign-ins cannot
  stall the interface, the scheduler or the webhooks. A password shorter than
  the twelve-character minimum is refused without hashing, since it cannot be
  the stored one. The permit and the queue slot are held by the *hash* rather
  than by the request: a blocking task cannot be cancelled, so a client that
  hangs up would otherwise return them while the work it started ran on, and a
  flood of abandoned connections would start one argon2 each up to the size of
  the blocking pool.

  A lockout after N failures was tried and removed. It bounds the sustained rate
  and **not the burst** — the failures it counts are recorded after the hashes
  they were meant to prevent, so thirty simultaneous attempts all hashed before
  the door shut, measured — and it hands anybody who reaches the port a way to
  deny sign-in to the only account there is. A permit bounds the resource itself
  and refuses service to no one.
- **The `forms` mode stores its single password with argon2id** and opens
  opaque server-side sessions, never signed tokens: revoking one is a delete,
  which a self-validating token cannot offer. Changing the password ends every
  session it had opened. The cookie is `HttpOnly`, `SameSite=Lax` and scoped to
  the mount point, and a write carrying it is refused when the request states an
  origin that is not this one.
- **The `oidc` mode runs the authorization code flow with PKCE**, checks the
  issuer, the audience, the expiry and the nonce it generated, and takes each
  sign-in attempt out of the table as it is used, so an authorisation code
  cannot be presented twice. The ID token's signature is deliberately not
  verified: it arrives in the body of a request this server made to the token
  endpoint over TLS with its client secret, which OpenID Connect Core §3.1.3.7
  accepts in place of the signature for exactly this flow.
- **The API key and the webhook token are compared in constant time**
  (`subtle::ConstantTimeEq`), on both accepted header forms. A webhook token
  that does not match answers 404 rather than 401, so it does not confirm
  whether the instance exists.
- **One webhook delivery is handled at a time per instance, and at most four
  wait behind it.** That route is the only one an unauthenticated party
  reaches, and each accepted call costs a request to the Arr, a metadata
  fetch, a simulation and — with automatic application armed — a write. The
  token travels in the URL, so it is in the reverse proxy's log and in the
  Arr's own. An Arr delivers sequentially and waits for each response, so a
  season import never has two in flight; a delivery that finds the instance
  busy takes a place in its queue and waits up to twenty seconds for its
  turn, first come first served, since an Arr does not retry what it
  considers delivered, and anything past the four places is acknowledged
  without work. A flood therefore costs one worker and four waiters, and a
  newcomer never passes a delivery already waiting. It does not bound a slow sequential caller, which costs exactly what
  a real import costs — rotate the token from the Instances screen if one is
  suspected.
- **Arr API keys are sealed with AES-256-GCM** and never returned. The interface
  shows `•••• (encrypted)`; only a plaintext key left by an older version gets a
  partial mask, and that is a prompt to re-save. A configuration bundle carries
  none of them, sealed or not: ciphertext is meaningless under another
  installation's master key, so the destination would store a blob it can never
  open while reporting the source as configured.
- **A backup that does not carry the master key says so before it is applied.**
  An installation whose key lives in `ROUTARR_SECRET_KEY` has no key file, so
  its archives contain none — and restoring one elsewhere leaves every sealed
  credential unreadable. The manifest records it, the restore returns it, and
  the interface warns on it rather than reporting a plain success.
- **The application makes no outbound request nobody asked for.** No telemetry,
  no fonts from a CDN, no analytics. Every response carries a CSP whose
  `connect-src` is `'self'`.
- **Redirects are followed only within the same origin** — scheme, host *and*
  port — because the Arr credential travels in a custom `X-Api-Key` header that
  no HTTP client knows to strip on a cross-origin hop. The one exception is an
  `http` → `https` upgrade **on the same port, or the canonical 80 → 443** —
  written as any `http` to any `https`, it carried the key from an Arr on one
  port to whatever answers on another, on the host where every homelab service
  lives. A downgrade to cleartext is refused even when the host and the port are
  unchanged. The policy is applied when the client is built, or the server does
  not start: reqwest's defaults follow ten redirects anywhere with no timeout,
  so a fallback would silently discard both properties on the one path that
  carries a credential.
- **Transport errors are described from the error's source chain**, never from
  `reqwest`'s own `Display`, which embeds the URL and therefore any `?api_key=`
  in it.
- **The image runs as a non-root user** and carries a HEALTHCHECK.
- **Every GitHub action is pinned to a commit**, not to a movable tag.

Adversarial tests live in
[`backend/src/tests/security.rs`](backend/src/tests/security.rs) — authentication
cannot be walked around, user text is data and not SQL, no response carries a
decrypted secret — and
[`backend/src/tests/webhook_fuzz.rs`](backend/src/tests/webhook_fuzz.rs) throws
about 3 000 generated JSON trees at the one route an unauthenticated party can
reach, on the invariant that no input produces a panic or a 500.

## Known limits, deliberately

Not vulnerabilities to report — decisions, with reasons.

- **`style-src` keeps `'unsafe-inline'`.** Four bars draw their width from
  data (`Confidence`, `LibraryFacets`, `Jobs`, `TableSkeleton`) and CSP does not
  distinguish a style attribute from an injected `<style>` block. Styles cannot
  exfiltrate `localStorage`; scripts can, and `script-src 'self'` allows none.
- **The API key is a full-access credential.** Whoever holds it can download a
  backup, which carries the master key, and can point a connection test or the
  outbound notification at any `http(s)` address the server can reach — the
  local network included. There is no read-only key to hand out; treat the one
  key as you would the Arr's own.
- **That includes replacing the key itself.** `POST /auth/api-key` sits behind
  the same middleware, so a stolen key can rotate itself — and in `apikey` mode,
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
- **There is no user model.** One API key, one operator. It is a self-hosted
  tool for a household, not a multi-tenant service. What a session mode adds is
  a *name*: the account or the provider's subject is stored beside every
  decision and every write it causes, so "who moved this" has an answer past
  "somebody, manually". The key is one such name — `apikey` — which is how a
  script is told apart from a person.
