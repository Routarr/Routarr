-- The schema states what the code relies on: no state nothing writes, a
-- folder's origin among the two the sync knows, flags that are 0 or 1, and a
-- title's countries a list. SQLite changes a CHECK or a NOT NULL only by
-- rebuilding the table: each is copied into a new one, which takes its name,
-- and the indexes are made again. Flags are read back as 0 or 1 on the way.

CREATE TABLE jobs_new (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('running', 'success', 'failed', 'cancelled')),
    trigger TEXT NOT NULL DEFAULT 'manual',
    instance_id TEXT,
    detail TEXT,
    progress_current INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    detail_key TEXT,
    detail_params TEXT,
    subject TEXT,
    result TEXT
);
INSERT INTO jobs_new (id, kind, status, trigger, instance_id, detail, progress_current,
                      progress_total, error_message, started_at, finished_at, detail_key,
                      detail_params, subject, result)
SELECT id, kind, CASE WHEN status = 'queued' THEN 'failed' ELSE status END, trigger,
       instance_id, detail, progress_current, progress_total, error_message, started_at,
       finished_at, detail_key, detail_params, subject, result
  FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_new RENAME TO jobs;
CREATE INDEX idx_jobs_started_at ON jobs(started_at DESC);
CREATE INDEX idx_jobs_status ON jobs(status);

CREATE TABLE root_folders_new (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER,
    path TEXT NOT NULL,
    free_space INTEGER,
    accessible INTEGER NOT NULL DEFAULT 1 CHECK(accessible IN (0, 1)),
    category TEXT,
    last_synced_at TEXT,
    last_accessible_at TEXT,
    -- 'arr' for a folder the instance reports, 'declared' for one typed here.
    -- The sync's orphan cleanup reads it: without it the first pass after a
    -- declaration would delete the row and the category mapped onto it.
    origin TEXT NOT NULL DEFAULT 'arr' CHECK(origin IN ('arr', 'declared')),
    UNIQUE(instance_id, arr_id)
);
INSERT INTO root_folders_new (id, instance_id, arr_id, path, free_space, accessible, category,
                              last_synced_at, last_accessible_at, origin)
SELECT id, instance_id, arr_id, path, free_space, CASE WHEN accessible THEN 1 ELSE 0 END,
       category, last_synced_at, last_accessible_at,
       CASE WHEN origin = 'declared' THEN 'declared' ELSE 'arr' END
  FROM root_folders;
DROP TABLE root_folders;
ALTER TABLE root_folders_new RENAME TO root_folders;
CREATE INDEX idx_root_folders_category ON root_folders(instance_id, category);

CREATE TABLE metadata_cache_new (
    -- A source id from `PROVIDERS` (`tmdb`, `anilist`, `omdb`...). Not an
    -- enum: a source is added in code, and a CHECK would turn that into a
    -- migration.
    source TEXT NOT NULL,
    -- The id in that source's namespace, as text: TMDb numbers them, OMDb
    -- keys on `tt…`.
    external_id TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    genres TEXT NOT NULL DEFAULT '[]',
    keywords TEXT NOT NULL DEFAULT '[]',
    original_language TEXT,
    origin_countries TEXT NOT NULL DEFAULT '[]',
    certification TEXT,
    certification_scale TEXT,
    status TEXT,
    overview TEXT,
    poster_path TEXT,
    cached_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    PRIMARY KEY(source, external_id, media_type)
);
INSERT INTO metadata_cache_new (source, external_id, media_type, genres, keywords,
                                original_language, origin_countries, certification,
                                certification_scale, status, overview, poster_path, cached_at,
                                expires_at)
SELECT source, external_id, media_type, genres, keywords, original_language,
       COALESCE(origin_countries, '[]'), certification, certification_scale, status, overview,
       poster_path, cached_at, expires_at
  FROM metadata_cache;
DROP TABLE metadata_cache;
ALTER TABLE metadata_cache_new RENAME TO metadata_cache;
CREATE INDEX idx_metadata_cache_expires ON metadata_cache(expires_at);
CREATE INDEX idx_metadata_cache_external ON metadata_cache(external_id, media_type);

CREATE TABLE instances_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    instance_type TEXT NOT NULL CHECK(instance_type IN ('radarr', 'sonarr')),
    base_url TEXT NOT NULL,
    api_key TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    sync_interval_minutes INTEGER NOT NULL DEFAULT 15,
    last_sync_at TEXT,
    last_sync_status TEXT,
    last_sync_attempt_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    webhook_token TEXT,
    certification_country TEXT,
    arr_version TEXT,
    root_folders_read_at TEXT
);
INSERT INTO instances_new (id, name, instance_type, base_url, api_key, enabled,
                           sync_interval_minutes, last_sync_at, last_sync_status,
                           last_sync_attempt_at, created_at, updated_at, webhook_token,
                           certification_country, arr_version, root_folders_read_at)
SELECT id, name, instance_type, base_url, api_key, CASE WHEN enabled THEN 1 ELSE 0 END,
       sync_interval_minutes, last_sync_at, last_sync_status, last_sync_attempt_at, created_at,
       updated_at, webhook_token, certification_country, arr_version, root_folders_read_at
  FROM instances;
DROP TABLE instances;
ALTER TABLE instances_new RENAME TO instances;

CREATE TABLE media_new (
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
    monitored INTEGER NOT NULL DEFAULT 1 CHECK(monitored IN (0, 1)),
    has_files INTEGER NOT NULL DEFAULT 0 CHECK(has_files IN (0, 1)),
    status TEXT,
    added_at TEXT,
    last_synced_at TEXT,
    series_type TEXT,
    size_on_disk INTEGER,
    season_count INTEGER,
    tags TEXT,
    genres TEXT,
    original_language TEXT,
    certification TEXT,
    moved_at TEXT,
    UNIQUE(instance_id, arr_id)
);
INSERT INTO media_new (id, instance_id, arr_id, media_type, title, sort_title, year, tmdb_id,
                       tvdb_id, imdb_id, current_path, current_root_folder, monitored, has_files,
                       status, added_at, last_synced_at, series_type, size_on_disk, season_count,
                       tags, genres, original_language, certification, moved_at)
SELECT id, instance_id, arr_id, media_type, title, sort_title, year, tmdb_id, tvdb_id, imdb_id,
       current_path, current_root_folder, CASE WHEN monitored THEN 1 ELSE 0 END,
       CASE WHEN has_files THEN 1 ELSE 0 END, status, added_at, last_synced_at, series_type,
       size_on_disk, season_count, tags, genres, original_language, certification, moved_at
  FROM media;
DROP TABLE media;
ALTER TABLE media_new RENAME TO media;
CREATE INDEX idx_media_tmdb ON media(tmdb_id);
CREATE INDEX idx_media_tvdb ON media(tvdb_id);
CREATE INDEX idx_media_imdb ON media(imdb_id);
CREATE INDEX idx_media_type ON media(media_type);
CREATE INDEX idx_media_series_type ON media(series_type);
