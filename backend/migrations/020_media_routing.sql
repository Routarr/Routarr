-- What the last stored run decided for each title, a title it left where it
-- is included. The decisions keep only what a run proposed, so a title
-- already in its folder had no row there and read as never evaluated.
CREATE TABLE media_routing (
    media_id TEXT PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    matched_rule_id TEXT,
    -- The order in which runs loaded the library: a run that loaded earlier
    -- and commits later leaves alone a title a later one decided.
    load_order INTEGER NOT NULL,
    simulation_id TEXT,
    evaluated_at TEXT NOT NULL
);

CREATE INDEX idx_media_routing_category ON media_routing(category);

-- Until the next run, what the library showed before: each title's latest
-- standing decision.
INSERT INTO media_routing (media_id, category, matched_rule_id, load_order, simulation_id,
                           evaluated_at)
SELECT d.media_id, d.target_category, d.matched_rule_id, 0, d.simulation_id, d.decided_at
  FROM decisions d
  JOIN media m ON m.id = d.media_id
 WHERE d.superseded = 0
   AND NOT EXISTS (SELECT 1 FROM decisions later
                    WHERE later.media_id = d.media_id AND later.superseded = 0
                      AND (later.decided_at > d.decided_at
                           OR (later.decided_at = d.decided_at AND later.rowid > d.rowid)));
