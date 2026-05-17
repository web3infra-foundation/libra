CREATE TABLE IF NOT EXISTS `automation_log` (
    `id` TEXT PRIMARY KEY,
    `rule_id` TEXT NOT NULL,
    `trigger_kind` TEXT NOT NULL,
    `action_kind` TEXT NOT NULL,
    `status` TEXT NOT NULL,
    `message` TEXT NOT NULL,
    `started_at` TEXT NOT NULL,
    `finished_at` TEXT NOT NULL,
    `details_json` TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS `idx_automation_log_finished_at`
    ON `automation_log` (`finished_at`);
CREATE INDEX IF NOT EXISTS `idx_automation_log_rule_id`
    ON `automation_log` (`rule_id`);
