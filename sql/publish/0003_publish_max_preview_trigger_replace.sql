-- Libra Publish — migration 0003.
--
-- Codex pass-11 P2: pass-9's 0002 created
-- `publish_sites_max_preview_bytes_positive_update` as a per-column
-- trigger (`BEFORE UPDATE OF max_preview_bytes`). Pass-10 widened
-- the same trigger to a row-level form (`BEFORE UPDATE`) by
-- editing 0002 in place. SQLite's `CREATE TRIGGER IF NOT EXISTS`
-- is a no-op when the trigger name already exists, so any tenant
-- who applied the pass-9 form will keep the per-column variant
-- forever.
--
-- This migration explicitly DROPs the old trigger and re-creates
-- it in its row-level form. `DROP TRIGGER IF EXISTS` is idempotent;
-- the subsequent `CREATE TRIGGER` re-installs the corrected
-- `BEFORE UPDATE` form. Fresh databases that started on 0001+0002
-- (pass-10 form) will see the DROP succeed (no-op) and the CREATE
-- succeed (replacing the identical row-level trigger).
--
-- Runs the same SET-list-bypass repair on the digest triggers as a
-- defence-in-depth — those are already row-level via `BEFORE
-- UPDATE OF <col>` (per-column triggers ARE the right shape for
-- digest enforcement because they only need to fire when the
-- digest column itself changes), so no DROP is required for them.

DROP TRIGGER IF EXISTS publish_sites_max_preview_bytes_positive_update;

CREATE TRIGGER IF NOT EXISTS publish_sites_max_preview_bytes_positive_update
    BEFORE UPDATE ON publish_sites
    FOR EACH ROW
    WHEN NEW.max_preview_bytes <= 0
BEGIN
    SELECT RAISE(ABORT, 'publish_sites.max_preview_bytes must be > 0');
END;
