-- OMDb's names and TheTVDB's codes now read into every ISO language and
-- country, where a narrower table dropped some or kept them as words. Their
-- cached answers are asked again so they read the same way.
UPDATE metadata_cache SET expires_at = datetime('now') WHERE source IN ('omdb', 'tvdb');
