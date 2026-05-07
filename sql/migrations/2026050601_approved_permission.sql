-- 2026050601_approved_permission
--
-- OC-Phase 2 P2.5: persistent storage for the `Always` reply of the
-- three-state Permission protocol (see docs/improvement/opencode.md
-- "Permission Ruleset 与 Approval 反馈协议").
--
-- When a user replies "Always" to a permission prompt, the runtime appends
-- one row per approved `(permission, pattern)` pair. On the next session
-- the rows are loaded into an `ApprovedRuleset` projection that is merged
-- into the in-memory `PermissionRuleset` ahead of the in-process session
-- ruleset, so the cached approval survives a process restart.
--
-- Versioning: this table sits at `2026050601`, strictly later than
-- `2026050501_agent_checkpoint_parent_nullable` (the previous built-in
-- version) and well after entire.md's `2026050303_agent_capture` so the
-- entire.md ordering invariant is preserved.

CREATE TABLE IF NOT EXISTS `approved_permission` (
    `project_id`  TEXT NOT NULL,
    `permission`  TEXT NOT NULL,
    `pattern`     TEXT NOT NULL,
    `created_at`  INTEGER NOT NULL,
    PRIMARY KEY (`project_id`, `permission`, `pattern`)
);

-- Lookup index: a session loads every row for the active project and
-- builds the `ApprovedRuleset` from it. The primary key already covers
-- this prefix-search shape but we materialize the explicit index so
-- `EXPLAIN QUERY PLAN` makes the intent obvious to future maintainers.
CREATE INDEX IF NOT EXISTS `idx_approved_permission_project`
    ON `approved_permission` (`project_id`, `created_at`);
