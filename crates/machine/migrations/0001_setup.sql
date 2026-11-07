CREATE TABLE installation (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    registration TEXT NOT NULL CHECK(json_valid(registration)),
    platform_origin TEXT NOT NULL,
    public_origin TEXT NOT NULL
);
CREATE TABLE resources (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0),
    state TEXT NOT NULL CHECK(state IN ('unconfigured','provisioning','ready','failed')),
    spec TEXT CHECK(spec IS NULL OR json_valid(spec)),
    error_code TEXT
);
INSERT INTO resources(singleton,state) VALUES(1,'unconfigured');
CREATE TABLE local_admin (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    failed_logins INTEGER NOT NULL DEFAULT 0,
    login_window INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE local_sessions (
    token_hash BLOB PRIMARY KEY,
    expires_at INTEGER NOT NULL
);
CREATE TABLE local_settings (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
