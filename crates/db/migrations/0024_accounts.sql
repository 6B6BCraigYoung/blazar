ALTER TABLE accounts ADD COLUMN config_dir TEXT;
ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE accounts ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE accounts ADD COLUMN checked_at TEXT;

INSERT OR IGNORE INTO accounts (id, provider, display_name, auth_mode, status, created_at)
VALUES ('claude-default', 'claude', '默认登录', 'cli', 'unknown', strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
       ('codex-default', 'codex', '默认登录', 'cli', 'unknown', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

ALTER TABLE agents ADD COLUMN account TEXT;
