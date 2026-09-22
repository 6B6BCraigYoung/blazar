CREATE TABLE agents (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    runtime         TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    instructions    TEXT NOT NULL DEFAULT '',
    model           TEXT,
    permission_mode TEXT,
    custom_env      TEXT NOT NULL DEFAULT '{}',
    color           TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX uq_agents_name ON agents(name);

ALTER TABLE sessions ADD COLUMN agent_profile TEXT;
