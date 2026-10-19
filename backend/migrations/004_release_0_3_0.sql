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
-- A probe of the address no longer asked says nothing about the new one.
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
