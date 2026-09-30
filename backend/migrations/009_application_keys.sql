-- A key the operator gives another application, held to its own scopes.
-- Only a hash of the secret is kept: the token is shown once, when it is made.
-- A revoked row stays, so the name it wrote on jobs, decisions and overrides
-- still says which application that was.
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    -- A JSON array of the scopes beyond read, which every key holds.
    scopes TEXT NOT NULL DEFAULT '[]',
    -- A JSON array of the guardrails this key may answer on its own.
    may_confirm TEXT NOT NULL DEFAULT '[]',
    may_move_files INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_by TEXT,
    last_used_at TEXT,
    revoked_at TEXT
);

-- Two live keys under one name would make every attribution ambiguous.
CREATE UNIQUE INDEX idx_api_keys_live_name ON api_keys(name) WHERE revoked_at IS NULL;

-- Who started a task and who pinned a title, beside the trigger: an
-- application's name, or the person a sign-in mode names.
ALTER TABLE jobs ADD COLUMN subject TEXT;
ALTER TABLE overrides ADD COLUMN subject TEXT;
