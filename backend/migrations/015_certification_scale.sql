-- The system a rating belongs to: a country code, or MAL for MyAnimeList's.
-- The certification regions rank the ratings of several sources by it, and
-- the same code means another age in another system (`R`).
ALTER TABLE metadata_cache ADD COLUMN certification_scale TEXT;

-- What every cached rating was issued under, where the source settles it:
-- OMDb rates for the United States, MyAnimeList in its own system. TMDb and
-- TheTVDB picked among the regions without saying which, so their rated
-- answers expire and the next enrichment pass asks again.
UPDATE metadata_cache SET certification_scale = 'US'
 WHERE source = 'omdb' AND certification IS NOT NULL;
UPDATE metadata_cache SET certification_scale = 'MAL'
 WHERE source = 'jikan' AND certification IS NOT NULL;
UPDATE metadata_cache SET expires_at = datetime('now')
 WHERE source IN ('tmdb', 'tvdb') AND certification IS NOT NULL;

-- The country a Radarr rates films for, read from its metadata settings at
-- each sync. A Sonarr rates for the United States.
ALTER TABLE instances ADD COLUMN certification_country TEXT;

-- The routing context reads the certification regions as well now.
DROP TRIGGER settings_insert_moves_routing;
DROP TRIGGER settings_update_moves_routing;
DROP TRIGGER settings_delete_moves_routing;

CREATE TRIGGER settings_insert_moves_routing AFTER INSERT ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_update_moves_routing AFTER UPDATE ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category', 'certification_regions')
  OR OLD.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_delete_moves_routing AFTER DELETE ON settings
WHEN OLD.key IN ('metadata_providers', 'default_category', 'certification_regions')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;
