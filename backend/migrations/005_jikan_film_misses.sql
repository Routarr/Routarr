-- A film has no broadcast season, and Jikan leaves its `year` null: the date is
-- in `aired`. A "found nothing" stored for a Jikan film was read from the wrong
-- field, so it goes, and the next pass searches the film again.
DELETE FROM source_identifiers
WHERE source = 'jikan' AND media_type = 'movie' AND external_id IS NULL;
