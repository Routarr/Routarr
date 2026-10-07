-- An id an Arr wrote as 0 is one it does not know: Sonarr sends a series with
-- no TMDB id as 0. Stored as an id, every such title shared one search key and
-- the one answer found for the first of them.
UPDATE media SET tmdb_id = NULL WHERE tmdb_id <= 0;
UPDATE media SET tvdb_id = NULL WHERE tvdb_id <= 0;
DELETE FROM source_identifiers WHERE local_key IN ('tmdb:0', 'tvdb:0');
DELETE FROM metadata_cache WHERE source IN ('tmdb', 'tvdb') AND external_id = '0';
