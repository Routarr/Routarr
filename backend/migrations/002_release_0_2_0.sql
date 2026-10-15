-- What Routarr 0.2.0 adds to the schema of 0.1.5, applied in one transaction:
-- the conversions of the data a 0.1.5 database holds run in the order the
-- tables and columns they read are created.

-- One number that moves whenever something the routing context reads changes,
-- so an apply in slices reloads that context only when it moved
-- (`routing::Revalidation`). A table the context starts reading needs its
-- three triggers here, or a change to it goes unseen until the next apply.
CREATE TABLE routing_generation (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    value INTEGER NOT NULL
);

INSERT INTO routing_generation (id, value) VALUES (1, 0);

CREATE TRIGGER rules_insert_moves_routing AFTER INSERT ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER rules_update_moves_routing AFTER UPDATE ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER rules_delete_moves_routing AFTER DELETE ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_insert_moves_routing AFTER INSERT ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_update_moves_routing AFTER UPDATE ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_delete_moves_routing AFTER DELETE ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_insert_moves_routing AFTER INSERT ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_update_moves_routing AFTER UPDATE ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_delete_moves_routing AFTER DELETE ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_insert_moves_routing AFTER INSERT ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_update_moves_routing AFTER UPDATE ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_delete_moves_routing AFTER DELETE ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_insert_moves_routing AFTER INSERT ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_update_moves_routing AFTER UPDATE ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_delete_moves_routing AFTER DELETE ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_insert_moves_routing AFTER INSERT ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_update_moves_routing AFTER UPDATE ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_delete_moves_routing AFTER DELETE ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

-- The two settings the context reads, and no other: a setting written on
-- every pass would reload it for nothing.
CREATE TRIGGER settings_insert_moves_routing AFTER INSERT ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_update_moves_routing AFTER UPDATE ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category') OR OLD.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_delete_moves_routing AFTER DELETE ON settings
WHEN OLD.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

-- The system a rating belongs to: a country code, or MAL for MyAnimeList's.
-- The certification regions rank the ratings of several sources by it, and
-- the same code means another age in another system (`R`).
ALTER TABLE metadata_cache ADD COLUMN certification_scale TEXT;

-- What every cached rating was issued under, where the source settles it:
-- OMDb rates for the United States, MyAnimeList in its own system. TMDb and
-- TheTVDB picked among the regions without saying which, so their rated
-- answers expire and the next enrichment pass asks again.
UPDATE metadata_cache SET certification_scale = 'US'
 WHERE source = 'omdb' AND certification IS NOT NULL;
UPDATE metadata_cache SET certification_scale = 'MAL'
 WHERE source = 'jikan' AND certification IS NOT NULL;
UPDATE metadata_cache SET expires_at = datetime('now')
 WHERE source IN ('tmdb', 'tvdb') AND certification IS NOT NULL;

-- The country a Radarr rates films for, read from its metadata settings at
-- each sync. A Sonarr rates for the United States.
ALTER TABLE instances ADD COLUMN certification_country TEXT;

-- The routing context reads the certification regions.
DROP TRIGGER settings_insert_moves_routing;
DROP TRIGGER settings_update_moves_routing;
DROP TRIGGER settings_delete_moves_routing;

CREATE TRIGGER settings_insert_moves_routing AFTER INSERT ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_update_moves_routing AFTER UPDATE ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category', 'certification_regions')
  OR OLD.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_delete_moves_routing AFTER DELETE ON settings
WHEN OLD.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

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

-- Routarr asks for no rescan after a move: the Arr's own move keeps the file
-- records, and a rescan running beside it deletes those of a title whose
-- files have not arrived yet.
DELETE FROM settings WHERE key = 'refresh_after_move';

-- Nothing reads a count of the changes to the routing context: an apply
-- revalidates each slice against its own items' context.
DROP TRIGGER instances_delete_moves_routing;
DROP TRIGGER instances_insert_moves_routing;
DROP TRIGGER instances_update_moves_routing;
DROP TRIGGER metadata_cache_delete_moves_routing;
DROP TRIGGER metadata_cache_insert_moves_routing;
DROP TRIGGER metadata_cache_update_moves_routing;
DROP TRIGGER overrides_delete_moves_routing;
DROP TRIGGER overrides_insert_moves_routing;
DROP TRIGGER overrides_update_moves_routing;
DROP TRIGGER root_folders_delete_moves_routing;
DROP TRIGGER root_folders_insert_moves_routing;
DROP TRIGGER root_folders_update_moves_routing;
DROP TRIGGER rules_delete_moves_routing;
DROP TRIGGER rules_insert_moves_routing;
DROP TRIGGER rules_update_moves_routing;
DROP TRIGGER settings_delete_moves_routing;
DROP TRIGGER settings_insert_moves_routing;
DROP TRIGGER settings_update_moves_routing;
DROP TRIGGER source_identifiers_delete_moves_routing;
DROP TRIGGER source_identifiers_insert_moves_routing;
DROP TRIGGER source_identifiers_update_moves_routing;
DROP TABLE routing_generation;

-- The version the Arr reported at its last sync, which the warnings hold
-- against the oldest release Routarr supports.
ALTER TABLE instances ADD COLUMN arr_version TEXT;

-- When the Arr's root folders were last listed: listing them makes the Arr
-- walk every folder inside each, so a scheduled sync lists them once a day
-- and reads the free space of the mounts in between.
ALTER TABLE instances ADD COLUMN root_folders_read_at TEXT;

-- What the last stored run decided for each title, a title it left where it
-- is included. The decisions keep only what a run proposed, so a title
-- already in its folder had no row there and read as never evaluated.
CREATE TABLE media_routing (
    media_id TEXT PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    matched_rule_id TEXT,
    -- The order in which runs loaded the library: a run that loaded earlier
    -- and commits later leaves alone a title a later one decided.
    load_order INTEGER NOT NULL,
    simulation_id TEXT,
    evaluated_at TEXT NOT NULL
);

CREATE INDEX idx_media_routing_category ON media_routing(category);

-- Until the next run, what the library showed before: each title's latest
-- standing decision.
INSERT INTO media_routing (media_id, category, matched_rule_id, load_order, simulation_id,
                           evaluated_at)
SELECT d.media_id, d.target_category, d.matched_rule_id, 0, d.simulation_id, d.decided_at
  FROM decisions d
  JOIN media m ON m.id = d.media_id
 WHERE d.superseded = 0
   AND NOT EXISTS (SELECT 1 FROM decisions later
                    WHERE later.media_id = d.media_id AND later.superseded = 0
                      AND (later.decided_at > d.decided_at
                           OR (later.decided_at = d.decided_at AND later.rowid > d.rowid)));

-- "Added within the last N days" now counts N times 24 hours and refuses 0,
-- which used to mean the last 24 hours. Every rule is written through serde,
-- so the condition has this one spelling.
UPDATE rules
   SET conditions = replace(conditions, '{"type":"added_within_days","value":0}',
                                        '{"type":"added_within_days","value":1}'),
       exclusions = replace(exclusions, '{"type":"added_within_days","value":0}',
                                        '{"type":"added_within_days","value":1}')
 WHERE instr(conditions, '{"type":"added_within_days","value":0}') > 0
    OR instr(exclusions, '{"type":"added_within_days","value":0}') > 0;

-- A title's resolutions are read and pruned by its key, which no index led
-- with: each lookup read the whole table, a row per title and per source
-- found by search.
CREATE INDEX idx_source_identifiers_local ON source_identifiers(local_key, media_type);

-- A title's history and an instance's, read from the move log newest first.
CREATE INDEX idx_execution_logs_media ON execution_logs(media_id, executed_at);
CREATE INDEX idx_execution_logs_instance ON execution_logs(instance_id, executed_at);

-- The titles a run decided, read back by the order it loaded in.
CREATE INDEX idx_media_routing_order ON media_routing(load_order);

-- Each starts another index, which answers its lookups: kept up on every
-- write for nothing.
DROP INDEX idx_media_instance;
DROP INDEX idx_decisions_media;
DROP INDEX idx_overrides_media;
DROP INDEX idx_root_folders_instance;

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

-- The last migration each release had applied when it opened the database.
-- An older release refuses the database only when a newer one migrated it
-- further than the older knows: a release with the same schema reads it as
-- its own.
ALTER TABLE _opened_by ADD COLUMN schema TEXT;

-- A title's added date in the shape every stored timestamp has, UTC, where
-- the Arr wrote it in its own. A value SQLite cannot read stays as it is.
UPDATE media
   SET added_at = COALESCE(strftime('%Y-%m-%d %H:%M:%S', added_at), added_at)
 WHERE added_at IS NOT NULL;

-- An instance is named by its name where ids mean nothing: in a bundle, its
-- folder mappings, pins and rule scopes travel by it. Names differing only by
-- case or spaces are one name. Instances sharing one keep it for the earliest,
-- and the others take a number after it, or their id where that number is a
-- name already taken.
CREATE TEMP TABLE renamed AS
SELECT id, trim(name) || ' (' || rank || ')' AS name
  FROM (SELECT id, name,
               ROW_NUMBER() OVER (PARTITION BY lower(trim(name)) ORDER BY created_at, id) AS rank
          FROM instances)
 WHERE rank > 1;

UPDATE renamed SET name = name || ' ' || substr(id, 1, 8)
 WHERE lower(name) IN (SELECT lower(trim(name)) FROM instances)
    OR lower(name) IN (SELECT lower(other.name) FROM renamed other WHERE other.id <> renamed.id);

UPDATE instances SET name = (SELECT name FROM renamed WHERE renamed.id = instances.id)
 WHERE id IN (SELECT id FROM renamed);

DROP TABLE renamed;

CREATE UNIQUE INDEX idx_instances_name ON instances(lower(trim(name)));

-- An OpenID Connect attempt travels in a cookie sealed with the master key,
-- so the public route that starts one writes nothing.
DROP TABLE IF EXISTS oidc_flows;

-- The salt a passphrase given as the master key is stretched with, one per
-- installation, carried in every backup beside the values it seals.
CREATE TABLE secret_salt (salt TEXT NOT NULL);
INSERT INTO secret_salt (salt) VALUES (lower(hex(randomblob(16))));

-- Who asked, by the application key that asked: a name can be given again
-- once its key is revoked, and the application that wrote a row is the key,
-- not whoever holds the name now. A person's rows carry none.
ALTER TABLE jobs ADD COLUMN subject_key TEXT;
ALTER TABLE decisions ADD COLUMN subject_key TEXT;
ALTER TABLE execution_logs ADD COLUMN subject_key TEXT;
ALTER TABLE overrides ADD COLUMN subject_key TEXT;

-- The rows written before by a key whose name no other key has had. A name
-- given twice cannot say which key wrote a row, and stays without one.
CREATE TEMP TABLE named_once AS
    SELECT name, MIN(id) AS id FROM api_keys GROUP BY name HAVING COUNT(*) = 1;

UPDATE jobs SET subject_key = (SELECT id FROM named_once WHERE name = jobs.subject)
 WHERE trigger IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE decisions SET subject_key = (SELECT id FROM named_once WHERE name = decisions.subject)
 WHERE actor IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE execution_logs
   SET subject_key = (SELECT id FROM named_once WHERE name = execution_logs.subject)
 WHERE actor IN ('api', 'auto') AND subject IN (SELECT name FROM named_once);
UPDATE overrides SET subject_key = (SELECT id FROM named_once WHERE name = overrides.subject)
 WHERE subject IN (SELECT name FROM named_once);

DROP TABLE named_once;

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

-- An id an Arr wrote as 0 is one it does not know: Sonarr sends a series with
-- no TMDB id as 0. Stored as an id, every such title shared one search key and
-- the one answer found for the first of them.
UPDATE media SET tmdb_id = NULL WHERE tmdb_id <= 0;
UPDATE media SET tvdb_id = NULL WHERE tvdb_id <= 0;
DELETE FROM source_identifiers WHERE local_key IN ('tmdb:0', 'tvdb:0');
DELETE FROM metadata_cache WHERE source IN ('tmdb', 'tvdb') AND external_id = '0';

-- Every country's rating TMDb and TheTVDB give, as a JSON object from country
-- code to rating. The certification regions pick one each time it is read,
-- so a change of regions holds from the next simulation.
ALTER TABLE metadata_cache ADD COLUMN certifications TEXT NOT NULL DEFAULT '{}';

-- The one rating each of their answers kept moves into the object, and the
-- answer is asked again for every country's.
UPDATE metadata_cache
   SET certifications = CASE
           WHEN certification_scale IS NOT NULL AND TRIM(certification) != ''
           THEN json_object(certification_scale, TRIM(certification))
           ELSE '{}'
       END,
       certification = NULL,
       certification_scale = NULL,
       expires_at = datetime('now')
 WHERE source IN ('tmdb', 'tvdb') AND certification IS NOT NULL;

-- The requests sent today to a source with a daily quota, counted across
-- every caller and kept through a restart, which would otherwise spend the
-- day twice. `day` is the UTC date the count belongs to.
CREATE TABLE source_requests (
    source TEXT PRIMARY KEY,
    day TEXT NOT NULL,
    spent INTEGER NOT NULL CHECK(spent >= 0)
);

-- A source with a daily quota is probed once a day. A new key or a new quota
-- deserves a probe of its own the same day, whichever path writes it.
CREATE TRIGGER settings_insert_forgets_probe AFTER INSERT ON settings
WHEN NEW.key GLOB '*_api_key' OR NEW.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(NEW.key, '_api_key', ''),
                                         '_daily_requests', '');
END;

CREATE TRIGGER settings_update_forgets_probe AFTER UPDATE ON settings
WHEN NEW.key GLOB '*_api_key' OR NEW.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(NEW.key, '_api_key', ''),
                                         '_daily_requests', '');
END;

CREATE TRIGGER settings_delete_forgets_probe AFTER DELETE ON settings
WHEN OLD.key GLOB '*_api_key' OR OLD.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(OLD.key, '_api_key', ''),
                                         '_daily_requests', '');
END;

-- OMDb's names and TheTVDB's codes now read into every ISO language and
-- country, where a narrower table dropped some or kept them as words. Their
-- cached answers are asked again so they read the same way.
UPDATE metadata_cache SET expires_at = datetime('now') WHERE source IN ('omdb', 'tvdb');

-- Posters are not fetched: TMDb's answers held a path to one, which nothing
-- reads.
ALTER TABLE metadata_cache DROP COLUMN poster_path;

-- Each source's status read into the words Radarr and Sonarr use, as
-- `integrations::status` reads a new answer. A state they have no word for is
-- no status.
UPDATE metadata_cache
   SET status = CASE
           WHEN folded IN ('released', 'ended', 'finished', 'finished airing')
               THEN CASE media_type WHEN 'movie' THEN 'released' ELSE 'ended' END
           WHEN folded IN ('releasing', 'currently airing')
               THEN CASE media_type WHEN 'movie' THEN 'inCinemas' ELSE 'continuing' END
           WHEN folded IN ('returning series', 'continuing', 'hiatus') AND media_type = 'series'
               THEN 'continuing'
           WHEN folded IN ('planned', 'in production', 'post production', 'pilot',
                           'not yet released', 'not yet aired', 'upcoming')
               THEN CASE media_type WHEN 'movie' THEN 'announced' ELSE 'upcoming' END
           WHEN folded = 'rumored'
               THEN CASE media_type WHEN 'movie' THEN 'tba' ELSE 'upcoming' END
           WHEN folded IN ('canceled', 'cancelled') AND media_type = 'series'
               THEN 'ended'
       END
  FROM (SELECT rowid AS id,
               lower(replace(replace(trim(status), '_', ' '), '-', ' ')) AS folded
          FROM metadata_cache WHERE status IS NOT NULL) AS read
 WHERE metadata_cache.rowid = read.id;

-- When the country a Radarr rates films for last changed. The films it rated
-- before keep the previous country's ratings until each is refreshed, which
-- Radarr does on its own within 180 days: the operator is told until a sync
-- they ask for after refreshing them.
ALTER TABLE instances ADD COLUMN certification_country_changed_at TEXT;

-- A cached answer is due once it is older than the lifetime set now, read at
-- each pass, or once someone asks for it again (`stale`). `cached_at` is the
-- date TMDB's six months are counted from.
ALTER TABLE metadata_cache ADD COLUMN stale INTEGER NOT NULL DEFAULT 0 CHECK(stale IN (0, 1));
UPDATE metadata_cache SET stale = 1 WHERE expires_at <= datetime('now');
DROP INDEX idx_metadata_cache_expires;
ALTER TABLE metadata_cache DROP COLUMN expires_at;
CREATE INDEX idx_metadata_cache_age ON metadata_cache(source, cached_at);

-- A credential the owner withdrew and has not replaced: the master API key or
-- the signing secrets. A restore finds none of either today both after a
-- withdrawal and on a host that never held one, and only this row tells the
-- two apart. Created only if missing: a restore copies it into an archive's
-- database ahead of this migration.
CREATE TABLE IF NOT EXISTS withdrawn_credentials (
    name TEXT PRIMARY KEY CHECK(name IN ('master_api_key', 'signing_secret')),
    withdrawn_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The code a task that failed on an error answers, as the envelope's `error`
-- would have: a caller that asked not to wait reads it here.
ALTER TABLE jobs ADD COLUMN error_code TEXT;
