CREATE TABLE memories (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL,
    type        TEXT NOT NULL,
    body        TEXT NOT NULL,
    version     INTEGER NOT NULL DEFAULT 1,
    updated_by  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    archived_at TEXT,
    UNIQUE (scope, name)
) STRICT;
