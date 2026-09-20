-- The cache purge keeps a row a search-addressed source wrote by looking it up
-- in `source_identifiers` on the source's own identifier; the primary key leads
-- on `local_key`, so without this the lookup is a scan per cache row.
CREATE INDEX IF NOT EXISTS idx_source_identifiers_external
    ON source_identifiers(source, media_type, external_id);

-- 018 rebuilt `root_folders` through a copy, and a copied table carries none of
-- the indexes the original had: the two from 001 and 002 went with it, and the
-- routing context and the executor both join on `instance_id`.
CREATE INDEX IF NOT EXISTS idx_root_folders_instance ON root_folders(instance_id);
CREATE INDEX IF NOT EXISTS idx_root_folders_category ON root_folders(instance_id, category);
