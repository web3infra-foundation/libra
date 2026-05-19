-- 2026050501_agent_checkpoint_parent_nullable
--
-- Phase 2 follow-up: `agent_checkpoint.parent_commit` was originally declared
-- `TEXT NOT NULL`, which forced the runtime to bind an empty string when the
-- user-branch HEAD was not yet born (or the lookup failed). The empty-string
-- encoding conflates "no HEAD yet" with "lookup error" and breaks downstream
-- consumers that try to treat the column as a Git oid.
--
-- SQLite cannot drop a NOT NULL constraint in place, so we recreate the
-- table. The DDL is wrapped so it is safe to apply on fresh databases too —
-- if the table was created by `2026050303_agent_capture.sql` immediately
-- before, the data copy is exhaustive and idempotent. If the table somehow
-- does not exist yet (legacy database resurrection), the rename step turns
-- into a no-op and the fresh table created at the end carries the relaxed
-- shape on its own.
--
-- The agent_checkpoint indexes are dropped and recreated alongside the
-- table swap so the unique/index definitions stay in sync.

-- Phase 1: drop indexes and rename the existing table out of the way.
DROP INDEX IF EXISTS `idx_agent_checkpoint_scope`;
DROP INDEX IF EXISTS `idx_agent_checkpoint_session`;
ALTER TABLE `agent_checkpoint` RENAME TO `agent_checkpoint__old_2026050303`;

-- Phase 2: re-create the table with `parent_commit` nullable.
CREATE TABLE `agent_checkpoint` (
    `checkpoint_id`        TEXT PRIMARY KEY,
    `session_id`           TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `parent_checkpoint_id` TEXT,
    `scope`                TEXT NOT NULL CHECK(`scope` IN ('temporary','committed','subagent')),
    `parent_commit`        TEXT,
    `tree_oid`             TEXT NOT NULL,
    `metadata_blob_oid`    TEXT NOT NULL,
    `traces_commit`        TEXT NOT NULL,
    `tool_use_id`          TEXT,
    `subagent_session_id`  TEXT,
    `description`          TEXT,
    `created_at`           INTEGER NOT NULL
);

-- Phase 3: copy any pre-existing rows over. Empty parent_commit strings
-- become NULL so callers that distinguish "no HEAD" from "an actual oid"
-- stop seeing the conflated sentinel. Other columns round-trip verbatim.
INSERT INTO `agent_checkpoint` (
    `checkpoint_id`, `session_id`, `parent_checkpoint_id`, `scope`,
    `parent_commit`, `tree_oid`, `metadata_blob_oid`, `traces_commit`,
    `tool_use_id`, `subagent_session_id`, `description`, `created_at`
)
SELECT
    `checkpoint_id`, `session_id`, `parent_checkpoint_id`, `scope`,
    NULLIF(`parent_commit`, ''), `tree_oid`, `metadata_blob_oid`, `traces_commit`,
    `tool_use_id`, `subagent_session_id`, `description`, `created_at`
FROM `agent_checkpoint__old_2026050303`;

-- Phase 4: drop the temporary copy.
DROP TABLE `agent_checkpoint__old_2026050303`;

-- Phase 5: re-create the indexes that the original migration declared.
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_session`
    ON `agent_checkpoint`(`session_id`, `created_at`);
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_scope`
    ON `agent_checkpoint`(`scope`);
