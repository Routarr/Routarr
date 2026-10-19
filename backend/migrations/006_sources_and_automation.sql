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
