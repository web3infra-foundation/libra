-- Rename the external-agent capture ref from the legacy `agent-traces`
-- short name to the single-word `traces` (refs/libra/traces).
--
-- Idempotent + conflict-safe:
--   * Only the local capture branch row (kind = 'Branch', remote IS NULL) is
--     touched; remote-tracking rows and tags are left alone.
--   * The rename is skipped when a `traces` branch row already exists (a fresh
--     repo created after the rename, or a re-run), so the partial UNIQUE index
--     `idx_name_kind` (name, kind WHERE remote IS NULL) can never collide.
--   * Reflog rows recorded under the old name are carried over so the
--     pre-rename history stays attached to `traces`.
UPDATE `reference`
SET `name` = 'traces'
WHERE `name` = 'agent-traces'
  AND `kind` = 'Branch'
  AND `remote` IS NULL
  AND NOT EXISTS (
    SELECT 1 FROM `reference` AS existing
    WHERE existing.`name` = 'traces'
      AND existing.`kind` = 'Branch'
      AND existing.`remote` IS NULL
  );

UPDATE `reflog` SET `ref_name` = 'traces' WHERE `ref_name` = 'agent-traces';
UPDATE `reflog`
SET `ref_name` = 'refs/heads/traces'
WHERE `ref_name` = 'refs/heads/agent-traces';
