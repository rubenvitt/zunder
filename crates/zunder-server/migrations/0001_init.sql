-- Zunder schema, version 1.
-- Timestamps are Unix milliseconds (INTEGER), IDs canonical UUID strings.

CREATE TABLE users (
    id           TEXT PRIMARY KEY,
    oidc_subject TEXT UNIQUE,
    email        TEXT,
    name         TEXT,
    created_at   INTEGER NOT NULL
);

-- Append-only operation log. `seq` is the monotonic server cursor
-- (AUTOINCREMENT: never reused).
CREATE TABLE ops (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id       TEXT    NOT NULL UNIQUE,
    owner_id    TEXT    NOT NULL REFERENCES users(id),
    entity      TEXT    NOT NULL CHECK (entity IN ('area', 'project', 'task')),
    entity_id   TEXT    NOT NULL,
    field       TEXT    NOT NULL,
    value_json  TEXT    NOT NULL,
    hlc         TEXT    NOT NULL,
    source      TEXT    NOT NULL,
    client_id   TEXT,
    received_at INTEGER NOT NULL
);
CREATE INDEX ops_owner_entity ON ops (owner_id, entity_id, seq);
CREATE INDEX ops_owner_seq ON ops (owner_id, seq);

-- Materialized state per entity: the winning register of every field
-- (value, hlc, op_id) as JSON, plus derived columns for queries.
CREATE TABLE entities (
    owner_id    TEXT    NOT NULL REFERENCES users(id),
    entity      TEXT    NOT NULL CHECK (entity IN ('area', 'project', 'task')),
    entity_id   TEXT    NOT NULL,
    created_hlc TEXT    NOT NULL,
    updated_hlc TEXT    NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0,
    fields_json TEXT    NOT NULL,
    PRIMARY KEY (owner_id, entity, entity_id)
);

CREATE TABLE api_tokens (
    id           TEXT PRIMARY KEY,
    user_id      TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
);
CREATE INDEX api_tokens_user ON api_tokens (user_id);

CREATE TABLE sessions (
    id_hash      TEXT PRIMARY KEY,
    user_id      TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);
CREATE INDEX sessions_expires ON sessions (expires_at);

-- Key/value bookkeeping, e.g. `materialized_seq:<owner_id>`.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
