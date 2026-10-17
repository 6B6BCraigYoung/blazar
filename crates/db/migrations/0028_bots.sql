CREATE TABLE bots (
    id             TEXT PRIMARY KEY,
    handle         TEXT NOT NULL UNIQUE,
    display_name   TEXT NOT NULL,
    role           TEXT NOT NULL DEFAULT 'worker',
    agent_id       TEXT NOT NULL REFERENCES agents(id),
    home_ws_id     TEXT NOT NULL REFERENCES workspaces(id),
    brain          TEXT NOT NULL DEFAULT 'hub',
    brain_ws_id    TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    priority       TEXT NOT NULL DEFAULT 'P1',
    runtimes       TEXT NOT NULL DEFAULT '[]',
    hot_until      TEXT,
    status         TEXT NOT NULL DEFAULT 'idle',
    status_note    TEXT NOT NULL DEFAULT '',
    last_active_at TEXT,
    archived_at    TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
) STRICT;

CREATE TABLE bot_threads (
    id         TEXT PRIMARY KEY,
    bot_id     TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    thread_id  TEXT,
    kind       TEXT NOT NULL,
    title      TEXT NOT NULL DEFAULT '',
    task_id    TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    runtime    TEXT,
    summary    TEXT NOT NULL DEFAULT '',
    closed_at  TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
CREATE INDEX idx_bot_threads_bot ON bot_threads(bot_id, closed_at);
