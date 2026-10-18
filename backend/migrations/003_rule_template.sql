-- The template a rule was added from, as `children@1`, so the gallery knows which
-- templates a library already holds. NULL for a rule written by hand.
ALTER TABLE rules ADD COLUMN template TEXT;
