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
