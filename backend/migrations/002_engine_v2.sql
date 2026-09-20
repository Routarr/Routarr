-- Routarr 0.2 — rule engine v2, job queue, auditability and retention.

-- Rules gain an explicit match mode and a dedicated exclusion list, so
-- "anime except kids-oriented ones" no longer needs a second inverted rule.
ALTER TABLE rules ADD COLUMN match_mode TEXT NOT NULL DEFAULT 'all';
ALTER TABLE rules ADD COLUMN exclusions TEXT NOT NULL DEFAULT '[]';

-- Decisions become auditable and revertable.
ALTER TABLE decisions ADD COLUMN confidence REAL NOT NULL DEFAULT 0;
ALTER TABLE decisions ADD COLUMN superseded INTEGER NOT NULL DEFAULT 0;
ALTER TABLE decisions ADD COLUMN simulation_id TEXT;
ALTER TABLE decisions ADD COLUMN reverted_at TEXT;

-- Execution logs carry enough context to be filtered without a join.
ALTER TABLE execution_logs ADD COLUMN instance_id TEXT;
ALTER TABLE execution_logs ADD COLUMN media_id TEXT;
ALTER TABLE execution_logs ADD COLUMN media_title TEXT;

-- Per-instance webhook secret for near-real-time routing of new additions.
ALTER TABLE instances ADD COLUMN webhook_token TEXT;

-- Background job queue, surfaced in the UI as the operations feed.
CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'success', 'failed', 'cancelled')),
    trigger TEXT NOT NULL DEFAULT 'manual',
    instance_id TEXT,
    detail TEXT,
    progress_current INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
);

CREATE INDEX idx_jobs_started_at ON jobs(started_at DESC);
CREATE INDEX idx_jobs_status ON jobs(status);
CREATE INDEX idx_decisions_media_status ON decisions(media_id, status, superseded);
CREATE INDEX idx_decisions_simulation ON decisions(simulation_id);
CREATE INDEX idx_execution_logs_executed_at ON execution_logs(executed_at DESC);
CREATE INDEX idx_tmdb_cache_expires ON tmdb_cache(expires_at);
CREATE INDEX idx_overrides_media ON overrides(media_id);
CREATE INDEX idx_root_folders_category ON root_folders(instance_id, category);

-- Retention and safety knobs, all overridable from the Settings screen.
INSERT OR IGNORE INTO settings (key, value) VALUES ('decision_retention_days', '30');
INSERT OR IGNORE INTO settings (key, value) VALUES ('log_retention_days', '90');
INSERT OR IGNORE INTO settings (key, value) VALUES ('confirmation_threshold', '10');
INSERT OR IGNORE INTO settings (key, value) VALUES ('refresh_after_move', 'true');
INSERT OR IGNORE INTO settings (key, value) VALUES ('move_files_default', 'false');
INSERT OR IGNORE INTO settings (key, value) VALUES ('scheduler_interval_minutes', '15')
