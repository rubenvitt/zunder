-- Zunder schema, version 3: devices. The Tauri app logs in through the
-- built-in authorization server (resource <public>/api) and receives a
-- device token (`zd_…`, stored as SHA-256 hex) that does not expire and is
-- revoked per device. No foreign key to oauth_clients: registered clients
-- are purged after a day, the device stays. `family_id` is the
-- authorization code's family, so replaying the code revokes the device.
CREATE TABLE devices (
    id           TEXT PRIMARY KEY,
    user_id      TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    family_id    TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
);
CREATE INDEX devices_user ON devices (user_id);
CREATE INDEX devices_family ON devices (family_id);
