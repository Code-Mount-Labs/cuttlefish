CREATE TABLE provider_workspaces (
 group_id UUID NOT NULL,
 workspace_id UUID NOT NULL,
 node_id UUID NOT NULL,
 workspace JSONB NOT NULL,
 PRIMARY KEY(group_id,workspace_id),
 FOREIGN KEY(group_id,node_id) REFERENCES nodes(group_id,node_id)
);
