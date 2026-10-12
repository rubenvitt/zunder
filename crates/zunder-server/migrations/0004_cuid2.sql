-- Schema 4: IDs of areas, projects, tasks and operations are CUID2 strings
-- (24 characters) instead of UUIDs. Existing data is rewritten by the server
-- right after this migration (db::migrate_legacy_ids, with a backup taken
-- before). A server older than this version refuses the database.
SELECT 1;
