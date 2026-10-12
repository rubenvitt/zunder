-- Schema 7: intake channels. A channel lets a foreign server create tasks in
-- exactly one project (`project_id`, a CUID2 of a project entity) and nothing
-- else. Its token (`zi_…`, stored as SHA-256 hex) is a separate kind of
-- credential: `/api` and `/mcp` only look at `api_tokens` and never accept it.
-- `IF NOT EXISTS`: re-running it (e.g. after `user_version` was set back) is a
-- no-op.
CREATE TABLE IF NOT EXISTS intake_channels (
    id           TEXT PRIMARY KEY,
    user_id      TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    project_id   TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
);
CREATE INDEX IF NOT EXISTS intake_channels_user ON intake_channels (user_id);
CREATE UNIQUE INDEX IF NOT EXISTS intake_channels_active_name ON intake_channels (user_id, name)
    WHERE revoked_at IS NULL;

-- One row per created task, so a repeated `external_id` returns the task it
-- created first (idempotency) and the rate limit can count recent creations.
CREATE TABLE IF NOT EXISTS intake_submissions (
    channel_id  TEXT    NOT NULL REFERENCES intake_channels(id),
    external_id TEXT    NOT NULL,
    task_id     TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (channel_id, external_id)
);
CREATE INDEX IF NOT EXISTS intake_submissions_recent ON intake_submissions (channel_id, created_at);
