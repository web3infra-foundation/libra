-- Libra Publish — D1 schema source of truth.
--
-- Per docs/improvement/publish.md, this file is the single source for the
-- D1 schema. The Worker copy at worker/migrations/0001_publish.sql MUST be
-- byte-for-byte identical; the publish_schema_contract test enforces the
-- equality. The schema is read by:
--
--   * `D1Client::ensure_publish_schema()` via include_str! (CLI side)
--   * `wrangler d1 migrations apply` at deploy time (Worker side)
--
-- Worker runtime never migrates the schema; reads only.
--
-- Naming + invariants:
--   * Every row carries `site_id` so a Worker query can scope to one site
--     without cross-site leakage.
--   * `publish_sites.status` is one of `active`, `disabled` (v1 set);
--     disabled sites surface 410 from the Worker API and skip R2 reads.
--   * `publish_refs.ref_name` MUST be a full ref (`refs/heads/<short>` or
--     `refs/tags/<short>`); CLI/UI render the short name but the table
--     stores the full path so a branch and tag with the same short name
--     stay distinct.
--   * `publish_revisions` is keyed on `(site_id, revision_oid)` so multi-
--     branch/tag references to the same commit share one snapshot.
--   * `publish_ai_objects.layer` is one of `snapshot`, `event`,
--     `projection`; redaction-mode filters live in the JSON payload, not
--     in the schema.
--   * Timestamps are RFC3339 UTC TEXT (matches the rest of the Libra D1
--     schema; SQLite has no native datetime type).

CREATE TABLE IF NOT EXISTS publish_sites (
    site_id TEXT NOT NULL PRIMARY KEY,
    repo_id TEXT NOT NULL,
    clone_domain TEXT NOT NULL,
    slug TEXT NOT NULL,
    display_origin TEXT NOT NULL,
    name TEXT NOT NULL,
    visibility TEXT NOT NULL CHECK (visibility IN ('public', 'private')),
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    worker_name TEXT NOT NULL,
    default_ref TEXT
        CHECK (default_ref IS NULL
               OR default_ref LIKE 'refs/heads/%'
               OR default_ref LIKE 'refs/tags/%'),
    latest_revision_oid TEXT,
    refs_generation INTEGER NOT NULL DEFAULT 0,
    max_preview_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (clone_domain, slug)
);

CREATE INDEX IF NOT EXISTS idx_publish_sites_repo_id
    ON publish_sites (repo_id);

CREATE TABLE IF NOT EXISTS publish_revisions (
    site_id TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('syncing', 'published', 'failed')),
    code_manifest_key TEXT,
    ai_index_key TEXT,
    file_count INTEGER NOT NULL DEFAULT 0,
    ai_object_count INTEGER NOT NULL DEFAULT 0,
    ai_bundle_count INTEGER NOT NULL DEFAULT 0,
    redaction_mode TEXT NOT NULL CHECK (redaction_mode IN ('default', 'strict')),
    redaction_rules_version TEXT NOT NULL,
    sync_run_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (site_id, revision_oid),
    FOREIGN KEY (site_id) REFERENCES publish_sites (site_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_revisions_status
    ON publish_revisions (site_id, status);

CREATE TABLE IF NOT EXISTS publish_refs (
    site_id TEXT NOT NULL,
    ref_name TEXT NOT NULL CHECK (
        ref_name LIKE 'refs/heads/%' OR ref_name LIKE 'refs/tags/%'
    ),
    ref_type TEXT NOT NULL CHECK (ref_type IN ('branch', 'tag')),
    short_name TEXT NOT NULL,
    target_oid TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    sync_run_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (site_id, ref_name),
    FOREIGN KEY (site_id) REFERENCES publish_sites (site_id) ON DELETE CASCADE,
    FOREIGN KEY (site_id, revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_refs_short_name
    ON publish_refs (site_id, short_name);

CREATE INDEX IF NOT EXISTS idx_publish_refs_revision_oid
    ON publish_refs (site_id, revision_oid);

-- At most one default ref per site. Codex pass-1 P2: without this
-- constraint two rows could carry `is_default = 1` and the Worker
-- API would non-deterministically pick a row.
CREATE UNIQUE INDEX IF NOT EXISTS uniq_publish_refs_default_per_site
    ON publish_refs (site_id) WHERE is_default = 1;

CREATE TABLE IF NOT EXISTS publish_files (
    site_id TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    path TEXT NOT NULL,
    display_mode TEXT NOT NULL CHECK (
        display_mode IN ('text', 'binary', 'too_large', 'ignored')
    ),
    content_sha256 TEXT,
    r2_key TEXT,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    language TEXT,
    PRIMARY KEY (site_id, revision_oid, path),
    FOREIGN KEY (site_id, revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_files_display_mode
    ON publish_files (site_id, revision_oid, display_mode);

CREATE TABLE IF NOT EXISTS publish_ai_objects (
    site_id TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    layer TEXT NOT NULL CHECK (layer IN ('snapshot', 'event', 'projection')),
    r2_key TEXT NOT NULL,
    redaction_mode TEXT NOT NULL CHECK (redaction_mode IN ('default', 'strict')),
    payload_sha256 TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (site_id, revision_oid, object_type, object_id),
    FOREIGN KEY (site_id, revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_ai_objects_layer
    ON publish_ai_objects (site_id, revision_oid, layer);

CREATE INDEX IF NOT EXISTS idx_publish_ai_objects_type
    ON publish_ai_objects (site_id, revision_oid, object_type);

CREATE TABLE IF NOT EXISTS publish_ai_versions (
    site_id TEXT NOT NULL,
    ai_version_id TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    bundle_key TEXT NOT NULL,
    object_count INTEGER NOT NULL DEFAULT 0,
    redaction_mode TEXT NOT NULL CHECK (redaction_mode IN ('default', 'strict')),
    redaction_rules_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (site_id, ai_version_id),
    FOREIGN KEY (site_id, revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_ai_versions_revision
    ON publish_ai_versions (site_id, revision_oid);

CREATE TABLE IF NOT EXISTS publish_sync_runs (
    sync_run_id TEXT NOT NULL PRIMARY KEY,
    site_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    refs_count INTEGER NOT NULL DEFAULT 0,
    revision_count INTEGER NOT NULL DEFAULT 0,
    file_count INTEGER NOT NULL DEFAULT 0,
    ai_object_count INTEGER NOT NULL DEFAULT 0,
    ai_bundle_count INTEGER NOT NULL DEFAULT 0,
    warnings_json TEXT NOT NULL DEFAULT '[]',
    error_message TEXT,
    cli_version TEXT NOT NULL,
    FOREIGN KEY (site_id) REFERENCES publish_sites (site_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_publish_sync_runs_site
    ON publish_sync_runs (site_id, started_at);

-- View: only revisions in `published` status. The Worker API SHOULD
-- read through this view (or always include `status = 'published'`
-- in joins) so a partially synchronised revision (`syncing`) cannot
-- leak into refs lookups. Codex pass-1 P2: Worker authors that
-- forget the status filter would otherwise expose mid-sync state.
CREATE VIEW IF NOT EXISTS publish_revisions_published AS
    SELECT site_id,
           revision_oid,
           code_manifest_key,
           ai_index_key,
           file_count,
           ai_object_count,
           ai_bundle_count,
           redaction_mode,
           redaction_rules_version,
           sync_run_id,
           created_at,
           updated_at
    FROM publish_revisions
    WHERE status = 'published';
