-- Posters are not fetched: TMDb's answers held a path to one, which nothing
-- reads.
ALTER TABLE metadata_cache DROP COLUMN poster_path;

-- Each source's status read into the words Radarr and Sonarr use, as
-- `integrations::status` reads a new answer. A state they have no word for is
-- no status.
UPDATE metadata_cache
   SET status = CASE
           WHEN folded IN ('released', 'ended', 'finished', 'finished airing')
               THEN CASE media_type WHEN 'movie' THEN 'released' ELSE 'ended' END
           WHEN folded IN ('releasing', 'currently airing')
               THEN CASE media_type WHEN 'movie' THEN 'inCinemas' ELSE 'continuing' END
           WHEN folded IN ('returning series', 'continuing', 'hiatus') AND media_type = 'series'
               THEN 'continuing'
           WHEN folded IN ('planned', 'in production', 'post production', 'pilot',
                           'not yet released', 'not yet aired', 'upcoming')
               THEN CASE media_type WHEN 'movie' THEN 'announced' ELSE 'upcoming' END
           WHEN folded = 'rumored'
               THEN CASE media_type WHEN 'movie' THEN 'tba' ELSE 'upcoming' END
           WHEN folded IN ('canceled', 'cancelled') AND media_type = 'series'
               THEN 'ended'
       END
  FROM (SELECT rowid AS id,
               lower(replace(replace(trim(status), '_', ' '), '-', ' ')) AS folded
          FROM metadata_cache WHERE status IS NOT NULL) AS read
 WHERE metadata_cache.rowid = read.id;
