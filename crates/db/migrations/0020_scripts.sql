CREATE TABLE workspace_scripts (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    setup        TEXT NOT NULL DEFAULT '',
    cleanup      TEXT NOT NULL DEFAULT '',
    dev          TEXT NOT NULL DEFAULT '',
    copy_files   TEXT NOT NULL DEFAULT '',
    updated_at   TEXT NOT NULL
) STRICT;

CREATE TABLE script_runs (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    trigger      TEXT NOT NULL,
    status       TEXT NOT NULL,
    exit_code    INTEGER,
    output       TEXT NOT NULL DEFAULT '',
    started_at   TEXT NOT NULL,
    finished_at  TEXT
) STRICT;
CREATE INDEX idx_script_runs ON script_runs(workspace_id, started_at DESC);
