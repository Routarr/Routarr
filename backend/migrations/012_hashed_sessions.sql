-- A session is stored by the digest of its id, not the id its cookie carries,
-- so a copy of the database opens none. A row stored by its id is matched by
-- no cookie, and goes: its holder signs in once more.
DELETE FROM sessions;
