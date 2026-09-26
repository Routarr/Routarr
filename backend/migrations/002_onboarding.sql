-- The getting-started guide is for an installation that is not set up yet. One that
-- already has an instance is set up, so it starts with the guide done. A fresh
-- database gets no row, which reads as "pending".
INSERT INTO settings (key, value)
SELECT 'onboarding', 'done' WHERE EXISTS (SELECT 1 FROM instances)
ON CONFLICT(key) DO NOTHING;
