-- The security log, kept for the screen that reads it: the same events the
-- log lines at `routarr::audit` carry. What happened is a dictionary key and
-- its parameters, read in the reader's language.
CREATE TABLE security_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    kind TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('allowed', 'refused')),
    subject TEXT,
    client TEXT,
    message TEXT NOT NULL,
    params TEXT NOT NULL DEFAULT '{}',
    repeated INTEGER NOT NULL DEFAULT 0 CHECK (repeated >= 0)
);

CREATE INDEX idx_security_events_at ON security_events (at);
CREATE INDEX idx_security_events_kind ON security_events (kind, at);
