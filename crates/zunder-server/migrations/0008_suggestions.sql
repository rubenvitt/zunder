-- Schema 8: suggestions of a decision model (Jev, ZUN-52) for inbox tasks:
-- weight, for whom and place, each with its full distribution (`body`, JSON).
-- Only proposals: nothing here changes a task. Whether the owner followed a
-- proposal is derived from the task's values later, not stored.
-- `IF NOT EXISTS`: re-running it is a no-op.
CREATE TABLE IF NOT EXISTS suggestions (
    task_id    TEXT PRIMARY KEY,
    model      TEXT    NOT NULL,
    body       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);
