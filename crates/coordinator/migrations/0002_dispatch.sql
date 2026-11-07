ALTER TABLE nodes ADD COLUMN dispatch_seen TIMESTAMPTZ;
ALTER TABLE nodes ADD COLUMN dispatch_active BIGINT NOT NULL DEFAULT 0 CHECK (dispatch_active>=0);
ALTER TABLE nodes ADD COLUMN dispatch_workspaces JSONB NOT NULL DEFAULT '[]';

CREATE TABLE thread_bindings (
    group_id UUID NOT NULL REFERENCES groups(group_id),
    thread_id UUID NOT NULL,
    task_id UUID NOT NULL,
    binding JSONB NOT NULL,
    revision BIGINT NOT NULL CHECK (revision>0),
    stopped BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(group_id,thread_id),
    UNIQUE(group_id,task_id)
);

CREATE TABLE dispatches (
    sequence BIGSERIAL PRIMARY KEY,
    group_id UUID NOT NULL,
    thread_id UUID NOT NULL,
    command_id UUID NOT NULL,
    node_id UUID NOT NULL,
    request JSONB NOT NULL,
    command JSONB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','claimed','completed')),
    accepted BOOLEAN NOT NULL DEFAULT FALSE,
    outcome JSONB,
    UNIQUE(group_id,command_id),
    FOREIGN KEY(group_id,thread_id) REFERENCES thread_bindings(group_id,thread_id),
    FOREIGN KEY(group_id,node_id) REFERENCES nodes(group_id,node_id),
    CHECK((state='completed')=(outcome IS NOT NULL))
);
CREATE UNIQUE INDEX one_pending_command ON dispatches(group_id,thread_id) WHERE state<>'completed';
CREATE INDEX node_deliveries ON dispatches(node_id,sequence) WHERE state<>'completed';
