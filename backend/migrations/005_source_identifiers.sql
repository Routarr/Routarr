-- Identifiers for the sources that have none of ours.
--
-- TMDb, OMDb and TheTVDB are addressed by an id the library already carries
-- (`tmdb_id`, `imdb_id`, `tvdb_id`). AniList and Jikan carry none of them: the
-- only way in is a search on the title and the year, which is a network call
-- with a fuzzy answer — exactly what must not be repeated on every pass.
--
-- `external_id` is nullable **on purpose**: a resolution that found nothing is
-- an answer, and storing it is what stops Routarr from searching AniList for
-- every live-action film in the library, for ever, once per run.
CREATE TABLE source_identifiers (
    source TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    -- The library's own identity for the item, namespaced: `tmdb:8392`,
    -- `tvdb:76885`, `imdb:tt0096283`, or a normalised title and year when the
    -- item has no external id at all. Shared by every instance holding the same
    -- item, so a film present in two Radarrs is resolved once.
    local_key TEXT NOT NULL,
    -- NULL means "searched, found nothing".
    external_id TEXT,
    resolved_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(source, media_type, local_key)
);

CREATE INDEX idx_source_identifiers_resolved ON source_identifiers(resolved_at);
