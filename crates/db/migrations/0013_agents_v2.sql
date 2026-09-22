ALTER TABLE agents ADD COLUMN archived_at TEXT;
ALTER TABLE agents ADD COLUMN avatar TEXT NOT NULL DEFAULT '';
ALTER TABLE agents ADD COLUMN starters TEXT NOT NULL DEFAULT '[]';
ALTER TABLE agents ADD COLUMN thinking_level TEXT;
ALTER TABLE agents ADD COLUMN custom_args TEXT NOT NULL DEFAULT '[]';
ALTER TABLE agents ADD COLUMN max_concurrent INTEGER NOT NULL DEFAULT 1;
DROP INDEX IF EXISTS uq_agents_name;
CREATE UNIQUE INDEX uq_agents_name_live ON agents(name) WHERE archived_at IS NULL;
