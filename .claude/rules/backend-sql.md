---
paths:
  - "backend/migrations/**"
  - "backend/src/*.rs"
  - "backend/src/{api,jobs,models,services,tests}/**"
  - "scripts/check-migrations.py"
---
# Backend data and SQL

- A shipped migration never changes: `_migrations` records names, and
  `scripts/check-migrations.py` refuses the edit. Until the next release, a schema change extends
  the latest unreleased file in `MIGRATIONS` (`backend/src/db.rs`) when the development database
  (`backend/data/`) has not run it, and goes in a new file otherwise. Just before the release's
  first tag, a pre-release's too, the files added since the last release fold into one, keeping
  each statement's net effect: nothing it creates is dropped again later in that file. An
  upgrade therefore starts from a released schema, 0.1.5 at the oldest.
- A migration may rebuild a table to change a CHECK or a type: `apply_migrations` runs it with
  foreign keys off and refuses a commit `foreign_key_check` faults. Recreate its indexes there.
- Timestamps are TEXT in `%Y-%m-%d %H:%M:%S` UTC, the shape `datetime('now')` writes, so both
  compare as strings. Booleans are INTEGER.
- A tuple `query_as` target binds by position and stops at 16 fields. Use a named `FromRow`
  struct for a wide row or one likely to grow.
- Write `AssertSqlSafe` at each call site, never inside a helper, so `grep AssertSqlSafe` lists
  every dynamic statement. Splice only consts and literal fragments and bind every value:
  `db::placeholders` for an `IN` list, chunked by `routing::BIND_CHUNK`, and `db::escape_like`
  for user text in a `LIKE ? ESCAPE '\'`. A `&'static str` parameter needs no marker
  (`maintenance::delete_older_than`).
- `backend/src/tests/scale.rs` counts the statements a simulation runs at two library sizes. A
  query per item fails there and nowhere else, since every other test seeds one item.
- A new `media` column goes in a migration, the `Media` struct, `MEDIA_COLUMNS` in
  `backend/src/services/routing.rs` (reads) and `upsert_media` in
  `backend/src/services/sync.rs` (writes). `FromRow` maps by name, so a column missing from
  either fails at run time on that one path.
- A column holding a category name is called `category` or `*_category`. A new one needs its
  `UPDATE` in the statement list of `api::categories::rename` and a row in
  `seed_every_reference` (`backend/src/tests/categories.rs`), whose test finds the columns by
  that name and fails until both are there. A column named otherwise escapes both.
- A check and the write it guards share one `db::write_transaction` (`BEGIN IMMEDIATE`): under a
  plain `BEGIN` another writer slips in between, as a category removed while a rule naming it
  is saved, and a transaction reading before it writes fails at once with "database is locked"
  behind another writer, whatever the busy timeout. Read the localizer before opening it, since
  a pool of one connection (the tests') cannot serve that read meanwhile. `race::checked` marks
  the point a test holds a writer at (`backend/src/tests/races.rs`).
- Declared and synced folders share `root_folders`, told apart by `origin`: a second table
  would put a `UNION` in `routing::load_context` and in every executor join.
- A `root_folders` row with `origin = 'declared'` belongs to the operator. A cleanup of folders
  an Arr stopped reporting stays scoped to `origin = 'arr'`, or it deletes the declared row and
  the category mapped onto it. A declared path an Arr adopts is promoted in place (`do_sync`),
  keeping one row per path.
- An apply stamps `media.moved_at`. Code writing the path columns from an Arr read takes its
  `read_at` before the first request and goes through `upsert_media`, which keeps a newer move.
  The `apply` and `sync:{instance}` job locks are distinct, so nothing else closes that race.
- A rule test stores a snapshot (`Media`, merged metadata, `now`), never a `media_id`: a sync
  deletes the rows an Arr stops returning, and a case tied to one would go with it.
- The default `ROUTARR_DB_PATH` is relative: `cargo run` in `backend/` opens `backend/data/`,
  another library than the release binary run from the repository root. A test relocates a
  config with `Config::set_db_path`, never by assigning `db_path` alone.
