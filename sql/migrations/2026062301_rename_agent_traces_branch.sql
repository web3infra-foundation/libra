-- Rename the external-agent capture ref from the legacy `agent-traces`
-- short name to the single-word `traces` (refs/libra/traces).
--
-- Idempotent + conflict-safe:
--   * Only the local capture branch row (kind = 'Branch', remote IS NULL) is
--     touched; remote-tracking rows and tags are left alone.
--   * The rename is skipped when a `traces` branch row already exists (a fresh
--     repo created after the rename, or a re-run), so the partial UNIQUE index
--     `idx_name_kind` (name, kind WHERE remote IS NULL) can never collide.
--   * Reflog rows recorded under the old name are carried over so the
--     pre-rename history stays attached to `traces`.
--
-- Schema guard: this is the first built-in migration to mutate the Git-core
-- `reference`/`reflog` tables (every earlier migration only creates its own AI
-- tables). In production those tables are always created by the bootstrap
-- schema (`sqlite_20260309_init.sql`) before any migration runs, so the
-- `IF NOT EXISTS` guards below are no-ops there. They exist to preserve the
-- long-standing invariant that the built-in migration set applies cleanly on a
-- bare connection — relied on by the migration-runner unit and integration
-- tests — which would otherwise abort here with "no such table: reference".
-- The DDL is copied verbatim from `sqlite_20260309_init.sql`.
CREATE TABLE IF NOT EXISTS `reference` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    -- name can't be ''
    `name` TEXT CHECK (name <> '' OR name IS NULL),
    `kind` TEXT NOT NULL CHECK (kind IN ('Branch', 'Tag', 'Head')),
    `commit` TEXT,
    -- remote can't be ''. If kind is Tag, remote must be NULL.
    `remote` TEXT CHECK (remote <> '' OR remote IS NULL),
    CHECK (
        (kind <> 'Tag' OR remote IS NULL)
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
-- The matching indexes must be created too, so a migration-only (bare) DB has
-- the same constraints production has — in particular the partial UNIQUE index
-- `idx_name_kind` that the conflict-safety `NOT EXISTS` clause below relies on
-- to keep the `traces` rename from colliding with an existing branch row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind_remote ON `reference`(`name`, `kind`, `remote`)
WHERE `remote` IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind ON `reference`(`name`, `kind`)
WHERE `remote` IS NULL;
CREATE INDEX IF NOT EXISTS idx_ref_name_timestamp ON `reflog`(`ref_name`, `timestamp`);

UPDATE `reference`
SET `name` = 'traces'
WHERE `name` = 'agent-traces'
  AND `kind` = 'Branch'
  AND `remote` IS NULL
  AND NOT EXISTS (
    SELECT 1 FROM `reference` AS existing
    WHERE existing.`name` = 'traces'
      AND existing.`kind` = 'Branch'
      AND existing.`remote` IS NULL
  );

UPDATE `reflog` SET `ref_name` = 'traces' WHERE `ref_name` = 'agent-traces';
UPDATE `reflog`
SET `ref_name` = 'refs/heads/traces'
WHERE `ref_name` = 'refs/heads/agent-traces';
