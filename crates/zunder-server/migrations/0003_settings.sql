-- Zunder schema, version 3: instance settings changed in the web UI (e.g.
-- the Timing bridge). They override the matching environment variables.
-- Values are plain text: the Timing API needs its token in clear, so the
-- database (and its backups) hold it like deploy/.env does.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);
