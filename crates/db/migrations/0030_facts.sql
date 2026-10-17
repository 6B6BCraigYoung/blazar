CREATE TABLE facts (
    key        TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    owner_bot  TEXT REFERENCES bots(id) ON DELETE SET NULL,
    value      TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'ok',
    ttl_secs   INTEGER,
    updated_at TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE TABLE fact_history (
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    at    TEXT NOT NULL
) STRICT;
CREATE INDEX idx_fact_history_key ON fact_history(key, at);
