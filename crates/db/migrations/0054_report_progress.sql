ALTER TABLE inbox ADD COLUMN progress_done INTEGER;
ALTER TABLE inbox ADD COLUMN progress_total INTEGER;
ALTER TABLE inbox ADD COLUMN progress_unit TEXT;
CREATE INDEX idx_inbox_workspace ON inbox(workspace_id, kind, created_at);
