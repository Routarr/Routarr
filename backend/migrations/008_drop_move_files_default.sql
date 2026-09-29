-- Whether to move the files on disk is asked at each apply and each revert, on
-- the screen where it is decided. A stored default nothing read would only
-- travel through every export.
DELETE FROM settings WHERE key = 'move_files_default';
