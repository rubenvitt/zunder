-- Schema 6: comments on tasks (entity `comment`). SQLite cannot change a
-- CHECK constraint, so `ops` and `entities` are rebuilt with the new list of
-- entity kinds. `seq` values and the AUTOINCREMENT high-water mark are kept
-- (a cursor is never reused).

CREATE TABLE ops_new (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id       TEXT    NOT NULL UNIQUE,
    owner_id    TEXT    NOT NULL REFERENCES users(id),
    entity      TEXT    NOT NULL CHECK (entity IN ('area', 'project', 'task', 'comment')),
    entity_id   TEXT    NOT NULL,
    field       TEXT    NOT NULL,
    value_json  TEXT    NOT NULL,
    hlc         TEXT    NOT NULL,
    source      TEXT    NOT NULL,
    client_id   TEXT,
    received_at INTEGER NOT NULL
);
INSERT INTO ops_new (seq, op_id, owner_id, entity, entity_id, field, value_json, hlc, source,
                     client_id, received_at)
    SELECT seq, op_id, owner_id, entity, entity_id, field, value_json, hlc, source, client_id,
           received_at
    FROM ops;
DELETE FROM sqlite_sequence WHERE name = 'ops_new';
INSERT INTO sqlite_sequence (name, seq) SELECT 'ops_new', seq FROM sqlite_sequence WHERE name = 'ops';
DROP TABLE ops;
ALTER TABLE ops_new RENAME TO ops;
CREATE INDEX ops_owner_entity ON ops (owner_id, entity_id, seq);
CREATE INDEX ops_owner_seq ON ops (owner_id, seq);

CREATE TABLE entities_new (
    owner_id    TEXT    NOT NULL REFERENCES users(id),
    entity      TEXT    NOT NULL CHECK (entity IN ('area', 'project', 'task', 'comment')),
    entity_id   TEXT    NOT NULL,
    created_hlc TEXT    NOT NULL,
    updated_hlc TEXT    NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0,
    fields_json TEXT    NOT NULL,
    PRIMARY KEY (owner_id, entity, entity_id)
);
INSERT INTO entities_new SELECT owner_id, entity, entity_id, created_hlc, updated_hlc, deleted,
                                fields_json
    FROM entities;
DROP TABLE entities;
ALTER TABLE entities_new RENAME TO entities;
