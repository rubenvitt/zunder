-- Schema 9: idempotency keys of MCP `create_task` (ZUN-57). A caller that
-- lost the answer to `create_task` repeats the call with the same
-- `client_ref` and gets the task created first instead of a second one.
-- One row per key and user, written in the same transaction as the task's
-- operations. `IF NOT EXISTS`: re-running it is a no-op.
CREATE TABLE IF NOT EXISTS task_client_refs (
    user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_ref TEXT    NOT NULL,
    task_id    TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, client_ref)
);
