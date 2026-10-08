-- A cached answer is due once it is older than the lifetime set now, read at
-- each pass, or once someone asks for it again (`stale`). `cached_at` is the
-- date TMDB's six months are counted from.
ALTER TABLE metadata_cache ADD COLUMN stale INTEGER NOT NULL DEFAULT 0 CHECK(stale IN (0, 1));
UPDATE metadata_cache SET stale = 1 WHERE expires_at <= datetime('now');
DROP INDEX idx_metadata_cache_expires;
ALTER TABLE metadata_cache DROP COLUMN expires_at;
CREATE INDEX idx_metadata_cache_age ON metadata_cache(source, cached_at);
