CREATE TABLE denial_decisions (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    tool_use_id  TEXT NOT NULL,
    tool_name    TEXT NOT NULL,
    what         TEXT NOT NULL,
    reason       TEXT,
    allow        INTEGER NOT NULL,
    decided_by   TEXT NOT NULL,
    decided_at   TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX idx_denial_decisions_call ON denial_decisions(session_id, tool_use_id);
