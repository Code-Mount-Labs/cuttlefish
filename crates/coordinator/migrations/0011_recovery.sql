CREATE TABLE task_recovery (
 group_id UUID NOT NULL,
 command_id UUID NOT NULL,
 request_id UUID NOT NULL,
 view JSONB NOT NULL,
 PRIMARY KEY(group_id, command_id),
 UNIQUE(group_id, request_id),
 FOREIGN KEY(group_id,command_id) REFERENCES dispatches(group_id,command_id)
);
