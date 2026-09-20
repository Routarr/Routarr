-- Pinned expectations for the rule engine: "these inputs must yield this
-- category".
--
-- The preview answers "what would change"; nothing answered "what must not".
-- Rules are first-match-by-priority, so inserting one rebalances everything
-- below it, and the routing that quietly moves is the one nobody was looking at.
--
-- The fixture is a *snapshot*, not a reference to a media row. `sync` deletes
-- rows the Arr stops returning and that cascades to overrides — a test pointing
-- at `media_id` would vanish with the film it was written to protect. It stores
-- the two things `EvalContext` reads, serialised.
--
-- `evaluated_at` is pinned for the same reason. `now` is an input: it is what
-- `added_within_days` compares against, so a case that borrowed the wall clock
-- would answer differently every day and eventually fail on its own. A
-- regression test asserts a fixed question.
CREATE TABLE rule_tests (
    id TEXT PRIMARY KEY,
    -- What the case claims, in the author's words.
    name TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('movie', 'series')),
    -- Serialised `Media`, and the merged metadata if any was known.
    media_json TEXT NOT NULL,
    metadata_json TEXT,
    evaluated_at TEXT NOT NULL,
    expected_category TEXT NOT NULL,
    -- Where it came from, for the "pin this decision" button. Free text and
    -- never joined on: the media row it names may be gone by the next sync.
    source_media_title TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_rule_tests_name ON rule_tests(name);
