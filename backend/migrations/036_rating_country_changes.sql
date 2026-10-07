-- When the country a Radarr rates films for last changed. The films it rated
-- before keep the previous country's ratings until each is refreshed, which
-- Radarr does on its own within 180 days: the operator is told until a sync
-- they ask for after refreshing them.
ALTER TABLE instances ADD COLUMN certification_country_changed_at TEXT;
