CREATE TABLE dispatch_receipts (
    command_id TEXT PRIMARY KEY REFERENCES command_receipts(command_id)
) STRICT;
