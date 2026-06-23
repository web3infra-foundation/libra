-- Reverse 2026062301: restore the legacy `agent-traces` capture-ref name.
-- Mirrors the up migration's conflict-safety: only rename back when no
-- `agent-traces` branch row already exists.
UPDATE `reference`
SET `name` = 'agent-traces'
WHERE `name` = 'traces'
  AND `kind` = 'Branch'
  AND `remote` IS NULL
  AND NOT EXISTS (
    SELECT 1 FROM `reference` AS existing
    WHERE existing.`name` = 'agent-traces'
      AND existing.`kind` = 'Branch'
      AND existing.`remote` IS NULL
  );

UPDATE `reflog` SET `ref_name` = 'agent-traces' WHERE `ref_name` = 'traces';
UPDATE `reflog`
SET `ref_name` = 'refs/heads/agent-traces'
WHERE `ref_name` = 'refs/heads/traces';
