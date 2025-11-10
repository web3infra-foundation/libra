-- Turso-compatible schema (without CHECK constraints)
-- Validation logic moved to application layer

CREATE TABLE IF NOT EXISTS `config` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `configuration` TEXT NOT NULL,
    `name` TEXT,
    `key` TEXT NOT NULL,
    `value` TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS `reference` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `name` TEXT,
    `kind` TEXT NOT NULL,
    `commit` TEXT,
    `remote` TEXT
);

CREATE TABLE IF NOT EXISTS `reflog` (
    `id`              INTEGER PRIMARY KEY AUTOINCREMENT,
    `ref_name`        TEXT NOT NULL,
    `old_oid`         TEXT NOT NULL,
    `new_oid`         TEXT NOT NULL,
    `committer_name`  TEXT NOT NULL,
    `committer_email` TEXT NOT NULL,
    `timestamp`       INTEGER NOT NULL,
    `action`          TEXT NOT NULL,
    `message`         TEXT NOT NULL
);

-- Partial indexes for unique constraints
CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind_remote ON `reference`(`name`, `kind`, `remote`)
WHERE `remote` IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind ON `reference`(`name`, `kind`)
WHERE `remote` IS NULL;

CREATE INDEX IF NOT EXISTS idx_ref_name_timestamp ON `reflog`(`ref_name`, `timestamp`);
