CREATE TABLE groups (
    group_id UUID PRIMARY KEY
);

CREATE TABLE nodes (
    node_id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(group_id),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    profile JSONB,
    active_sessions BIGINT NOT NULL DEFAULT 0 CHECK (active_sessions >= 0),
    last_seen TIMESTAMPTZ,
    UNIQUE (group_id, node_id)
);
CREATE INDEX group_nodes ON nodes(group_id, node_id);

CREATE TABLE node_events (
    cursor BIGSERIAL PRIMARY KEY,
    group_id UUID NOT NULL,
    node_id UUID NOT NULL,
    local_sequence BIGINT NOT NULL CHECK (local_sequence > 0),
    command_id UUID NOT NULL,
    payload JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (group_id, node_id) REFERENCES nodes(group_id, node_id),
    UNIQUE (group_id, node_id, local_sequence),
    UNIQUE (group_id, node_id, command_id)
);
CREATE INDEX group_events ON node_events(group_id, cursor);
