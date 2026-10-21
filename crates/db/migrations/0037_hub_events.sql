CREATE TABLE hub_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX idx_hub_events_created ON hub_events(created_at);
