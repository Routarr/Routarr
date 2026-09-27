-- Listed without a key, TMDb answers nothing and the diagnostics warn about it
-- before anyone has configured anything. A database that routes nothing yet,
-- with no instance and no TMDb key, drops the list the initial schema seeds and
-- reads the shipped default, the Arr alone, which a TMDb key handed in by the
-- environment extends at startup. Any other list stays: rules may rely on what
-- only TMDb answers, and a list someone chose is theirs.
DELETE FROM settings
WHERE key = 'metadata_providers'
  AND value = 'arr,tmdb'
  AND NOT EXISTS (SELECT 1 FROM instances)
  AND NOT EXISTS (SELECT 1 FROM settings WHERE key = 'tmdb_api_key' AND trim(value) <> '');
