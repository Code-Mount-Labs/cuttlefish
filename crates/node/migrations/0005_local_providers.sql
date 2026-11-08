CREATE TABLE workspace_providers (
 workspace_id TEXT PRIMARY KEY REFERENCES workspace_mounts(workspace_id),
 configuration TEXT NOT NULL CHECK(json_valid(configuration))
) STRICT;
