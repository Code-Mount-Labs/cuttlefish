-- A routine owns its prompt and schedule; it does not depend on a workflow.
CREATE TABLE routines (
    group_id UUID NOT NULL REFERENCES groups(group_id),
    id UUID NOT NULL,
    revision BIGINT NOT NULL,
    definition JSONB NOT NULL,
    enabled BOOLEAN NOT NULL,
    next_run_ms BIGINT NOT NULL,
    interval_ms BIGINT NOT NULL CHECK(interval_ms >= 60000),
    PRIMARY KEY(group_id,id)
);
ALTER TABLE workflow_runs ALTER COLUMN workflow_id DROP NOT NULL;
ALTER TABLE workflow_runs ADD COLUMN routine_id UUID;
ALTER TABLE workflow_runs ADD FOREIGN KEY(group_id,routine_id) REFERENCES routines(group_id,id);

-- Keep schedule identities, next occurrences and already queued command identities.
INSERT INTO routines(group_id,id,revision,definition,enabled,next_run_ms,interval_ms)
SELECT r.group_id,r.id,r.revision,
    jsonb_build_object('id',r.id,'revision',r.revision,'name',r.definition->'name',
        'workspace_id',w.definition->'workspace_id','instructions',w.definition->'instructions',
        'enabled',r.enabled,'first_run_ms',r.definition->'first_run_ms',
        'interval_minutes',r.definition->'interval_minutes'),
    r.enabled,r.next_run_ms,r.interval_ms
FROM scheduled_routes r JOIN workflows w ON w.group_id=r.group_id AND w.id=r.workflow_id;
UPDATE workflow_runs r SET routine_id=r.route_id,workflow_id=NULL
WHERE EXISTS(SELECT 1 FROM routines n WHERE n.group_id=r.group_id AND n.id=r.route_id);
DELETE FROM scheduled_routes r WHERE EXISTS(SELECT 1 FROM routines n WHERE n.group_id=r.group_id AND n.id=r.id);
ALTER TABLE workflow_runs ADD CONSTRAINT one_run_owner CHECK(num_nonnulls(workflow_id,routine_id)=1);
CREATE INDEX routine_runs ON workflow_runs(group_id,routine_id,created_ms) WHERE routine_id IS NOT NULL;
