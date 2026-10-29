CREATE TABLE review_comments (
    id           TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file         TEXT NOT NULL,
    side         TEXT NOT NULL,
    line         INTEGER NOT NULL,
    code         TEXT NOT NULL,
    text         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, id)
) STRICT;

CREATE TABLE discards (
    token        TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entries      TEXT NOT NULL,
    created_at   TEXT NOT NULL
) STRICT;

CREATE INDEX idx_discards_created ON discards(created_at);
