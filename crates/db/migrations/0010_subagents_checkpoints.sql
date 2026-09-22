ALTER TABLE events ADD COLUMN parent_tool_id TEXT;

CREATE TABLE checkpoints (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id   TEXT,
    seq          INTEGER,
    commit_sha   TEXT NOT NULL,
    label        TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL
) STRICT;

CREATE INDEX idx_checkpoints_ws ON checkpoints(workspace_id, created_at);
