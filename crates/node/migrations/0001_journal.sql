-- One database per computer, on a local filesystem. Never share it over NFS/SMB.
CREATE TABLE node_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity TEXT NOT NULL CHECK (json_valid(identity))
) STRICT;

CREATE TABLE workspace_mounts (
    workspace_id TEXT PRIMARY KEY,
    path TEXT NOT NULL
) STRICT;

CREATE TABLE local_sessions (
    session_id TEXT PRIMARY KEY,
    target TEXT NOT NULL CHECK (json_valid(target)),
    workspace_id TEXT NOT NULL REFERENCES workspace_mounts(workspace_id),
    instance_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('starting','running','stopped','interrupted')),
    pid INTEGER CHECK (pid > 0),
    heartbeat_at INTEGER NOT NULL,
    stop_requested INTEGER NOT NULL DEFAULT 0 CHECK (stop_requested IN (0,1)),
    last_error TEXT
) STRICT;

CREATE TABLE command_receipts (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL REFERENCES local_sessions(session_id),
    request TEXT NOT NULL CHECK (json_valid(request)),
    status TEXT NOT NULL CHECK (status IN ('queued','executing','completed','rejected','uncertain')),
    outcome TEXT CHECK (outcome IS NULL OR json_valid(outcome)),
    CHECK ((status IN ('completed','rejected')) = (outcome IS NOT NULL))
) STRICT;

CREATE INDEX claim_commands ON command_receipts(session_id, status, sequence);

CREATE TABLE pending_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT NOT NULL UNIQUE REFERENCES command_receipts(command_id),
    outcome TEXT NOT NULL CHECK (json_valid(outcome)),
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0,1))
) STRICT;

CREATE INDEX unacknowledged_events ON pending_events(acknowledged, sequence);
