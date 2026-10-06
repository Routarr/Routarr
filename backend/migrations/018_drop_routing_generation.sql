-- Nothing reads a count of the changes to the routing context: an apply
-- revalidates each slice against its own items' context.
DROP TRIGGER instances_delete_moves_routing;
DROP TRIGGER instances_insert_moves_routing;
DROP TRIGGER instances_update_moves_routing;
DROP TRIGGER metadata_cache_delete_moves_routing;
DROP TRIGGER metadata_cache_insert_moves_routing;
DROP TRIGGER metadata_cache_update_moves_routing;
DROP TRIGGER overrides_delete_moves_routing;
DROP TRIGGER overrides_insert_moves_routing;
DROP TRIGGER overrides_update_moves_routing;
DROP TRIGGER root_folders_delete_moves_routing;
DROP TRIGGER root_folders_insert_moves_routing;
DROP TRIGGER root_folders_update_moves_routing;
DROP TRIGGER rules_delete_moves_routing;
DROP TRIGGER rules_insert_moves_routing;
DROP TRIGGER rules_update_moves_routing;
DROP TRIGGER settings_delete_moves_routing;
DROP TRIGGER settings_insert_moves_routing;
DROP TRIGGER settings_update_moves_routing;
DROP TRIGGER source_identifiers_delete_moves_routing;
DROP TRIGGER source_identifiers_insert_moves_routing;
DROP TRIGGER source_identifiers_update_moves_routing;
DROP TABLE routing_generation;
