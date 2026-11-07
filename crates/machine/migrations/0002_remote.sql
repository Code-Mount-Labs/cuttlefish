CREATE TABLE remote_access (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0),
    choice TEXT NOT NULL DEFAULT '{"mode":"local"}',
    credential TEXT,
    lease TEXT,
    state TEXT NOT NULL DEFAULT 'local',
    public_url TEXT,
    local_origin TEXT,
    proof TEXT NOT NULL,
    error_code TEXT
);
INSERT INTO remote_access(singleton,proof) VALUES(1,lower(hex(randomblob(32))));
