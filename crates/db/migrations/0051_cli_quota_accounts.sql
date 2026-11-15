INSERT OR IGNORE INTO accounts (id, provider, display_name, auth_mode, status, created_at)
VALUES ('devin-default', 'devin', '默认账号', 'cli', 'unknown', strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
       ('droid-default', 'droid', '默认账号', 'cli', 'unknown', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
