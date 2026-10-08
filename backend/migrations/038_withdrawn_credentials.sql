-- A credential the owner withdrew and has not replaced: the master API key or
-- the signing secrets. A restore finds none of either today both after a
-- withdrawal and on a host that never held one, and only this row tells the
-- two apart. Created only if missing: a restore copies it into an archive's
-- database ahead of this migration.
CREATE TABLE IF NOT EXISTS withdrawn_credentials (
    name TEXT PRIMARY KEY CHECK(name IN ('master_api_key', 'signing_secret')),
    withdrawn_at TEXT NOT NULL DEFAULT (datetime('now'))
);
