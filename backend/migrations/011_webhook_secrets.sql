-- The secrets the notification webhook is signed with, sealed like an Arr's
-- key. The newest signs, and the one it replaced signs beside it for a day,
-- so a receiver can be given the new one without missing a message.
CREATE TABLE webhook_secrets (
    secret TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
