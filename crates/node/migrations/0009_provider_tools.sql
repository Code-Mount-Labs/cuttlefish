CREATE TABLE provider_tools (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 parent TEXT NOT NULL REFERENCES command_receipts(command_id),
 call_id TEXT NOT NULL,
 request TEXT NOT NULL CHECK(json_valid(request)),
 response TEXT CHECK(response IS NULL OR json_valid(response)),
 UNIQUE(parent,call_id)
) STRICT;
