-- 2026050303_agent_capture: external-Agent session and checkpoint catalog.
--
-- Backs `libra agent` (external-agent capture). The Git side of the world owns
-- the transcript / event-stream blobs: this schema only tracks the lightweight
-- summaries that are cheap to query and join. See docs/improvement/entire.md
-- (sections 3.4 and 4.1) for the full rationale.
--
-- Idempotency: every DDL statement uses `IF NOT EXISTS`, so the migration is
-- safe to apply on legacy databases that may already have a partial shape.

CREATE TABLE IF NOT EXISTS `agent_session` (
    `session_id`           TEXT PRIMARY KEY,
    -- Closed enum mirrored by `AgentKind::as_db_str` in
    -- `src/internal/ai/observed_agents/adapter.rs`. Adding a new value here
    -- requires a paired migration that bumps the CHECK constraint.
    `agent_kind`           TEXT NOT NULL CHECK(`agent_kind` IN (
        'claude_code', 'cursor', 'codex', 'gemini',
        'opencode', 'copilot', 'factory_ai'
    )),
    `provider_session_id`  TEXT NOT NULL,
    -- Soft FK to `ai_thread(thread_id)`; ON DELETE SET NULL because losing the
    -- thread row should not cascade-delete captured external-agent sessions.
    `thread_id`            TEXT REFERENCES `ai_thread`(`thread_id`) ON DELETE SET NULL,
    `state`                TEXT NOT NULL CHECK(`state` IN ('pending','active','condensed','stopped','quarantined')),
    `working_dir`          TEXT NOT NULL,
    `worktree_id`          TEXT,
    `parent_commit`        TEXT,
    `parent_session_id`    TEXT,
    `metadata_json`        TEXT NOT NULL DEFAULT '{}',
    `redaction_report`     TEXT NOT NULL DEFAULT '{}',
    `started_at`           INTEGER NOT NULL,
    `last_event_at`        INTEGER NOT NULL,
    `stopped_at`           INTEGER,
    `schema_version`       INTEGER NOT NULL DEFAULT 1
);

CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_session_provider`
    ON `agent_session`(`agent_kind`, `provider_session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_session_active`
    ON `agent_session`(`state`, `working_dir`) WHERE `state` = 'active';
CREATE INDEX IF NOT EXISTS `idx_agent_session_thread`
    ON `agent_session`(`thread_id`);

CREATE TABLE IF NOT EXISTS `agent_checkpoint` (
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

CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_session`
    ON `agent_checkpoint`(`session_id`, `created_at`);
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_scope`
    ON `agent_checkpoint`(`scope`);
