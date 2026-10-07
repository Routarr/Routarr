-- The requests sent today to a source with a daily quota, counted across
-- every caller and kept through a restart, which would otherwise spend the
-- day twice. `day` is the UTC date the count belongs to.
CREATE TABLE source_requests (
    source TEXT PRIMARY KEY,
    day TEXT NOT NULL,
    spent INTEGER NOT NULL CHECK(spent >= 0)
);

-- A source with a daily quota is probed once a day. A new key or a new quota
-- deserves a probe of its own the same day, whichever path writes it.
CREATE TRIGGER settings_insert_forgets_probe AFTER INSERT ON settings
WHEN NEW.key GLOB '*_api_key' OR NEW.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(NEW.key, '_api_key', ''),
                                         '_daily_requests', '');
END;

CREATE TRIGGER settings_update_forgets_probe AFTER UPDATE ON settings
WHEN NEW.key GLOB '*_api_key' OR NEW.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(NEW.key, '_api_key', ''),
                                         '_daily_requests', '');
END;

CREATE TRIGGER settings_delete_forgets_probe AFTER DELETE ON settings
WHEN OLD.key GLOB '*_api_key' OR OLD.key = 'omdb_daily_requests'
BEGIN
    DELETE FROM probe_results
     WHERE subject = 'source:' || replace(replace(OLD.key, '_api_key', ''),
                                         '_daily_requests', '');
END;
