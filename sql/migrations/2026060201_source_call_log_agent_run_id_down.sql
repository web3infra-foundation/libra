-- Down migration for 2026060201_source_call_log_agent_run_id.
--
-- SQLite cannot DROP COLUMN before 3.35; use the portable table-rebuild dance
-- (symmetric with the agent_usage_stats_agent_name down migration).

DROP INDEX IF EXISTS `idx_source_call_log_agent_run_id`;

CREATE TABLE IF NOT EXISTS `source_call_log__rebuild` (
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

INSERT INTO `source_call_log__rebuild`
    (id, session_id, source_slug, tool_name, registered_tool_name, tool_call_id,
     credential_ref, latency_ms, input_bytes, output_bytes, cost_estimate_micros,
     approval_decision, state_namespace, success, created_at)
SELECT
    id, session_id, source_slug, tool_name, registered_tool_name, tool_call_id,
    credential_ref, latency_ms, input_bytes, output_bytes, cost_estimate_micros,
    approval_decision, state_namespace, success, created_at
FROM `source_call_log`;

DROP TABLE `source_call_log`;
ALTER TABLE `source_call_log__rebuild` RENAME TO `source_call_log`;

CREATE INDEX IF NOT EXISTS `idx_source_call_log_session`
    ON `source_call_log` (`session_id`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_source_slug`
    ON `source_call_log` (`source_slug`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_tool_call_id`
    ON `source_call_log` (`tool_call_id`);
CREATE INDEX IF NOT EXISTS `idx_source_call_log_created`
    ON `source_call_log` (`created_at`);
