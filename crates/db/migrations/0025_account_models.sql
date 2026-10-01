ALTER TABLE sessions ADD COLUMN account_auto INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN request TEXT;

CREATE TABLE account_model_blocks (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    reason      TEXT NOT NULL DEFAULT '',
    observed_at TEXT NOT NULL,
    PRIMARY KEY (account_id, model)
) STRICT;

UPDATE accounts SET plan = NULL WHERE plan = 'oauth_token';
