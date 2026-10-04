-- One number that moves whenever something the routing context reads changes,
-- so an apply in slices reloads that context only when it moved
-- (`routing::Revalidation`). A table the context starts reading needs its
-- three triggers here, or a change to it goes unseen until the next apply.
CREATE TABLE routing_generation (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    value INTEGER NOT NULL
);

INSERT INTO routing_generation (id, value) VALUES (1, 0);

CREATE TRIGGER rules_insert_moves_routing AFTER INSERT ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER rules_update_moves_routing AFTER UPDATE ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER rules_delete_moves_routing AFTER DELETE ON rules
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_insert_moves_routing AFTER INSERT ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_update_moves_routing AFTER UPDATE ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER overrides_delete_moves_routing AFTER DELETE ON overrides
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_insert_moves_routing AFTER INSERT ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_update_moves_routing AFTER UPDATE ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER root_folders_delete_moves_routing AFTER DELETE ON root_folders
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_insert_moves_routing AFTER INSERT ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_update_moves_routing AFTER UPDATE ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER instances_delete_moves_routing AFTER DELETE ON instances
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_insert_moves_routing AFTER INSERT ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_update_moves_routing AFTER UPDATE ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER metadata_cache_delete_moves_routing AFTER DELETE ON metadata_cache
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_insert_moves_routing AFTER INSERT ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_update_moves_routing AFTER UPDATE ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER source_identifiers_delete_moves_routing AFTER DELETE ON source_identifiers
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

-- The two settings the context reads, and no other: a setting written on
-- every pass would reload it for nothing.
CREATE TRIGGER settings_insert_moves_routing AFTER INSERT ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_update_moves_routing AFTER UPDATE ON settings
WHEN NEW.key IN ('metadata_providers', 'default_category') OR OLD.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;

CREATE TRIGGER settings_delete_moves_routing AFTER DELETE ON settings
WHEN OLD.key IN ('metadata_providers', 'default_category')
BEGIN
    UPDATE routing_generation SET value = value + 1;
END;
