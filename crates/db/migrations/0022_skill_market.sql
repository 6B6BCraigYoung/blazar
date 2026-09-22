ALTER TABLE skills ADD COLUMN origin TEXT;
ALTER TABLE skills ADD COLUMN version TEXT;

CREATE TABLE kv_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
