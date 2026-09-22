CREATE TABLE hooks (
    id           TEXT PRIMARY KEY,
    event        TEXT NOT NULL,
    command      TEXT NOT NULL,
    target       TEXT NOT NULL DEFAULT 'hub',
    matcher      TEXT,
    blocking     INTEGER NOT NULL DEFAULT 0,
    timeout_secs INTEGER NOT NULL DEFAULT 30,
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL
) STRICT;

CREATE INDEX idx_hooks_event ON hooks(event) WHERE enabled = 1;
