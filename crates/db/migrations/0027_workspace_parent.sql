ALTER TABLE workspaces ADD COLUMN parent_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL;
CREATE INDEX idx_workspaces_parent ON workspaces(parent_id);
