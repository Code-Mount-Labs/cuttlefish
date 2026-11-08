CREATE TABLE workflows (
 group_id UUID NOT NULL REFERENCES groups(group_id), id UUID NOT NULL,
 revision BIGINT NOT NULL, definition JSONB NOT NULL, PRIMARY KEY(group_id,id)
);
CREATE TABLE scheduled_routes (
 group_id UUID NOT NULL, id UUID NOT NULL, workflow_id UUID NOT NULL,
 revision BIGINT NOT NULL, definition JSONB NOT NULL, enabled BOOLEAN NOT NULL,
 next_run_ms BIGINT NOT NULL, interval_ms BIGINT NOT NULL CHECK(interval_ms>=60000),
 PRIMARY KEY(group_id,id), FOREIGN KEY(group_id,workflow_id) REFERENCES workflows(group_id,id)
);
CREATE TABLE workflow_runs (
 group_id UUID NOT NULL, id UUID NOT NULL, workflow_id UUID NOT NULL, route_id UUID,
 request JSONB NOT NULL, created_ms BIGINT NOT NULL, accepted BOOLEAN NOT NULL DEFAULT FALSE,
 error TEXT, PRIMARY KEY(group_id,id),
 FOREIGN KEY(group_id,workflow_id) REFERENCES workflows(group_id,id)
);
CREATE INDEX pending_workflow_runs ON workflow_runs(group_id,created_ms) WHERE NOT accepted;
