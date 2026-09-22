CREATE TABLE mesh_invites (
    id           TEXT PRIMARY KEY,
    member       TEXT NOT NULL,
    hostname     TEXT NOT NULL,
    ipv4         TEXT,
    network_name TEXT NOT NULL,
    issued_by    TEXT NOT NULL DEFAULT '',
    note         TEXT NOT NULL DEFAULT '',
    issued_at    INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    revoked_at   INTEGER
);

CREATE INDEX idx_mesh_invites_active ON mesh_invites (revoked_at, expires_at);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
