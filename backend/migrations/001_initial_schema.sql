-- Routarr Initial Schema
-- Instances Arr configurées
CREATE TABLE instances (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    instance_type TEXT NOT NULL CHECK(instance_type IN ('radarr', 'sonarr')),
    base_url TEXT NOT NULL,
    api_key TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    sync_interval_minutes INTEGER NOT NULL DEFAULT 15,
    last_sync_at TEXT,
    last_sync_status TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Root folders synced from the Arr instances
CREATE TABLE root_folders (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    free_space INTEGER,
    accessible INTEGER NOT NULL DEFAULT 1,
    category TEXT,
    last_synced_at TEXT,
    UNIQUE(instance_id, arr_id)
);

-- Catégories métier (user-defined)
CREATE TABLE categories (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Insert default category
INSERT INTO categories (id, name, description, is_default, display_order)
VALUES ('cat-standard', 'standard', 'Default category for unclassified media', 1, 0);

-- Synced media
CREATE TABLE media (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    title TEXT NOT NULL,
    sort_title TEXT,
    year INTEGER,
    tmdb_id INTEGER,
    tvdb_id INTEGER,
    imdb_id TEXT,
    current_path TEXT,
    current_root_folder TEXT,
    monitored INTEGER NOT NULL DEFAULT 1,
    has_files INTEGER NOT NULL DEFAULT 0,
    status TEXT,
    added_at TEXT,
    last_synced_at TEXT,
    UNIQUE(instance_id, arr_id)
);

-- Cache métadonnées TMDb
CREATE TABLE tmdb_cache (
    tmdb_id INTEGER NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    genres TEXT NOT NULL DEFAULT '[]',
    keywords TEXT NOT NULL DEFAULT '[]',
    original_language TEXT,
    origin_countries TEXT DEFAULT '[]',
    certification TEXT,
    status TEXT,
    overview TEXT,
    poster_path TEXT,
    cached_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    PRIMARY KEY(tmdb_id, media_type)
);

-- Règles de routage
CREATE TABLE rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 100,
    enabled INTEGER NOT NULL DEFAULT 1,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series', 'both')),
    conditions TEXT NOT NULL DEFAULT '[]',
    target_category TEXT NOT NULL,
    instance_ids TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Overrides manuels
CREATE TABLE overrides (
    id TEXT PRIMARY KEY,
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    target_category TEXT NOT NULL,
    reason TEXT,
    locked INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(media_id)
);

-- Decision history
CREATE TABLE decisions (
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
    status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'applied', 'failed', 'skipped')),
    error_message TEXT,
    decided_at TEXT NOT NULL DEFAULT (datetime('now')),
    applied_at TEXT
);

-- Paramètres globaux
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Default settings
INSERT INTO settings (key, value) VALUES ('default_category', 'standard');
INSERT INTO settings (key, value) VALUES ('global_dry_run', 'true');
INSERT INTO settings (key, value) VALUES ('batch_limit', '50');
INSERT INTO settings (key, value) VALUES ('tmdb_cache_ttl_days', '7');
INSERT INTO settings (key, value) VALUES ('auto_sync_enabled', 'true');

-- Logs d'exécution
CREATE TABLE execution_logs (
    id TEXT PRIMARY KEY,
    decision_id TEXT REFERENCES decisions(id),
    action TEXT NOT NULL,
    details TEXT,
    success INTEGER NOT NULL,
    error_message TEXT,
    executed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indexes for performance
CREATE INDEX idx_media_instance ON media(instance_id);
CREATE INDEX idx_media_tmdb ON media(tmdb_id);
CREATE INDEX idx_media_type ON media(media_type);
CREATE INDEX idx_decisions_media ON decisions(media_id);
CREATE INDEX idx_decisions_status ON decisions(status);
CREATE INDEX idx_decisions_decided_at ON decisions(decided_at);
CREATE INDEX idx_root_folders_instance ON root_folders(instance_id);
CREATE INDEX idx_rules_priority ON rules(priority);
CREATE INDEX idx_execution_logs_decision ON execution_logs(decision_id)
