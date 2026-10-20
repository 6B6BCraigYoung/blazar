CREATE TABLE jobs (
    id                TEXT PRIMARY KEY,
    owner_kind        TEXT NOT NULL,
    bot_id            TEXT REFERENCES bots(id) ON DELETE SET NULL,
    session_id        TEXT,
    workspace_id      TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    node_id           TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL,
    label             TEXT NOT NULL,
    command           TEXT,
    cwd               TEXT,
    run_dir           TEXT,
    pid               INTEGER,
    log_path          TEXT,
    waits_for         TEXT NOT NULL DEFAULT '[]',
    deadline_at       TEXT,
    deadline_noted_at TEXT,
    status            TEXT NOT NULL DEFAULT 'running',
    exit_code         INTEGER,
    notify            TEXT NOT NULL DEFAULT 'wake',
    last_checked_at   TEXT,
    last_error        TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    ended_at          TEXT
) STRICT;
CREATE INDEX idx_jobs_status ON jobs(status);
CREATE INDEX idx_jobs_bot ON jobs(bot_id);
CREATE INDEX idx_jobs_workspace ON jobs(workspace_id);
