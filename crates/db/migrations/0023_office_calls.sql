CREATE TABLE office_calls (
    id         TEXT PRIMARY KEY,
    app        TEXT NOT NULL,
    command    TEXT NOT NULL,
    risk       TEXT NOT NULL,
    verdict    TEXT NOT NULL,
    exit_code  INTEGER,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX idx_office_calls ON office_calls(app, created_at DESC);
