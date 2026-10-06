-- An instance is named by its name where ids mean nothing: in a bundle, its
-- folder mappings, pins and rule scopes travel by it. Names differing only by
-- case or spaces are one name. Instances sharing one keep it for the earliest,
-- and the others take a number after it, or their id where that number is a
-- name already taken.
CREATE TEMP TABLE renamed AS
SELECT id, trim(name) || ' (' || rank || ')' AS name
  FROM (SELECT id, name,
               ROW_NUMBER() OVER (PARTITION BY lower(trim(name)) ORDER BY created_at, id) AS rank
          FROM instances)
 WHERE rank > 1;

UPDATE renamed SET name = name || ' ' || substr(id, 1, 8)
 WHERE lower(name) IN (SELECT lower(trim(name)) FROM instances)
    OR lower(name) IN (SELECT lower(other.name) FROM renamed other WHERE other.id <> renamed.id);

UPDATE instances SET name = (SELECT name FROM renamed WHERE renamed.id = instances.id)
 WHERE id IN (SELECT id FROM renamed);

DROP TABLE renamed;

CREATE UNIQUE INDEX idx_instances_name ON instances(lower(trim(name)));
