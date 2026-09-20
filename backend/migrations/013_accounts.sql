-- The single account of the `forms` mode, and the sessions it opens.
--
-- One row in `users`, like the Servarr applications: their UserService reads
-- `SingleOrDefault()` and their User model carries no role. Access here is all
-- or nothing too, so there is no role column to keep in step with anything.
--
-- Session ids are opaque random strings, never signed tokens: revoking one is
-- a DELETE, which a stateless token cannot offer.
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    -- Who the session belongs to: a username today, an OIDC subject later.
    subject TEXT NOT NULL,
    -- Which mode opened it, so a mode change does not leave live sessions it
    -- would never have granted.
    source TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    last_used_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sessions_expires ON sessions(expires_at);
