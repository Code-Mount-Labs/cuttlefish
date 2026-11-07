CREATE TABLE execution_activity (
    group_id UUID NOT NULL,
    node_id UUID NOT NULL,
    sequence BIGINT NOT NULL CHECK(sequence>0),
    command_id UUID NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY(group_id,node_id,sequence),
    FOREIGN KEY(group_id,node_id) REFERENCES nodes(group_id,node_id),
    FOREIGN KEY(group_id,command_id) REFERENCES dispatches(group_id,command_id)
);
CREATE INDEX activity_command ON execution_activity(group_id,command_id,sequence);
