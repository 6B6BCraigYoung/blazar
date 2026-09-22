DROP TABLE IF EXISTS leases;
DROP TABLE IF EXISTS tasks;
CREATE TABLE tasks (
    id            TEXT PRIMARY KEY,
    number        INTEGER NOT NULL UNIQUE,
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'todo',
    priority      TEXT NOT NULL DEFAULT 'none',
    labels        TEXT NOT NULL DEFAULT '[]',
    parent_id     TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    workspace_id  TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    agent_profile TEXT,
    runtime       TEXT,
    thread_id     TEXT,
    position      REAL NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    started_at    TEXT,
    completed_at  TEXT
) STRICT;
CREATE INDEX idx_tasks_status ON tasks(status, position);
CREATE INDEX idx_tasks_thread ON tasks(thread_id);
CREATE INDEX idx_tasks_workspace ON tasks(workspace_id);

CREATE TABLE task_comments (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    note       INTEGER NOT NULL DEFAULT 0,
    session_id TEXT,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX idx_task_comments_task ON task_comments(task_id, created_at);
