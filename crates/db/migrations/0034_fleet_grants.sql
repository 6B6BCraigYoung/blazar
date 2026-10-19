CREATE TABLE fleet_grants (
    token_sha256 TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    node         TEXT NOT NULL,
    caller       TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE INDEX idx_fleet_grants_session ON fleet_grants(session_id);
