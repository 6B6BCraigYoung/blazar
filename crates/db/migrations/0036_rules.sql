ALTER TABLE inbox ADD COLUMN resolved_at TEXT;

CREATE TABLE rule_alerts (
    rule         TEXT NOT NULL,
    subject      TEXT NOT NULL,
    since        TEXT NOT NULL,
    raised_at    TEXT,
    escalated_at TEXT,
    resolved_at  TEXT,
    inbox_id     TEXT,
    PRIMARY KEY (rule, subject)
) STRICT;
