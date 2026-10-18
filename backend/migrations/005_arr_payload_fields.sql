-- What the Arrs send about a title for the conditions on its quality profile, runtime, studio,
-- network and collection, and each instance's quality profiles, resolved to names as tags are.
ALTER TABLE media ADD COLUMN quality_profile TEXT;
ALTER TABLE media ADD COLUMN runtime INTEGER;
ALTER TABLE media ADD COLUMN studio TEXT;
ALTER TABLE media ADD COLUMN network TEXT;
ALTER TABLE media ADD COLUMN collection TEXT;
ALTER TABLE media ADD COLUMN collection_tmdb_id INTEGER;

CREATE TABLE arr_quality_profiles (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (instance_id, arr_id)
);
