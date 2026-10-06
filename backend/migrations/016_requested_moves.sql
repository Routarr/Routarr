-- `requested`: the Arr was asked to move the title and has not been seen to
-- finish. Radarr and Sonarr move files in a queued command after answering, so
-- an apply records `requested` until it reads the title back in its target,
-- and a sync settles one the apply stopped waiting for. A revert in that state
-- carries its `reverted_at`. SQLite changes a CHECK only by rebuilding the
-- table, which the migration runner allows by turning foreign keys off.
CREATE TABLE decisions_new (
    id TEXT PRIMARY KEY,
    media_id TEXT NOT NULL,
    media_title TEXT NOT NULL,
    media_type TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    instance_name TEXT,
    current_root_folder TEXT,
    target_root_folder TEXT,
    target_category TEXT NOT NULL,
    matched_rule_id TEXT,
    matched_rule_name TEXT,
    is_override INTEGER NOT NULL DEFAULT 0,
    reasons TEXT NOT NULL DEFAULT '[]',
    alternatives TEXT NOT NULL DEFAULT '[]',
    action TEXT NOT NULL CHECK(action IN ('none', 'move', 'skip')),
    status TEXT NOT NULL CHECK(status IN ('pending', 'requested', 'applied', 'failed', 'skipped')),
    error_message TEXT,
    decided_at TEXT NOT NULL DEFAULT (datetime('now')),
    applied_at TEXT,
    confidence REAL NOT NULL DEFAULT 0,
    superseded INTEGER NOT NULL DEFAULT 0,
    simulation_id TEXT,
    reverted_at TEXT,
    actor TEXT,
    subject TEXT
);

INSERT INTO decisions_new
SELECT id, media_id, media_title, media_type, instance_id, instance_name,
       current_root_folder, target_root_folder, target_category, matched_rule_id,
       matched_rule_name, is_override, reasons, alternatives, action,
       CASE status WHEN 'approved' THEN 'pending' ELSE status END,
       error_message, decided_at, applied_at, confidence, superseded, simulation_id,
       reverted_at, actor, subject
  FROM decisions;

DROP TABLE decisions;
ALTER TABLE decisions_new RENAME TO decisions;

CREATE INDEX idx_decisions_media ON decisions(media_id);
CREATE INDEX idx_decisions_status ON decisions(status);
CREATE INDEX idx_decisions_decided_at ON decisions(decided_at);
CREATE INDEX idx_decisions_media_status ON decisions(media_id, status, superseded);
CREATE INDEX idx_decisions_simulation ON decisions(simulation_id);
