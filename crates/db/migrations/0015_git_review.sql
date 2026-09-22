ALTER TABLE workspaces ADD COLUMN target_branch TEXT;
ALTER TABLE workspaces ADD COLUMN pr_url TEXT;
ALTER TABLE workspaces ADD COLUMN pr_number INTEGER;
ALTER TABLE workspaces ADD COLUMN pr_state TEXT;
ALTER TABLE workspaces ADD COLUMN pr_checked_at TEXT;
