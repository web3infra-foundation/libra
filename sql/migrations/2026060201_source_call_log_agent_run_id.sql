-- 2026060201_source_call_log_agent_run_id: CEX-S2-14 trace chain.
--
-- Adds the `agent_run_id` dimension to `source_call_log` so a Source-Pool call
-- made inside a sub-agent's tool loop is attributable to its run, completing
-- the `thread → agent_run_id → tool_call_id → source_call` trace chain (the
-- `agent_run_id` leg in `agent_usage_stats` landed earlier; this adds it to the
-- source-call telemetry).
--
-- Additive: a NULL value is the equivalence-class for "main-session (non
-- sub-agent) source call" — old rows stay queryable through the existing
-- indexes. The producer populates it from the invocation's
-- `ToolRuntimeContext::agent_run_id`.

ALTER TABLE `source_call_log` ADD COLUMN `agent_run_id` TEXT;

CREATE INDEX IF NOT EXISTS `idx_source_call_log_agent_run_id`
    ON `source_call_log` (`agent_run_id`);
