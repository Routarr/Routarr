-- Signals Radarr and Sonarr already expose and that Routarr ignored.
--
-- No extra external call: everything arrives in the response the sync reads
-- anyway. They close the most frequently hit gap — telling Japanese animation
-- apart without depending on TMDb — and they enable disk tiering, which is
-- common in a homelab where every root folder is a different volume.

-- `standard` | `anime` | `daily`: Sonarr's own marker, more reliable for this
-- case than any heuristic over TMDb genres. NULL for movies, which Radarr does
-- not classify that way.
ALTER TABLE media ADD COLUMN series_type TEXT;

-- Bytes. Radarr exposes it directly, Sonarr through its statistics.
ALTER TABLE media ADD COLUMN size_on_disk INTEGER;

-- Series only. Excludes season 0 (the extras), which nobody counts when they
-- write "more than five seasons".
ALTER TABLE media ADD COLUMN season_count INTEGER;

-- The tags the user set in their own Arr: the one signal no external metadata
-- can replace, because it expresses their intent. Stored denormalised as JSON
-- rather than in a join table: they are read in bulk on every rule evaluation
-- and are never queried on their own.
ALTER TABLE media ADD COLUMN tags TEXT;

-- Identifier-to-label mapping, per instance. An Arr only exposes numeric ids on
-- a media item; without this table a rule would have to target "tag 7", which
-- means nothing to a human.
CREATE TABLE arr_tags (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER NOT NULL,
    label TEXT NOT NULL,
    PRIMARY KEY (instance_id, arr_id)
);

CREATE INDEX idx_media_series_type ON media(series_type);
