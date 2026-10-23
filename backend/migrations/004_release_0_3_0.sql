-- What 0.3.0 adds after 003, kept as one file while no release has run it.

-- What the last run of each rule test found, read by the warning and the gauges, and who
-- pinned the test.
ALTER TABLE rule_tests ADD COLUMN last_passed INTEGER;
ALTER TABLE rule_tests ADD COLUMN last_checked_at TEXT;
ALTER TABLE rule_tests ADD COLUMN subject TEXT;
ALTER TABLE rule_tests ADD COLUMN subject_key TEXT;

-- What the Arrs send about a title for the conditions on its quality profile, runtime, studio,
-- network and collection, and each instance's quality profiles, resolved to names as tags are.
ALTER TABLE media ADD COLUMN quality_profile TEXT;
ALTER TABLE media ADD COLUMN runtime INTEGER;
ALTER TABLE media ADD COLUMN studio TEXT;
ALTER TABLE media ADD COLUMN network TEXT;
ALTER TABLE media ADD COLUMN collection TEXT;
ALTER TABLE media ADD COLUMN collection_tmdb_id INTEGER;

CREATE TABLE arr_quality_profiles (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (instance_id, arr_id)
);

-- MyAnimeList is read through Tenrai, under the source id `tenrai`. The MyAnimeList ids are the
-- same, so the cached answers and the resolved titles stay.
UPDATE metadata_cache SET source = 'tenrai' WHERE source = 'jikan';
UPDATE source_identifiers SET source = 'tenrai' WHERE source = 'jikan';
UPDATE settings
   SET value = trim(replace(',' || replace(value, ' ', '') || ',', ',jikan,', ',tenrai,'), ',')
 WHERE key = 'metadata_providers';
-- A pinned test names the sources of its snapshot, which the explanation of a failure reads.
UPDATE rule_tests SET metadata_json = replace(metadata_json, '"jikan"', '"tenrai"')
 WHERE metadata_json LIKE '%"jikan"%';
-- A probe of Jikan's address says nothing about Tenrai's.
DELETE FROM probe_results WHERE subject = 'source:jikan';

-- The scores a title is given, by scale: Radarr's on the media row, each source's in the cache.
ALTER TABLE media ADD COLUMN ratings TEXT;
ALTER TABLE metadata_cache ADD COLUMN ratings TEXT NOT NULL DEFAULT '{}';
-- Asked again as each pass reaches them, within each source's own pace, for the scores they
-- were cached without.
UPDATE metadata_cache SET stale = 1 WHERE source IN ('tmdb', 'omdb', 'tenrai', 'anilist');

-- Whether automatic application may write to this instance, under the global switch. On, so an
-- upgrade changes nobody's behaviour and one switch still arms a single instance.
ALTER TABLE instances ADD COLUMN auto_apply INTEGER NOT NULL DEFAULT 1 CHECK (auto_apply IN (0, 1));
-- The last webhook delivery with a valid token and its event, and when the token was issued,
-- which a rotation sets: an instance armed for automatic application and silent an hour on is
-- warned of.
ALTER TABLE instances ADD COLUMN last_webhook_at TEXT;
ALTER TABLE instances ADD COLUMN last_webhook_event TEXT;
ALTER TABLE instances ADD COLUMN webhook_token_at TEXT;
-- The Arr's webhook connection to the instance as last read, JSON, cleared when the address or
-- the token changes.
ALTER TABLE instances ADD COLUMN webhook_check TEXT;
ALTER TABLE instances ADD COLUMN webhook_checked_at TEXT;

-- The task a move or a revert ran in, so a notification naming the task leads to every title it
-- moved.
ALTER TABLE execution_logs ADD COLUMN job_id TEXT;
CREATE INDEX idx_execution_logs_job ON execution_logs (job_id);

-- When an application key stops being accepted, null for one that never does, and when the owner
-- was told it soon would, once per key.
ALTER TABLE api_keys ADD COLUMN expires_at TEXT;
ALTER TABLE api_keys ADD COLUMN expiry_notified_at TEXT;

-- Each backup copy folder whose marker this database wrote. Named here and holding no marker, a
-- folder is a share not mounted, and nothing is copied into it. A restore writes into it before
-- this migration runs on an older restored database, hence IF NOT EXISTS.
CREATE TABLE IF NOT EXISTS backup_copy_folders (
    path TEXT PRIMARY KEY
);

-- Each archive this database copied into the copy folder, the only ones its retention count may
-- remove there: another installation's archives in a shared or inherited folder stay.
CREATE TABLE IF NOT EXISTS backup_copies (
    name TEXT PRIMARY KEY,
    copied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- What a person signed in may reach: everything, or what an application key's read scope reads.
-- Set at sign-in from the OIDC read lists and kept for the session.
ALTER TABLE sessions ADD COLUMN reach TEXT NOT NULL DEFAULT 'full' CHECK (reach IN ('full', 'read'));

-- Each deprecated API path called, by caller: how often, when last, and the day its log line was
-- last written. An application key's id names its caller, and the owner's key and every session
-- share the empty one. Kept after a key is revoked, so the counts the metrics read never fall.
CREATE TABLE deprecated_calls (
    caller_kind TEXT NOT NULL CHECK (caller_kind IN ('application', 'person')),
    caller_id TEXT NOT NULL,
    caller_name TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    calls INTEGER NOT NULL,
    last_called_at TEXT NOT NULL,
    logged_on TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (caller_kind, caller_id, method, path)
);

-- Each folder whose suggested category a person dismissed, by its path on its instance, so a sync
-- keeps it hidden. Dropped with the instance, or once no folder of the instance holds the path.
CREATE TABLE folder_suggestion_dismissals (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    PRIMARY KEY (instance_id, path)
);

-- How each identifier was found: by a search, shared by the other MyAnimeList source, or read
-- from the anime identifier table.
ALTER TABLE source_identifiers ADD COLUMN found_by TEXT NOT NULL DEFAULT 'search'
    CHECK (found_by IN ('search', 'shared', 'mapping'));

-- The anime identifier table a person downloaded or imported, one row per id of an entry: a TMDB,
-- TheTVDB or IMDb id of a film or a series, beside the AniList and MyAnimeList ids of that work.
-- A series' row carries the season the work is. Replaced whole at each load.
CREATE TABLE anime_ids (
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'series')),
    id_kind TEXT NOT NULL CHECK (id_kind IN ('tmdb', 'tvdb', 'imdb')),
    id_value TEXT NOT NULL,
    season INTEGER,
    entry_kind TEXT NOT NULL,
    anilist_id INTEGER,
    mal_id INTEGER
);
CREATE INDEX anime_ids_by_id ON anime_ids (media_type, id_kind, id_value);

-- When the table was loaded, how many entries it held, and from where: its address, or `file`.
CREATE TABLE anime_ids_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    loaded_at TEXT NOT NULL,
    entries INTEGER NOT NULL,
    origin TEXT NOT NULL
);

-- The ids a source refused although the table named them: never read from it again, so the title
-- is searched instead. Kept through loads, which would otherwise bring them back.
CREATE TABLE anime_ids_refused (
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    PRIMARY KEY (source, external_id)
);

-- The library in title order, which its screen pages through.
CREATE INDEX idx_media_title ON media (title, id);

-- One salt per installation, held as one row: a second would leave what the first sealed
-- unreadable.
CREATE TABLE secret_salt_one (id INTEGER PRIMARY KEY CHECK (id = 1), salt TEXT NOT NULL);
INSERT INTO secret_salt_one (id, salt) SELECT 1, salt FROM secret_salt ORDER BY rowid LIMIT 1;
DROP TABLE secret_salt;
ALTER TABLE secret_salt_one RENAME TO secret_salt;

-- The exceptions of titles a sync removed, kept by the title's own id (TMDB for a film, TheTVDB for
-- a series) and given back when the instance reports it again: an Arr answering part of its
-- library would otherwise delete what the operator set by hand.
CREATE TABLE retired_overrides (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'series')),
    work_id INTEGER NOT NULL,
    target_category TEXT NOT NULL,
    reason TEXT,
    created_at TEXT NOT NULL,
    subject TEXT,
    subject_key TEXT,
    retired_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (instance_id, media_type, work_id)
);

-- How many titles the last sync kept although the Arr no longer reports them: an answer that drops
-- most of a library waits for a person rather than deleting it on one read.
ALTER TABLE instances ADD COLUMN removal_held INTEGER NOT NULL DEFAULT 0;

-- What a session opened under besides its mode: the digest of the OIDC lists in force, or a
-- digest of the API key mixed with the session's own id, so a change of either ends the sessions
-- it concerns. A row written without one names neither, so every session ends here.
ALTER TABLE sessions ADD COLUMN opened_under TEXT NOT NULL DEFAULT '';
DELETE FROM sessions;
