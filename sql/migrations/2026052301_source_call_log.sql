CREATE TABLE IF NOT EXISTS `source_call_log` (
    `id` TEXT PRIMARY KEY,
    `session_id` TEXT NOT NULL,
    `source_slug` TEXT NOT NULL,
    `tool_name` TEXT NOT NULL,
    `registered_tool_name` TEXT NOT NULL,
    `tool_call_id` TEXT NOT NULL,
    `credential_ref` TEXT,
    `latency_ms` INTEGER,
    `input_bytes` INTEGER NOT NULL DEFAULT 0,
    `output_bytes` INTEGER NOT NULL DEFAULT 0,
    `cost_estimate_micros` INTEGER,
    `approval_decision` TEXT,
    `state_namespace` TEXT NOT NULL,
    `success` INTEGER NOT NULL DEFAULT 1,
    `created_at` TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_session`
    ON `source_call_log` (`session_id`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_source_slug`
    ON `source_call_log` (`source_slug`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_tool_call_id`
    ON `source_call_log` (`tool_call_id`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_created`
    ON `source_call_log` (`created_at`);
