-- Rollback: restore the NOT NULL constraint on `agent_checkpoint.parent_commit`.
--
-- This is the symmetric inverse of the up migration: rename, recreate with
-- the original NOT NULL shape, copy data substituting empty strings for any
-- NULL parent_commit values (lossy but deterministic), drop the temporary
-- copy. Indexes are recreated.

DROP INDEX IF EXISTS `idx_agent_checkpoint_scope`;
DROP INDEX IF EXISTS `idx_agent_checkpoint_session`;
ALTER TABLE `agent_checkpoint` RENAME TO `agent_checkpoint__nullable_2026050501`;

CREATE TABLE `agent_checkpoint` (
    `checkpoint_id`        TEXT PRIMARY KEY,
    `session_id`           TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `parent_checkpoint_id` TEXT,
    `scope`                TEXT NOT NULL CHECK(`scope` IN ('temporary','committed','subagent')),
    `parent_commit`        TEXT NOT NULL,
    `tree_oid`             TEXT NOT NULL,
    `metadata_blob_oid`    TEXT NOT NULL,
    `traces_commit`        TEXT NOT NULL,
    `tool_use_id`          TEXT,
    `subagent_session_id`  TEXT,
    `description`          TEXT,
    `created_at`           INTEGER NOT NULL
);

INSERT INTO `agent_checkpoint` (
    `checkpoint_id`, `session_id`, `parent_checkpoint_id`, `scope`,
    `parent_commit`, `tree_oid`, `metadata_blob_oid`, `traces_commit`,
    `tool_use_id`, `subagent_session_id`, `description`, `created_at`
)
SELECT
    `checkpoint_id`, `session_id`, `parent_checkpoint_id`, `scope`,
    COALESCE(`parent_commit`, ''), `tree_oid`, `metadata_blob_oid`, `traces_commit`,
    `tool_use_id`, `subagent_session_id`, `description`, `created_at`
FROM `agent_checkpoint__nullable_2026050501`;

DROP TABLE `agent_checkpoint__nullable_2026050501`;

CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_session`
    ON `agent_checkpoint`(`session_id`, `created_at`);
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_scope`
    ON `agent_checkpoint`(`scope`);
