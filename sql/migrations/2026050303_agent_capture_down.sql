-- Rollback for 2026050303_agent_capture.
--
-- Drops indexes before tables (sqlite tolerates the reverse order, but the
-- explicit sequencing keeps the intent legible). Children before parents
-- because `agent_checkpoint.session_id` has a foreign key into `agent_session`.

DROP INDEX IF EXISTS `idx_agent_checkpoint_scope`;
DROP INDEX IF EXISTS `idx_agent_checkpoint_session`;
DROP TABLE IF EXISTS `agent_checkpoint`;

DROP INDEX IF EXISTS `idx_agent_session_thread`;
DROP INDEX IF EXISTS `idx_agent_session_active`;
DROP INDEX IF EXISTS `idx_agent_session_provider`;
DROP TABLE IF EXISTS `agent_session`;
