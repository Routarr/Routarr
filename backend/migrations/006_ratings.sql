-- The scores a title is given, by scale: Radarr's on the media row, each source's in the cache.
ALTER TABLE media ADD COLUMN ratings TEXT;
ALTER TABLE metadata_cache ADD COLUMN ratings TEXT NOT NULL DEFAULT '{}';
-- Asked again as each pass reaches them, within each source's own pace, for the scores they
-- were cached without.
UPDATE metadata_cache SET stale = 1 WHERE source IN ('tmdb', 'omdb', 'jikan', 'anilist');
