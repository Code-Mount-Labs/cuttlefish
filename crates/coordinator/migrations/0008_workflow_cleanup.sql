-- Persist an idempotent session-close command independently of the run result.
ALTER TABLE workflow_runs ADD COLUMN cleanup_command_id UUID NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE workflow_runs ADD COLUMN cleanup_accepted BOOLEAN NOT NULL DEFAULT FALSE;
CREATE UNIQUE INDEX workflow_cleanup_identity ON workflow_runs(cleanup_command_id);
CREATE INDEX pending_workflow_cleanup ON workflow_runs(group_id,created_ms) WHERE NOT cleanup_accepted;
