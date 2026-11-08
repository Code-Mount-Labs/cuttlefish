CREATE TABLE provider_quiescence (
 command_id TEXT PRIMARY KEY REFERENCES command_receipts(command_id),
 stopped INTEGER NOT NULL CHECK(stopped IN (0,1))
) STRICT;
CREATE TABLE task_recovery (
 request_id TEXT PRIMARY KEY,
 command_id TEXT NOT NULL REFERENCES command_receipts(command_id),
 view TEXT NOT NULL CHECK(json_valid(view)),
 instance TEXT
) STRICT;
