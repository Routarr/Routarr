-- Several metadata sources instead of one hard-wired TMDb.
--
-- Two things change. Radarr and Sonarr already return genres, original language
-- and certification in the payload the sync reads anyway, so they become a
-- source of their own — one that needs no API key and no extra request, and
-- that makes TMDb optional rather than mandatory. And the cache stops being
-- TMDb's: it is keyed by the source and by the identifier *in that source's
-- namespace*, because the next provider (AniList, OMDb, TheTVDB) will not have
-- a TMDb id to be filed under.

-- What the Arr itself knows. Stored on `media` rather than in the cache below:
-- it arrives with the sync, it expires with the sync, and giving it a row in a
-- table of *fetched* metadata would mean inventing an identifier namespace for
-- a source that never issues a request.
ALTER TABLE media ADD COLUMN genres TEXT;
ALTER TABLE media ADD COLUMN original_language TEXT;
ALTER TABLE media ADD COLUMN certification TEXT;

CREATE TABLE metadata_cache (
    -- `tmdb`, and tomorrow `anilist` or `omdb`. Not an enum: a provider is
    -- added in code, and a CHECK would turn that into a migration.
    source TEXT NOT NULL,
    -- The id in that provider's namespace, as text — TMDb numbers them, OMDb
    -- keys on `tt…`.
    external_id TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    genres TEXT NOT NULL DEFAULT '[]',
    keywords TEXT NOT NULL DEFAULT '[]',
    original_language TEXT,
    origin_countries TEXT DEFAULT '[]',
    certification TEXT,
    status TEXT,
    overview TEXT,
    poster_path TEXT,
    cached_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    PRIMARY KEY(source, external_id, media_type)
);

INSERT INTO metadata_cache (source, external_id, media_type, genres, keywords,
    original_language, origin_countries, certification, status, overview,
    poster_path, cached_at, expires_at)
SELECT 'tmdb', CAST(tmdb_id AS TEXT), media_type, genres, keywords,
    original_language, origin_countries, certification, status, overview,
    poster_path, cached_at, expires_at
FROM tmdb_cache;

DROP TABLE tmdb_cache;

CREATE INDEX idx_metadata_cache_expires ON metadata_cache(expires_at);

-- The TTL now governs every fetching source, not TMDb alone. Renamed rather
-- than duplicated so the Settings screen cannot show two knobs for one value;
-- `api/config.rs` accepts the old name when importing an older bundle.
UPDATE settings SET key = 'metadata_cache_ttl_days' WHERE key = 'tmdb_cache_ttl_days';
INSERT OR IGNORE INTO settings (key, value) VALUES ('metadata_cache_ttl_days', '7');

-- Priority order, highest first: the first source that has a value for a field
-- wins it, and the ones below fill in what it left empty. Comma-separated, like
-- `certification_regions`. The default puts the Arr first because it is always
-- present and always current, and TMDb second for what only it carries —
-- keywords and origin countries.
INSERT OR IGNORE INTO settings (key, value) VALUES ('metadata_providers', 'arr,tmdb');
