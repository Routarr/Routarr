-- An OpenID Connect attempt travels in a cookie sealed with the master key,
-- so the public route that starts one writes nothing.
DROP TABLE IF EXISTS oidc_flows;
