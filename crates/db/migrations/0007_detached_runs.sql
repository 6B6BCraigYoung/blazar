ALTER TABLE sessions ADD COLUMN run_dir TEXT;
ALTER TABLE sessions ADD COLUMN out_offset INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN exit_code INTEGER;
ALTER TABLE sessions ADD COLUMN node TEXT;
ALTER TABLE sessions ADD COLUMN interactive INTEGER NOT NULL DEFAULT 0;

ALTER TABLE approvals ADD COLUMN provider_request_id TEXT;
ALTER TABLE approvals ADD COLUMN src_offset INTEGER;
ALTER TABLE approvals ADD COLUMN workspace_id TEXT;
ALTER TABLE approvals ADD COLUMN decided_by TEXT;
ALTER TABLE approvals ADD COLUMN delivered_at TEXT;

CREATE UNIQUE INDEX idx_approvals_provider ON approvals(session_id, provider_request_id);
CREATE INDEX idx_sessions_detached ON sessions(status) WHERE run_dir IS NOT NULL;
