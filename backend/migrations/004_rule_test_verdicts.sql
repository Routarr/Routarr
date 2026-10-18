-- What the last run of each rule test found, read by the warning and the gauges, and who
-- pinned the test.
ALTER TABLE rule_tests ADD COLUMN last_passed INTEGER;
ALTER TABLE rule_tests ADD COLUMN last_checked_at TEXT;
ALTER TABLE rule_tests ADD COLUMN subject TEXT;
ALTER TABLE rule_tests ADD COLUMN subject_key TEXT;
