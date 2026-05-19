-- Libra Publish — migration 0004.
--
-- Codex pass-13 P3: refs pagination orders rows by
-- `(ref_type, short_name)` with a keyset cursor on the same pair,
-- but the existing indexes on `publish_refs` are
-- `(site_id, short_name)` and `(site_id, revision_oid)`. Without a
-- composite index that matches the ORDER BY column order, SQLite
-- has to materialise + sort the row set before applying LIMIT,
-- which is why the pass-12 SQL push was only a partial speed-up.
--
-- Add `(site_id, ref_type, short_name)` so the planner can range-
-- scan the index range directly. Idempotent (`CREATE INDEX IF NOT
-- EXISTS`) and additive: no DDL beyond a single index creation.

CREATE INDEX IF NOT EXISTS idx_publish_refs_site_type_short
    ON publish_refs (site_id, ref_type, short_name);
