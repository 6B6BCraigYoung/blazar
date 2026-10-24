UPDATE accounts SET display_name = '默认账号'
WHERE id IN ('claude-default', 'codex-default') AND display_name = '默认登录';
