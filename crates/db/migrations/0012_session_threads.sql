ALTER TABLE sessions ADD COLUMN thread_id TEXT;
UPDATE sessions SET thread_id = id WHERE thread_id IS NULL;
