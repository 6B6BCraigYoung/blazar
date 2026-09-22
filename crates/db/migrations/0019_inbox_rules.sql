CREATE TABLE inbox (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    title        TEXT NOT NULL,
    body         TEXT NOT NULL DEFAULT '',
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    thread_id    TEXT,
    session_id   TEXT,
    ref_id       TEXT,
    read_at      TEXT,
    archived_at  TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE INDEX idx_inbox_open ON inbox(archived_at, created_at DESC);
CREATE INDEX idx_inbox_ref ON inbox(ref_id);

CREATE TABLE approval_rules (
    id           TEXT PRIMARY KEY,
    tool         TEXT NOT NULL,
    pattern      TEXT NOT NULL DEFAULT '',
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    enabled      INTEGER NOT NULL DEFAULT 1,
    hits         INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
) STRICT;
