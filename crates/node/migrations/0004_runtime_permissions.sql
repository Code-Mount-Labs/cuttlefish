CREATE TABLE provider_bindings (
    session_id TEXT PRIMARY KEY REFERENCES local_sessions(session_id),
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    UNIQUE (binding)
);
CREATE UNIQUE INDEX provider_session_identity ON provider_bindings (
    json_extract(binding, '$.provider'), json_extract(binding, '$.provider_session_id')
);

-- These credentials belong to the human review channel, not the node transport.
CREATE TABLE permission_reviewers (
    reviewer_id TEXT PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE CHECK(length(token_hash) = 32),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
);

CREATE TABLE runtime_permissions (
    permission_id TEXT PRIMARY KEY,
    command_id TEXT NOT NULL REFERENCES command_receipts(command_id),
    record TEXT NOT NULL CHECK(json_valid(record)),
    CHECK(json_extract(record, '$.state') IN ('waiting','decided','dispatching','delivered','abandoned')),
    CHECK(json_extract(record, '$.state') = 'abandoned' OR
        (json_extract(record, '$.state') = 'waiting') = (json_extract(record, '$.review') IS NULL))
);
CREATE UNIQUE INDEX permission_decision_identity ON runtime_permissions (
    json_extract(record, '$.review.decision.decision_id')
);
CREATE UNIQUE INDEX provider_request_identity ON runtime_permissions (
    json_extract(record, '$.request.binding.target.session_id'),
    json_extract(record, '$.request.provider_request_id')
) WHERE json_extract(record, '$.state') <> 'abandoned';
CREATE UNIQUE INDEX one_pending_permission_per_command ON runtime_permissions(command_id)
    WHERE json_extract(record, '$.state') NOT IN ('delivered','abandoned');
