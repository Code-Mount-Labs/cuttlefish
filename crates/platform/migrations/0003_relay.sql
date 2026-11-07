CREATE TABLE relay_grants (
    installation_id UUID PRIMARY KEY REFERENCES installations(installation_id),
    revision BIGINT NOT NULL CHECK (revision > 0),
    epoch UUID NOT NULL,
    secret_hash BYTEA,
    CHECK (secret_hash IS NULL OR octet_length(secret_hash) = 32)
);
