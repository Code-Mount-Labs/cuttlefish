CREATE TABLE attachments (
    group_id UUID NOT NULL,
    attachment_id UUID NOT NULL,
    metadata JSONB NOT NULL,
    ready BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(group_id,attachment_id)
);
CREATE TABLE attachment_chunks (
    group_id UUID NOT NULL,
    attachment_id UUID NOT NULL,
    part INTEGER NOT NULL CHECK(part>=0 AND part<44),
    content BYTEA NOT NULL CHECK(octet_length(content)<=196608),
    PRIMARY KEY(group_id,attachment_id,part),
    FOREIGN KEY(group_id,attachment_id) REFERENCES attachments ON DELETE CASCADE
);
