-- Down migration for 2026050801_agent_usage_stats_agent_name.
--
-- SQLite cannot DROP COLUMN before 3.35; the down migration uses the
-- table-rebuild dance to be portable across the toolchain matrix Libra
-- supports (rusqlite + sqlx-sqlite both ship 3.40+ in their bundled
-- builds, but we keep the rebuild form for clarity and audit-trail
-- symmetry with the up migration).

DROP INDEX IF EXISTS `idx_agent_usage_stats_agent_name_provider_model`;

CREATE TABLE IF NOT EXISTS `agent_usage_stats__rebuild` (
    `id` TEXT PRIMARY KEY,
    `session_id` TEXT,
    `thread_id` TEXT,
    `agent_run_id` TEXT,
    `run_id` TEXT,
    `provider` TEXT NOT NULL,
    `model` TEXT NOT NULL,
    `request_kind` TEXT NOT NULL DEFAULT 'completion',
    `intent` TEXT,
    `prompt_tokens` INTEGER NOT NULL DEFAULT 0,
    `completion_tokens` INTEGER NOT NULL DEFAULT 0,
    `cached_tokens` INTEGER NOT NULL DEFAULT 0,
    `reasoning_tokens` INTEGER NOT NULL DEFAULT 0,
    `total_tokens` INTEGER NOT NULL DEFAULT 0,
    `tool_call_count` INTEGER NOT NULL DEFAULT 0,
    `wall_clock_ms` INTEGER NOT NULL DEFAULT 0,
    `provider_latency_ms` INTEGER,
    `cost_estimate_micro_dollars` INTEGER,
    `cost_usd` REAL,
    `usage_estimated` INTEGER NOT NULL DEFAULT 0,
    `started_at` TEXT,
    `finished_at` TEXT,
    `success` INTEGER NOT NULL DEFAULT 1,
    `error_kind` TEXT,
    `schema_version` INTEGER NOT NULL DEFAULT 1,
    `created_at` TEXT NOT NULL
);

INSERT INTO `agent_usage_stats__rebuild`
    (id, session_id, thread_id, agent_run_id, run_id, provider, model,
     request_kind, intent, prompt_tokens, completion_tokens, cached_tokens,
     reasoning_tokens, total_tokens, tool_call_count, wall_clock_ms,
     provider_latency_ms, cost_estimate_micro_dollars, cost_usd,
     usage_estimated, started_at, finished_at, success, error_kind,
     schema_version, created_at)
SELECT
    id, session_id, thread_id, agent_run_id, run_id, provider, model,
    request_kind, intent, prompt_tokens, completion_tokens, cached_tokens,
    reasoning_tokens, total_tokens, tool_call_count, wall_clock_ms,
    provider_latency_ms, cost_estimate_micro_dollars, cost_usd,
    usage_estimated, started_at, finished_at, success, error_kind,
    schema_version, created_at
FROM `agent_usage_stats`;

DROP TABLE `agent_usage_stats`;
ALTER TABLE `agent_usage_stats__rebuild` RENAME TO `agent_usage_stats`;

-- Recreate the original indexes (the rebuild table started without them).
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_provider_model`
    ON `agent_usage_stats` (`provider`, `model`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_thread`
    ON `agent_usage_stats` (`thread_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_session`
    ON `agent_usage_stats` (`session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_started`
    ON `agent_usage_stats` (`started_at`);
