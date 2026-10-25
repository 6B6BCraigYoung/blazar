ALTER TABLE workspaces ADD COLUMN kind TEXT NOT NULL DEFAULT 'project';
CREATE INDEX idx_workspaces_kind ON workspaces(kind);
