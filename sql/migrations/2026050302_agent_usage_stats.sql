CREATE TABLE IF NOT EXISTS `agent_usage_stats` (
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
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_provider_model`
    ON `agent_usage_stats` (`provider`, `model`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_thread`
    ON `agent_usage_stats` (`thread_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_session`
    ON `agent_usage_stats` (`session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_started`
    ON `agent_usage_stats` (`started_at`);
