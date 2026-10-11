-- Zunder schema, version 2: built-in OAuth 2.1 authorization server for
-- /mcp (claude.ai connectors). Secrets (codes, tokens) are stored as
-- SHA-256 hex only. Timestamps are Unix milliseconds.

-- Dynamically registered public clients (RFC 7591).
CREATE TABLE oauth_clients (
    id                 TEXT PRIMARY KEY,
    name               TEXT    NOT NULL,
    redirect_uris_json TEXT    NOT NULL,
    created_at         INTEGER NOT NULL,
    last_used_at       INTEGER,
    revoked_at         INTEGER
);
CREATE INDEX oauth_clients_created ON oauth_clients (created_at);

-- Authorization codes: short-lived, single use, bound to client,
-- redirect URI, PKCE challenge and resource. `family_id` names the token
-- family issued for the code (revoked when the code is replayed).
CREATE TABLE oauth_codes (
    code_hash      TEXT PRIMARY KEY,
    client_id      TEXT    NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id        TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri   TEXT    NOT NULL,
    code_challenge TEXT    NOT NULL,
    resource       TEXT    NOT NULL,
    scope          TEXT,
    family_id      TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL,
    used_at        INTEGER
);
CREATE INDEX oauth_codes_expires ON oauth_codes (expires_at);

-- Access and refresh tokens. One authorization = one family; refresh
-- tokens rotate within the family, and replaying a rotated refresh token
-- revokes the whole family.
CREATE TABLE oauth_tokens (
    id         TEXT PRIMARY KEY,
    token_hash TEXT    NOT NULL UNIQUE,
    kind       TEXT    NOT NULL CHECK (kind IN ('access', 'refresh')),
    client_id  TEXT    NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    family_id  TEXT    NOT NULL,
    resource   TEXT    NOT NULL,
    scope      TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER,
    revoked_at INTEGER
);
CREATE INDEX oauth_tokens_family ON oauth_tokens (family_id);
CREATE INDEX oauth_tokens_client ON oauth_tokens (client_id);
CREATE INDEX oauth_tokens_expires ON oauth_tokens (expires_at);
