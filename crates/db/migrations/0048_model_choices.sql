CREATE TABLE model_choices (
    scope      TEXT NOT NULL,
    runtime    TEXT NOT NULL,
    model      TEXT,
    effort     TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope, runtime)
) STRICT;
