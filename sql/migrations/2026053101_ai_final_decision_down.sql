-- Rollback for 2026053101_ai_final_decision.sql.
DROP INDEX IF EXISTS idx_ai_final_decision_latest;
DROP INDEX IF EXISTS idx_ai_final_decision_thread_created;
DROP TABLE IF EXISTS `ai_final_decision`;
