-- Libra Publish — D1 schema source of truth.
--
-- Per docs/development/commands/publish.md, this file is the single source for the
-- D1 schema. The Worker copy at worker/migrations/0001_publish.sql MUST be
-- byte-for-byte identical; the publish_schema_contract test enforces the
-- equality. The schema is read by:
--
--   * `D1Client::ensure_publish_schema()` via include_str! (CLI side)
--   * `wrangler d1 migrations apply` at deploy time (Worker side)
--
-- Worker runtime never migrates the schema; reads only.
--
-- Migration convention (Codex pass-3 P3):
--   * This file is `0001_publish.sql`. Once landed it is IMMUTABLE in
--     production — additive-only edits during pass-N review iterations
--     before a release tag are allowed (this file is still pre-cut).
--   * Future schema changes go into `0002_<topic>.sql`, `0003_…`, etc.
--     Each migration MUST be additive (CREATE TABLE … IF NOT EXISTS,
--     ALTER TABLE … ADD COLUMN with a default, CREATE INDEX … IF NOT
--     EXISTS) so reapplying on a fresh shard yields the same end state
--     as applying the chain incrementally.
--   * `D1Client::ensure_publish_schema()` will apply the migrations in
--     order; do not rely on cross-migration ordering of statements
--     within a single file beyond what SQLite's grammar guarantees.
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
    refs_generation INTEGER NOT NULL DEFAULT 0
        CHECK (refs_generation >= 0),
    max_preview_bytes INTEGER NOT NULL CHECK (max_preview_bytes >= 0),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (clone_domain, slug),
    -- Stable URL invariant (Codex pass-3 P2): the design doc
    -- specifies `(clone_domain, repo_id)` is the rename-proof
    -- entry point. Two sites under one clone domain MUST NOT
    -- share a repo_id, otherwise `repo/<repo_id>` lookups become
    -- ambiguous.
    UNIQUE (clone_domain, repo_id),
    -- Composite FKs (Codex pass-2 P2): a site's `default_ref` must
    -- name a real ref of this site, and `latest_revision_oid` must
    -- name a real revision. Both columns are nullable, so the
    -- chicken-and-egg insert order works:
    --   1) INSERT publish_sites with both NULL.
    --   2) INSERT publish_sync_runs / publish_revisions / publish_refs.
    --   3) UPDATE publish_sites with the resolved values.
    -- Default ON DELETE/UPDATE is NO ACTION; this blocks deleting a
    -- ref/revision that is still referenced as default/latest.
    FOREIGN KEY (site_id, default_ref)
        REFERENCES publish_refs (site_id, ref_name),
    FOREIGN KEY (site_id, latest_revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid)
);

CREATE INDEX IF NOT EXISTS idx_publish_sites_repo_id
    ON publish_sites (repo_id);

CREATE TABLE IF NOT EXISTS publish_revisions (
    site_id TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('syncing', 'published', 'failed')),
    code_manifest_key TEXT,
    ai_index_key TEXT,
    file_count INTEGER NOT NULL DEFAULT 0 CHECK (file_count >= 0),
    ai_object_count INTEGER NOT NULL DEFAULT 0 CHECK (ai_object_count >= 0),
    ai_bundle_count INTEGER NOT NULL DEFAULT 0 CHECK (ai_bundle_count >= 0),
    redaction_mode TEXT NOT NULL CHECK (redaction_mode IN ('default', 'strict')),
    redaction_rules_version TEXT NOT NULL,
    sync_run_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- Published-revision manifest invariant (Codex pass-3 P2 +
    -- pass-4 P2): a row whose status is `published` MUST carry a
    -- non-empty code manifest key. `ai_index_key` (when present)
    -- must also be non-empty. `ai_index_key` may legitimately be
    -- NULL when a snapshot has no AI objects (rare but allowed).
    CHECK (
        status != 'published'
        OR (code_manifest_key IS NOT NULL AND length(code_manifest_key) > 0)
    ),
    CHECK (ai_index_key IS NULL OR length(ai_index_key) > 0),
    PRIMARY KEY (site_id, revision_oid),
    FOREIGN KEY (site_id) REFERENCES publish_sites (site_id) ON DELETE CASCADE,
    -- Codex pass-2 P2: the sync_run that produced this revision must
    -- exist. Default NO ACTION blocks deleting a sync_run that is
    -- still referenced.
    FOREIGN KEY (sync_run_id) REFERENCES publish_sync_runs (sync_run_id)
);

CREATE INDEX IF NOT EXISTS idx_publish_revisions_status
    ON publish_revisions (site_id, status);

CREATE TABLE IF NOT EXISTS publish_refs (
    site_id TEXT NOT NULL,
    ref_name TEXT NOT NULL,
    ref_type TEXT NOT NULL,
    short_name TEXT NOT NULL,
    target_oid TEXT NOT NULL,
    revision_oid TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    sync_run_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    -- Composite CHECK (Codex pass-2 P2): ref_type and ref_name MUST
    -- agree. Independent CHECKs would let `ref_type='branch'` carry
    -- `ref_name='refs/tags/v1'` and vice versa, breaking the picker
    -- and the tag target-OID invariant tests.
    CHECK (
        (ref_type = 'branch' AND ref_name LIKE 'refs/heads/%')
        OR (ref_type = 'tag' AND ref_name LIKE 'refs/tags/%')
    ),
    PRIMARY KEY (site_id, ref_name),
    FOREIGN KEY (site_id) REFERENCES publish_sites (site_id) ON DELETE CASCADE,
    FOREIGN KEY (site_id, revision_oid)
        REFERENCES publish_revisions (site_id, revision_oid) ON DELETE CASCADE,
    FOREIGN KEY (sync_run_id) REFERENCES publish_sync_runs (sync_run_id)
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
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    language TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1,
    -- File-content invariant (Codex pass-3 P2 + pass-4 P2): only
    -- `text` rows carry contents in R2 and a sha256 digest;
    -- `binary`, `too_large` and `ignored` rows record metadata
    -- only and MUST NOT carry content pointers (otherwise stale
    -- R2 keys leak). content_sha256 is exactly 64 hex chars when
    -- present; r2_key must be non-empty when present.
    CHECK (
        (display_mode = 'text'
            AND content_sha256 IS NOT NULL
            AND length(content_sha256) = 64
            AND content_sha256 NOT GLOB '*[^0-9a-f]*'
            AND r2_key IS NOT NULL
            AND length(r2_key) > 0)
        OR (display_mode IN ('binary', 'too_large', 'ignored')
            AND content_sha256 IS NULL
            AND r2_key IS NULL)
    ),
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
    r2_key TEXT NOT NULL CHECK (length(r2_key) > 0),
    redaction_mode TEXT NOT NULL CHECK (redaction_mode IN ('default', 'strict')),
    -- sha256 hex is exactly 64 chars; pin shape so a truncated hash
    -- never enters the index (Codex pass-4 P2).
    -- Codex pass-5 P2: pin lowercase hex shape on every digest column.
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
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
    bundle_key TEXT NOT NULL CHECK (length(bundle_key) > 0),
    -- Bundle integrity (Codex pass-4 P2): the snapshot builder
    -- writes the canonical bundle JSON to R2 and records its
    -- sha256 here. The Worker MUST refuse to serve a bundle
    -- whose R2 body does not hash to this digest, so a stale or
    -- tampered R2 write cannot bypass the redaction policy
    -- recorded alongside the index. The column is exactly 64
    -- hex chars (lowercase) so a truncated digest never enters
    -- the index.
    -- Codex pass-5 P2: pin lowercase hex shape so an upper-case or
    -- non-hex digest never enters the index. SQLite's GLOB is the
    -- portable way to express a character-class restriction.
    bundle_sha256 TEXT NOT NULL CHECK (
        length(bundle_sha256) = 64
        AND bundle_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    object_count INTEGER NOT NULL DEFAULT 0 CHECK (object_count >= 0),
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
    refs_count INTEGER NOT NULL DEFAULT 0 CHECK (refs_count >= 0),
    revision_count INTEGER NOT NULL DEFAULT 0 CHECK (revision_count >= 0),
    file_count INTEGER NOT NULL DEFAULT 0 CHECK (file_count >= 0),
    ai_object_count INTEGER NOT NULL DEFAULT 0 CHECK (ai_object_count >= 0),
    ai_bundle_count INTEGER NOT NULL DEFAULT 0 CHECK (ai_bundle_count >= 0),
    -- warnings_json invariant (Codex pass-3 P2): the column stores
    -- a JSON array of human-readable warning strings. A malformed
    -- write would poison every status reader.
    warnings_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(warnings_json) AND json_type(warnings_json) = 'array'),
    error_message TEXT,
    cli_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    -- Sync-run state machine (Codex pass-2 P2): a `succeeded` or
    -- `failed` row MUST have `finished_at`; a `running` row MUST
    -- NOT. A `failed` row MUST carry a non-empty `error_message`;
    -- non-failed rows MUST NOT.
    CHECK (
        (status = 'running' AND finished_at IS NULL AND error_message IS NULL)
        OR (status = 'succeeded' AND finished_at IS NOT NULL AND error_message IS NULL)
        OR (status = 'failed' AND finished_at IS NOT NULL
            AND error_message IS NOT NULL AND length(error_message) > 0)
    ),
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
           schema_version,
           created_at,
           updated_at
    FROM publish_revisions
    WHERE status = 'published';
