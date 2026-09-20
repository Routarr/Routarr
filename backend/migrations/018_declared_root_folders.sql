-- A destination Routarr declares, beside the ones an Arr reports.
--
-- Until now a target had to already be a root folder in Radarr or Sonarr: the
-- sync was the only writer of this table and deleted anything the Arr stopped
-- returning. That forced an operator to declare in the Arr every folder they
-- wanted to route into, when the arrangement they want is the opposite — one
-- root folder per Arr, and the targets beneath it declared here.
--
-- One table rather than two: `routing::load_context` builds the category map in
-- a single query, the executor joins on it to weigh capacity, and the folders
-- screen and the conflicts read it. Two tables would put a UNION in every one
-- of those, including the hot path whose query count `scale.rs` pins.
--
-- `arr_id` becomes nullable, which SQLite cannot do in place, so the table is
-- rebuilt. NULLs are distinct in a unique index, so several declared rows per
-- instance are allowed while the synced pair stays unique.
CREATE TABLE root_folders_new (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    arr_id INTEGER,
    path TEXT NOT NULL,
    free_space INTEGER,
    accessible INTEGER NOT NULL DEFAULT 1,
    category TEXT,
    last_synced_at TEXT,
    last_accessible_at TEXT,
    -- 'arr' for a folder the instance reports, 'declared' for one typed here.
    -- The sync's orphan cleanup reads it: without it the first pass after a
    -- declaration would delete the row and the category mapped onto it.
    origin TEXT NOT NULL DEFAULT 'arr',
    UNIQUE(instance_id, arr_id)
);

INSERT INTO root_folders_new
    (id, instance_id, arr_id, path, free_space, accessible, category,
     last_synced_at, last_accessible_at, origin)
SELECT id, instance_id, arr_id, path, free_space, accessible, category,
       last_synced_at, last_accessible_at, 'arr'
FROM root_folders;

DROP TABLE root_folders;
ALTER TABLE root_folders_new RENAME TO root_folders;

-- A declared destination is addressed by its path, and two of them on one
-- instance would make the routing map ambiguous.
CREATE UNIQUE INDEX idx_root_folders_path ON root_folders(instance_id, path);
