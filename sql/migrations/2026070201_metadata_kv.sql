-- Unified scoped metadata KV store (lore.md 1.5): the single table for all
-- non-repo scoped metadata (repo scope lives in config_kv under `metadata.*`).
-- protect / archive / lineage.* are KEYS in this table, never new tables.
-- v1 uses scope='branch' with target = the local branch short name; the scope
-- column is app-validated (no CHECK) so future scopes need no table rebuild.
CREATE TABLE IF NOT EXISTS `metadata_kv` (
    `id`         INTEGER PRIMARY KEY AUTOINCREMENT,
    `scope`      TEXT NOT NULL,
    `target`     TEXT NOT NULL,
    `key`        TEXT NOT NULL,
    `value`      TEXT NOT NULL,
    `value_type` TEXT NOT NULL DEFAULT 'text',
    `created_at` TEXT NOT NULL,
    `updated_at` TEXT NOT NULL,
    UNIQUE(`scope`, `target`, `key`)
);
