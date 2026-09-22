CREATE TABLE nodes (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    transport     TEXT NOT NULL,
    endpoint      TEXT,
    network       TEXT,
    os            TEXT,
    arch          TEXT,
    labels        TEXT NOT NULL DEFAULT '[]',
    capabilities  TEXT NOT NULL DEFAULT '{}',
    load          TEXT NOT NULL DEFAULT '{}',
    status        TEXT NOT NULL DEFAULT 'offline',
    last_seen_at  TEXT,
    created_at    TEXT NOT NULL
) STRICT;

CREATE TABLE accounts (
    id             TEXT PRIMARY KEY,
    provider       TEXT NOT NULL,
    display_name   TEXT NOT NULL,
    auth_mode      TEXT NOT NULL,
    credential_ref TEXT,
    plan           TEXT,
    status         TEXT NOT NULL DEFAULT 'active',
    created_at     TEXT NOT NULL
) STRICT;

CREATE TABLE rate_limit_snapshots (
    account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    window_name  TEXT NOT NULL,
    utilization  REAL NOT NULL,
    resets_at    TEXT,
    observed_at  TEXT NOT NULL,
    PRIMARY KEY (account_id, window_name)
) STRICT;

CREATE TABLE repos (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    canonical_url  TEXT,
    bare_path      TEXT,
    default_branch TEXT NOT NULL DEFAULT 'main',
    created_at     TEXT NOT NULL
) STRICT;

CREATE TABLE projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    repo_id    TEXT REFERENCES repos(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE workspaces (
    id             TEXT PRIMARY KEY,
    project_id     TEXT REFERENCES projects(id) ON DELETE SET NULL,
    node_id        TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    path           TEXT NOT NULL,
    branch         TEXT,
    base_commit    TEXT,
    origin         TEXT NOT NULL DEFAULT 'manual',
    task_id        TEXT,
    activity       TEXT NOT NULL DEFAULT 'idle',
    status         TEXT NOT NULL DEFAULT 'active',
    last_active_at TEXT,
    created_at     TEXT NOT NULL,
    UNIQUE (node_id, path)
) STRICT;

CREATE INDEX idx_workspaces_project ON workspaces(project_id);
CREATE INDEX idx_workspaces_node ON workspaces(node_id);
CREATE INDEX idx_workspaces_activity ON workspaces(activity);

CREATE TABLE workspace_repos (
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repo_id       TEXT NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    target_branch TEXT,
    PRIMARY KEY (workspace_id, repo_id)
) STRICT;

CREATE TABLE workspace_context (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (workspace_id, kind)
) STRICT;

CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    runtime_kind        TEXT NOT NULL,
    account_id          TEXT REFERENCES accounts(id) ON DELETE SET NULL,
    provider_session_id TEXT,
    status              TEXT NOT NULL DEFAULT 'running',
    created_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_sessions_workspace ON sessions(workspace_id);

CREATE TABLE events (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    ts         TEXT NOT NULL,
    payload    TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
) STRICT;

CREATE TABLE approvals (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    request     TEXT NOT NULL,
    decision    TEXT,
    decided_at  TEXT,
    created_at  TEXT NOT NULL
) STRICT;

CREATE INDEX idx_approvals_pending ON approvals(session_id) WHERE decision IS NULL;

CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    project_id   TEXT REFERENCES projects(id) ON DELETE SET NULL,
    title        TEXT NOT NULL,
    body         TEXT,
    requirements TEXT NOT NULL DEFAULT '{}',
    fanout       INTEGER NOT NULL DEFAULT 1,
    priority     INTEGER NOT NULL DEFAULT 0,
    status       TEXT NOT NULL DEFAULT 'draft',
    created_at   TEXT NOT NULL
) STRICT;

CREATE TABLE leases (
    task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    token       TEXT NOT NULL,
    acquired_at TEXT NOT NULL,
    expires_at  TEXT NOT NULL
) STRICT;
