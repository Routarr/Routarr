-- Routarr asks for no rescan after a move: the Arr's own move keeps the file
-- records, and a rescan running beside it deletes those of a title whose
-- files have not arrived yet.
DELETE FROM settings WHERE key = 'refresh_after_move';
