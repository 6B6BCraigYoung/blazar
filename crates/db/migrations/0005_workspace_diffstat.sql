ALTER TABLE workspaces ADD COLUMN diff_added INTEGER;
ALTER TABLE workspaces ADD COLUMN diff_removed INTEGER;
ALTER TABLE workspaces ADD COLUMN diff_files INTEGER;
ALTER TABLE workspaces ADD COLUMN diff_at TEXT;
