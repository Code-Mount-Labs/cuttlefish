CREATE TABLE goals (
    group_id UUID NOT NULL REFERENCES groups(group_id),
    goal_id UUID NOT NULL,
    thread_id UUID NOT NULL,
    owner_node UUID NOT NULL,
    snapshot JSONB NOT NULL,
    PRIMARY KEY(group_id,goal_id),
    UNIQUE(group_id,thread_id),
    FOREIGN KEY(group_id,owner_node) REFERENCES nodes(group_id,node_id)
);
CREATE TABLE goal_tasks (
    group_id UUID NOT NULL,
    task_id UUID NOT NULL,
    goal_id UUID NOT NULL,
    thread_id UUID NOT NULL,
    command_id UUID NOT NULL,
    PRIMARY KEY(group_id,task_id),
    UNIQUE(group_id,thread_id),
    UNIQUE(group_id,command_id),
    FOREIGN KEY(group_id,goal_id) REFERENCES goals(group_id,goal_id)
);
CREATE TABLE goal_commands (
    sequence BIGSERIAL PRIMARY KEY,
    group_id UUID NOT NULL,
    goal_id UUID NOT NULL,
    command_id UUID NOT NULL,
    actor TEXT NOT NULL,
    request JSONB NOT NULL,
    response JSONB NOT NULL,
    UNIQUE(group_id,command_id),
    FOREIGN KEY(group_id,goal_id) REFERENCES goals(group_id,goal_id)
);
CREATE TABLE capability_releases (
    group_id UUID NOT NULL REFERENCES groups(group_id),
    release_id UUID NOT NULL,
    manifest JSONB NOT NULL,
    PRIMARY KEY(group_id,release_id)
);
CREATE TABLE node_capabilities (
    group_id UUID NOT NULL,
    node_id UUID NOT NULL,
    capability TEXT NOT NULL,
    release_id UUID NOT NULL,
    evidence JSONB NOT NULL,
    active BOOLEAN NOT NULL,
    PRIMARY KEY(group_id,node_id,capability),
    FOREIGN KEY(group_id,node_id) REFERENCES nodes(group_id,node_id),
    FOREIGN KEY(group_id,release_id) REFERENCES capability_releases(group_id,release_id)
);
