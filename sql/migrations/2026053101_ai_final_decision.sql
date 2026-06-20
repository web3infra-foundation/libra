-- Phase 4 completion: the formal final `Decision` artifact.
--
-- Closes the ValidationReport -> RiskScoreBreakdown -> DecisionProposal ->
-- **Decision** chain described in docs/development/commands/agent.md (Implementation
-- Phase 4). A DecisionProposal carries a `proposed_verdict` plus a routing
-- decision; when that route is auto-accept (no human gate required) the
-- runtime finalises it into a `Decision` row recording the resolved verdict.
-- Human-gated routes (HumanReview / RequestChanges) finalise later through the
-- CEX-S2-13 human-gated merge flow.
--
-- Mirrors the `ai_decision_proposal` shape (per-thread latest-pointer +
-- created-at index + ON DELETE CASCADE to ai_thread) so the same
-- write_latest_with_session_mirror pattern applies.
CREATE TABLE IF NOT EXISTS `ai_final_decision` (
    `decision_id` TEXT PRIMARY KEY,
    `thread_id` TEXT NOT NULL,
    `decision_proposal_id` TEXT,
    `validation_report_id` TEXT,
    `policy_version` TEXT NOT NULL,
    `verdict` TEXT NOT NULL,
    `stale` INTEGER NOT NULL DEFAULT 0 CHECK (`stale` IN (0, 1)),
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `summary_json` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_final_decision_thread_created
    ON `ai_final_decision`(`thread_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_final_decision_latest
    ON `ai_final_decision`(`thread_id`) WHERE `is_latest` = 1;
