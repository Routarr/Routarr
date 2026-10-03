-- Every release that opens the database, so an older one that finds a newer
-- recorded can stop before it reads a schema it does not know.
CREATE TABLE _opened_by (version TEXT PRIMARY KEY);
