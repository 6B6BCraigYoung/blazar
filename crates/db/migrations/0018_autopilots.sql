CREATE TABLE autopilots (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    instructions    TEXT NOT NULL,
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    agent_profile   TEXT,
    runtime         TEXT,
    model           TEXT,
    permission_mode TEXT,
    mode            TEXT NOT NULL DEFAULT 'task',
    title_template  TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
    paused_reason   TEXT,
    cron            TEXT,
    timezone        TEXT NOT NULL DEFAULT 'UTC',
    next_run_at     INTEGER,
    concurrency     TEXT NOT NULL DEFAULT 'skip',
    webhook_token   TEXT UNIQUE,
    fail_streak     INTEGER NOT NULL DEFAULT 0,
    last_run_at     TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
) STRICT;
CREATE INDEX idx_autopilots_due ON autopilots(next_run_at) WHERE status = 'active';

CREATE TABLE autopilot_runs (
    id           TEXT PRIMARY KEY,
    autopilot_id TEXT NOT NULL REFERENCES autopilots(id) ON DELETE CASCADE,
    source       TEXT NOT NULL,
    status       TEXT NOT NULL,
    reason       TEXT,
    task_id      TEXT,
    thread_id    TEXT,
    session_id   TEXT,
    payload      TEXT,
    triggered_at TEXT NOT NULL,
    completed_at TEXT
) STRICT;
CREATE INDEX idx_autopilot_runs ON autopilot_runs(autopilot_id, triggered_at DESC);
CREATE INDEX idx_autopilot_runs_session ON autopilot_runs(session_id);
