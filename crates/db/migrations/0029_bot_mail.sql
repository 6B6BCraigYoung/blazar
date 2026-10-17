CREATE TABLE bot_mail (
    id           TEXT PRIMARY KEY,
    to_bot       TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    thread_hint  TEXT,
    from_kind    TEXT NOT NULL,
    from_ref     TEXT NOT NULL DEFAULT '',
    from_thread  TEXT,
    reply_to     TEXT REFERENCES bot_mail(id) ON DELETE SET NULL,
    subject      TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL,
    attachments  TEXT NOT NULL DEFAULT '[]',
    priority     TEXT NOT NULL DEFAULT 'normal',
    status       TEXT NOT NULL DEFAULT 'queued',
    session_id   TEXT,
    delivered_at TEXT,
    done_at      TEXT,
    expires_at   TEXT,
    created_at   TEXT NOT NULL
) STRICT;
CREATE INDEX idx_bot_mail_pending ON bot_mail(to_bot, status, created_at);
CREATE INDEX idx_bot_mail_reply ON bot_mail(reply_to);
CREATE INDEX idx_bot_mail_session ON bot_mail(session_id);
