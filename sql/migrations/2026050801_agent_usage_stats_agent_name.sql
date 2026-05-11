-- 2026050801_agent_usage_stats_agent_name: OC-Phase 5 P5.2.
--
-- Adds the `agent_name` dimension to `agent_usage_stats` so the
-- multi-agent runtime (OC-Phase 3 dispatcher + OC-Phase 5 declarative
-- config) can attribute spend and tokens to a specific agent profile
-- (`planner` / `explorer` / `reviewer` / …) on top of the existing
-- (provider, model) aggregation.
--
-- Additive migration: a NULL value is the equivalence-class for "no
-- agent context recorded" — i.e. the single-agent legacy path. Old
-- rows therefore stay queryable through the existing
-- `(provider, model)` indexes; new code paths populate the column.
--
-- Index: `(agent_name, provider, model)` matches the primary
-- aggregation grain the TUI `/usage --by=agent` surface (P5.4) walks,
-- so the planner can satisfy the GROUP BY without scanning the table.
-- The leading column is `agent_name` because the most common filter
-- the operator types is "show me what `explorer` spent".

ALTER TABLE `agent_usage_stats` ADD COLUMN `agent_name` TEXT;

CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_agent_name_provider_model`
    ON `agent_usage_stats` (`agent_name`, `provider`, `model`);
