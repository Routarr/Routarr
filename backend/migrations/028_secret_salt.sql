-- The salt a passphrase given as the master key is stretched with, one per
-- installation, carried in every backup beside the values it seals.
CREATE TABLE secret_salt (salt TEXT NOT NULL);
INSERT INTO secret_salt (salt) VALUES (lower(hex(randomblob(16))));
