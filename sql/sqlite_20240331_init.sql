CREATE TABLE IF NOT EXISTS `config` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `configuration` TEXT NOT NULL,
    `name` TEXT,
    `key` TEXT NOT NULL,
    `value` TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS `reference` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    -- name can't be ''
    `name` TEXT CHECK (name <> '' OR name IS NULL),
    `kind` TEXT NOT NULL CHECK (kind IN ('Branch', 'Tag', 'Head')),
    `commit` TEXT,
    -- remote can't be ''. If kind is Tag, remote must be NULL.
    `remote` TEXT CHECK (remote <> '' OR remote IS NULL),
    CHECK (
        (kind <> 'Tag' OR (kind = 'Tag' AND remote IS NULL))
    )
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
CREATE TABLE IF NOT EXISTS `rebase_state` (
    `id`           INTEGER PRIMARY KEY AUTOINCREMENT,
    `head_name`    TEXT NOT NULL,
    `onto`         TEXT NOT NULL,
    `orig_head`    TEXT NOT NULL,
    `current_head` TEXT NOT NULL,
    `todo`         TEXT NOT NULL,
    `done`         TEXT NOT NULL,
    `stopped_sha`  TEXT
);
--  (name, kind, remote) as unique key when remote is not null
CREATE UNIQUE INDEX idx_name_kind_remote ON `reference`(`name`, `kind`, `remote`)
WHERE `remote` IS NOT NULL;

-- (name, kind) as unique key when remote is null
CREATE UNIQUE INDEX idx_name_kind ON `reference`(`name`, `kind`)
WHERE `remote` IS NULL;

CREATE INDEX idx_ref_name_timestamp ON `reflog`(`ref_name`, `timestamp`);

-- Object index table for cloud backup (D1/R2)
CREATE TABLE IF NOT EXISTS `object_index` (
    `id`         INTEGER PRIMARY KEY AUTOINCREMENT,
    `o_id`       TEXT NOT NULL,             -- Object Hash (SHA-1/SHA-256)
    `o_type`     TEXT NOT NULL,             -- Type: blob, tree, commit, tag
    `o_size`     INTEGER NOT NULL,          -- Original object size in bytes
    `repo_id`    TEXT NOT NULL,             -- Repository UUID for multi-tenant isolation
    `created_at` INTEGER NOT NULL,          -- Unix timestamp
    `is_synced`  INTEGER DEFAULT 0,         -- 0=not synced to cloud, 1=synced
    UNIQUE(`repo_id`, `o_id`)               -- Support same object in different repos
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_object_repo_oid ON `object_index`(`repo_id`, `o_id`);
CREATE INDEX IF NOT EXISTS idx_object_sync ON `object_index`(`repo_id`, `is_synced`);

