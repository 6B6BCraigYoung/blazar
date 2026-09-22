ALTER TABLE sessions ADD COLUMN last_uuid TEXT;
ALTER TABLE sessions ADD COLUMN rewound_at TEXT;

CREATE TABLE queued_messages (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    thread_id    TEXT,
    request      TEXT NOT NULL,
    held         TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX uq_queue_thread ON queued_messages(workspace_id, COALESCE(thread_id, ''));
