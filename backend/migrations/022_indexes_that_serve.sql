-- A title's resolutions are read and pruned by its key, which no index led
-- with: each lookup read the whole table, a row per title and per source
-- found by search.
CREATE INDEX idx_source_identifiers_local ON source_identifiers(local_key, media_type);

-- A title's history and an instance's, read from the move log newest first.
CREATE INDEX idx_execution_logs_media ON execution_logs(media_id, executed_at);
CREATE INDEX idx_execution_logs_instance ON execution_logs(instance_id, executed_at);

-- The titles a run decided, read back by the order it loaded in.
CREATE INDEX idx_media_routing_order ON media_routing(load_order);

-- Each starts another index, which answers its lookups: kept up on every
-- write for nothing.
DROP INDEX idx_media_instance;
DROP INDEX idx_decisions_media;
DROP INDEX idx_overrides_media;
DROP INDEX idx_root_folders_instance;
