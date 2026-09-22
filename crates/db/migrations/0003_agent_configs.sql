CREATE TABLE agent_configs (
    id              TEXT PRIMARY KEY,
    node_id         TEXT REFERENCES nodes(id) ON DELETE CASCADE,
    agent_id        TEXT NOT NULL,
    display_name    TEXT,
    program_path    TEXT,
    model           TEXT,
    permission_mode TEXT,
    custom_args     TEXT NOT NULL DEFAULT '[]',
    custom_env      TEXT NOT NULL DEFAULT '{}',
    max_concurrent  INTEGER NOT NULL DEFAULT 2,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (node_id, agent_id)
) STRICT;

CREATE INDEX idx_agent_configs_node ON agent_configs(node_id);
