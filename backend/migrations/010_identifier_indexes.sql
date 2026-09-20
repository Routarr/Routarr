-- Every join between `media` and `metadata_cache` is an equality on one
-- identifier namespace: `tmdb_id`, `tvdb_id` or `imdb_id`. Two of the three
-- had no index, and the cache had none led by `external_id`, so each lookup
-- was a scan of the other table.
CREATE INDEX idx_media_imdb ON media(imdb_id);
CREATE INDEX idx_media_tvdb ON media(tvdb_id);
CREATE INDEX idx_metadata_cache_external ON metadata_cache(external_id, media_type);
