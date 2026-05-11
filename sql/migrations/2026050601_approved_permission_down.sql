-- Rollback for 2026050601_approved_permission.
--
-- Drops the index first (so SQLite does not complain about a dangling
-- index reference), then the table itself. Idempotent: `IF EXISTS`
-- makes a partial-apply rollback safe.

DROP INDEX IF EXISTS `idx_approved_permission_project`;
DROP TABLE IF EXISTS `approved_permission`;
