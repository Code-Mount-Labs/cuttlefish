CREATE TABLE super_agent_outbox (
    goal_id TEXT PRIMARY KEY,
    command TEXT NOT NULL CHECK(json_valid(command))
) STRICT;
CREATE TABLE planning_budget (
    goal_id TEXT PRIMARY KEY,
    turns INTEGER NOT NULL CHECK(turns>0)
) STRICT;
