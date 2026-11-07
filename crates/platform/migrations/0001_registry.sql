CREATE TABLE accounts (
    account_id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE CHECK(length(username) BETWEEN 3 AND 64),
    password_hash TEXT NOT NULL,
    failed_logins INTEGER NOT NULL DEFAULT 0 CHECK(failed_logins >= 0),
    login_window TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY CHECK(octet_length(token_hash)=32),
    account_id UUID NOT NULL REFERENCES accounts(account_id),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX session_expiry ON sessions(expires_at);
CREATE TABLE app_groups (
    group_id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(account_id),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE invitations (
    token_hash BYTEA PRIMARY KEY CHECK(octet_length(token_hash)=32),
    group_id UUID NOT NULL REFERENCES app_groups(group_id),
    expires_at TIMESTAMPTZ NOT NULL,
    registration_hash BYTEA
);
CREATE TABLE installations (
    installation_id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES app_groups(group_id),
    role TEXT NOT NULL CHECK(role IN ('router_agent','router','agent')),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    token_hash BYTEA NOT NULL UNIQUE CHECK(octet_length(token_hash)=32),
    router_url TEXT,
    router_dashboard_url TEXT,
    agent_dashboard_url TEXT,
    ready BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen TIMESTAMPTZ,
    CHECK((role IN ('router','router_agent')) = (router_url IS NOT NULL))
);
CREATE UNIQUE INDEX one_router_per_group ON installations(group_id)
    WHERE role IN ('router','router_agent');
