CREATE TABLE local_artifacts (
    attachment_id TEXT PRIMARY KEY,
    metadata TEXT NOT NULL CHECK(json_valid(metadata)),
    content BLOB NOT NULL,
    uploaded INTEGER NOT NULL DEFAULT 0 CHECK(uploaded IN(0,1))
) STRICT;
