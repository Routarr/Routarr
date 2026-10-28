---
paths:
  - "backend/src/tests/**"
  - "backend/src/integrations/**"
  - "backend/src/services/sync.rs"
  - "backend/src/services/executor/**"
---
# Backend test harness

- `TestApp` (`backend/src/tests/mod.rs`) drives the real `Router`, middleware included,
  against an in-memory database. `TestApp::new()` runs with auth off and
  `TestApp::with_api_key(key)` turns it on. The router holds the state it is built from, so a
  test that changes `app.state` rebuilds with `TestApp::around(state)`, the config through
  `with_config`. Fixtures and helpers (`seed_library`, `seed_route_to_anime`,
  `seed_instance_at`, `store_setting`, `save_setting`, `simulate`, `LogCapture`...) live in
  `mod.rs`, once: a second file needing one uses it there rather than copying it.
- Anything touching `backend/src/integrations/`, the sync or the executor runs against an
  in-process stand-in on an ephemeral port, started with `tests::serve` (`listen` and `serve_on`
  for one that names its own address) (`fake_arr.rs`, `fake_tmdb.rs`, `fake_sources.rs`,
  `fake_oidc.rs` in `backend/src/tests/`) and asserts on what it recorded.
  `FakeArr::failing(status)` drives the error paths. Its Radarr holds Totoro (10) and the films
  `hold_film` names (`films_to_move` does), and a move reads the film before writing it.
- `live_sources.rs` (the real sources) and `live_arrs.rs` (a real Radarr and Sonarr, weekly in
  `live-arrs.yml` only) stay `#[ignore]`d: no build waits on a third party. Each client keeps
  captured-payload tests beside it, which run in CI.
