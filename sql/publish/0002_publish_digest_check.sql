-- Libra Publish — migration 0002.
--
-- Codex pass-6 P2: the `NOT GLOB '*[^0-9a-f]*'` lowercase-hex
-- constraints on `payload_sha256`, `bundle_sha256` and
-- `content_sha256` were added directly to the `0001_publish.sql`
-- definition during the pre-release review iteration. SQLite
-- evaluates `CREATE TABLE IF NOT EXISTS` as a no-op when the table
-- already exists, so any D1 instance that applied the previous
-- 0001 will not see the tightened constraint. This migration adds
-- a CHECK-equivalent guard via INSERT/UPDATE triggers so existing
-- tenants get the same enforcement, and is a no-op for fresh
-- databases that already include the column-level CHECK.
--
-- Triggers fire BEFORE INSERT and BEFORE UPDATE; the
-- `RAISE(ABORT, ...)` halts the statement with a SQLITE_CONSTRAINT
-- error indistinguishable from the column-level CHECK so the Worker
-- error envelope stays uniform.

-- Codex pass-9 P2: enforce `max_preview_bytes > 0` at the trigger
-- layer. The 0001 schema CHECKs only `>= 0`, but at the publish
-- semantic level a zero cap publishes no previews and is treated as
-- a misuse — the CLI rejects it via clap, and this trigger pins the
-- invariant on databases that already applied 0001.
CREATE TRIGGER IF NOT EXISTS publish_sites_max_preview_bytes_positive_insert
    BEFORE INSERT ON publish_sites
    FOR EACH ROW
    WHEN NEW.max_preview_bytes <= 0
BEGIN
    SELECT RAISE(ABORT, 'publish_sites.max_preview_bytes must be > 0');
END;

-- Codex pass-10 P2: row-level UPDATE trigger (no `OF` clause) so an
-- update statement that does NOT touch `max_preview_bytes` still
-- re-validates the invariant. Per-column triggers (`BEFORE UPDATE OF
-- max_preview_bytes`) skip statements that omit the column from the
-- SET list, which would let a row violating the invariant be
-- modified without the trigger firing.
CREATE TRIGGER IF NOT EXISTS publish_sites_max_preview_bytes_positive_update
    BEFORE UPDATE ON publish_sites
    FOR EACH ROW
    WHEN NEW.max_preview_bytes <= 0
BEGIN
    SELECT RAISE(ABORT, 'publish_sites.max_preview_bytes must be > 0');
END;

CREATE TRIGGER IF NOT EXISTS publish_files_content_sha256_lowercase_hex_insert
    BEFORE INSERT ON publish_files
    FOR EACH ROW
    WHEN NEW.content_sha256 IS NOT NULL
         AND (length(NEW.content_sha256) != 64
              OR NEW.content_sha256 GLOB '*[^0-9a-f]*')
BEGIN
    SELECT RAISE(ABORT, 'publish_files.content_sha256 must be lowercase 64-char hex');
END;

CREATE TRIGGER IF NOT EXISTS publish_files_content_sha256_lowercase_hex_update
    BEFORE UPDATE OF content_sha256 ON publish_files
    FOR EACH ROW
    WHEN NEW.content_sha256 IS NOT NULL
         AND (length(NEW.content_sha256) != 64
              OR NEW.content_sha256 GLOB '*[^0-9a-f]*')
BEGIN
    SELECT RAISE(ABORT, 'publish_files.content_sha256 must be lowercase 64-char hex');
END;

CREATE TRIGGER IF NOT EXISTS publish_ai_objects_payload_sha256_lowercase_hex_insert
    BEFORE INSERT ON publish_ai_objects
    FOR EACH ROW
    WHEN length(NEW.payload_sha256) != 64
         OR NEW.payload_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'publish_ai_objects.payload_sha256 must be lowercase 64-char hex');
END;

CREATE TRIGGER IF NOT EXISTS publish_ai_objects_payload_sha256_lowercase_hex_update
    BEFORE UPDATE OF payload_sha256 ON publish_ai_objects
    FOR EACH ROW
    WHEN length(NEW.payload_sha256) != 64
         OR NEW.payload_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'publish_ai_objects.payload_sha256 must be lowercase 64-char hex');
END;

CREATE TRIGGER IF NOT EXISTS publish_ai_versions_bundle_sha256_lowercase_hex_insert
    BEFORE INSERT ON publish_ai_versions
    FOR EACH ROW
    WHEN length(NEW.bundle_sha256) != 64
         OR NEW.bundle_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'publish_ai_versions.bundle_sha256 must be lowercase 64-char hex');
END;

CREATE TRIGGER IF NOT EXISTS publish_ai_versions_bundle_sha256_lowercase_hex_update
    BEFORE UPDATE OF bundle_sha256 ON publish_ai_versions
    FOR EACH ROW
    WHEN length(NEW.bundle_sha256) != 64
         OR NEW.bundle_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'publish_ai_versions.bundle_sha256 must be lowercase 64-char hex');
END;
