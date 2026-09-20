-- One row per sign-in attempt in flight, deleted the moment it is used.
--
-- `state` ties the browser that left to the browser that came back, and the
-- verifier is the other half of the PKCE exchange: both have to survive a
-- redirect to somebody else's server, and neither may reach the browser.
-- Stored rather than sealed into a cookie because a used row must be *gone* —
-- a replayed authorisation code is exactly what this defeats.
CREATE TABLE oidc_flows (
    state TEXT PRIMARY KEY,
    nonce TEXT NOT NULL,
    verifier TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX idx_oidc_flows_expires ON oidc_flows(expires_at);
