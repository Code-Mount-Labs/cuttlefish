CREATE TABLE execution_activity (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT NOT NULL REFERENCES command_receipts(command_id),
    provider TEXT NOT NULL CHECK (json_valid(provider)),
    phase TEXT NOT NULL CHECK (json_valid(phase)),
    at_ms INTEGER NOT NULL,
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0,1))
) STRICT;
CREATE INDEX activity_outbox ON execution_activity(acknowledged,sequence);
CREATE INDEX activity_command ON execution_activity(command_id,sequence);
