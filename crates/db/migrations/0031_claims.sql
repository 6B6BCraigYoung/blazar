CREATE TABLE claims (
    id           TEXT PRIMARY KEY,
    bot_id       TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    node_id      TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    scope        TEXT NOT NULL,
    mode         TEXT NOT NULL,
    note         TEXT NOT NULL DEFAULT '',
    expires_at   TEXT,
    notified_at  TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE INDEX idx_claims_bot ON claims(bot_id);
CREATE INDEX idx_claims_node ON claims(node_id);
