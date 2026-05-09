import "server-only";
import { PublishApiError, badRequest, notFound } from "./errors";

/* ------------------------------------------------------------------ *
 *  Row types — shape of D1 results.
 *
 *  All shapes here are NULLs-as-`null` (D1 returns SQL NULL as null).
 *  Every query in this module uses prepared statements with positional
 *  `?` binds; URL parameters never enter SQL strings.
 * ------------------------------------------------------------------ */

export type SiteRow = {
  readonly site_id: string;
  readonly repo_id: string;
  readonly clone_domain: string;
  readonly slug: string;
  readonly display_origin: string;
  readonly name: string;
  readonly visibility: "public" | "private";
  readonly status: "active" | "disabled";
  readonly worker_name: string;
  readonly default_ref: string | null;
  readonly latest_revision_oid: string | null;
  readonly refs_generation: number;
  readonly max_preview_bytes: number;
  readonly schema_version: number;
  readonly created_at: string;
  readonly updated_at: string;
};

export type RevisionRow = {
  readonly site_id: string;
  readonly revision_oid: string;
  readonly status: "syncing" | "published" | "failed";
  readonly code_manifest_key: string | null;
  readonly ai_index_key: string | null;
  readonly file_count: number;
  readonly ai_object_count: number;
  readonly ai_bundle_count: number;
  readonly redaction_mode: "default" | "strict";
  readonly redaction_rules_version: string;
  readonly sync_run_id: string;
  readonly schema_version: number;
  readonly created_at: string;
  readonly updated_at: string;
};

export type RefRow = {
  readonly site_id: string;
  readonly ref_name: string;
  readonly ref_type: "branch" | "tag";
  readonly short_name: string;
  readonly target_oid: string;
  readonly revision_oid: string;
  readonly is_default: number;
  readonly sync_run_id: string;
  readonly schema_version: number;
  readonly updated_at: string;
};

export type FileRow = {
  readonly site_id: string;
  readonly revision_oid: string;
  readonly path: string;
  readonly display_mode: "text" | "binary" | "too_large" | "ignored";
  readonly content_sha256: string | null;
  readonly r2_key: string | null;
  readonly size_bytes: number;
  readonly language: string | null;
  readonly schema_version: number;
};

export type AiObjectRow = {
  readonly site_id: string;
  readonly revision_oid: string;
  readonly object_type: string;
  readonly object_id: string;
  readonly layer: "snapshot" | "event" | "projection";
  readonly r2_key: string;
  readonly redaction_mode: "default" | "strict";
  readonly payload_sha256: string;
  readonly schema_version: number;
  readonly created_at: string;
};

export type AiVersionRow = {
  readonly site_id: string;
  readonly ai_version_id: string;
  readonly revision_oid: string;
  readonly bundle_key: string;
  /**
   * Codex pass-4 P2: lowercase hex sha256 of the bundle JSON the
   * Worker reads from `bundle_key`. Verified against the R2 body
   * before responding so a stale or tampered bundle cannot bypass
   * the redaction policy recorded on this row.
   */
  readonly bundle_sha256: string;
  readonly object_count: number;
  readonly redaction_mode: "default" | "strict";
  readonly redaction_rules_version: string;
  readonly schema_version: number;
  readonly created_at: string;
};

export type SyncRunRow = {
  readonly sync_run_id: string;
  readonly site_id: string;
  readonly status: "running" | "succeeded" | "failed";
  readonly started_at: string;
  readonly finished_at: string | null;
  readonly refs_count: number;
  readonly revision_count: number;
  readonly file_count: number;
  readonly ai_object_count: number;
  readonly ai_bundle_count: number;
  readonly warnings_json: string;
  readonly error_message: string | null;
  readonly cli_version: string;
  readonly schema_version: number;
};

/* ------------------------------------------------------------------ *
 *  Site lookup
 * ------------------------------------------------------------------ */

export async function findSiteBySlug(
  db: D1Database,
  cloneDomain: string | null,
  slug: string,
): Promise<SiteRow | null> {
  // Worker host is the implicit cloneDomain when not passed.
  // We bind both as parameters; SQL string is fixed.
  if (cloneDomain) {
    return db
      .prepare(
        `SELECT site_id, repo_id, clone_domain, slug, display_origin,
                name, visibility, status, worker_name, default_ref,
                latest_revision_oid, refs_generation, max_preview_bytes,
                schema_version, created_at, updated_at
         FROM publish_sites
         WHERE clone_domain = ? AND slug = ?`,
      )
      .bind(cloneDomain, slug)
      .first<SiteRow>();
  }
  return db
    .prepare(
      `SELECT site_id, repo_id, clone_domain, slug, display_origin,
              name, visibility, status, worker_name, default_ref,
              latest_revision_oid, refs_generation, max_preview_bytes,
              schema_version, created_at, updated_at
       FROM publish_sites
       WHERE slug = ?`,
    )
    .bind(slug)
    .first<SiteRow>();
}

export async function findSiteByRepoId(
  db: D1Database,
  cloneDomain: string,
  repoId: string,
): Promise<SiteRow | null> {
  return db
    .prepare(
      `SELECT site_id, repo_id, clone_domain, slug, display_origin,
              name, visibility, status, worker_name, default_ref,
              latest_revision_oid, refs_generation, max_preview_bytes,
              schema_version, created_at, updated_at
       FROM publish_sites
       WHERE clone_domain = ? AND repo_id = ?`,
    )
    .bind(cloneDomain, repoId)
    .first<SiteRow>();
}

/* ------------------------------------------------------------------ *
 *  Refs
 * ------------------------------------------------------------------ */

export type ListRefsArgs = {
  readonly type?: "branch" | "tag";
  readonly limit?: number;
  readonly afterRefType?: "branch" | "tag";
  readonly afterShortName?: string;
};

export type ListRefsResult = {
  readonly rows: readonly RefRow[];
  readonly nextCursor?: { readonly refType: "branch" | "tag"; readonly shortName: string };
};

/**
 * List refs with optional `(ref_type, short_name)` keyset cursor.
 *
 * Codex pass-12 P2: pagination is now pushed into the SQL query so
 * D1 never has to materialise the full refs set into the Worker
 * before the route trims it. Without `limit`, callers (page
 * components, the ref picker) get every row at once — which is
 * fine because the picker UI loads them client-side anyway.
 */
export async function listRefs(
  db: D1Database,
  siteId: string,
  args: ListRefsArgs = {},
): Promise<ListRefsResult> {
  const limit = args.limit;
  const fetchN = limit !== undefined ? limit + 1 : undefined;

  let sql = `SELECT site_id, ref_name, ref_type, short_name, target_oid,
                    revision_oid, is_default, sync_run_id, schema_version,
                    updated_at
             FROM publish_refs
             WHERE site_id = ?`;
  const binds: (string | number)[] = [siteId];
  if (args.type) {
    sql += ` AND ref_type = ?`;
    binds.push(args.type);
  }
  if (args.afterRefType !== undefined && args.afterShortName !== undefined) {
    sql += ` AND (ref_type > ? OR (ref_type = ? AND short_name > ?))`;
    binds.push(args.afterRefType, args.afterRefType, args.afterShortName);
  }
  sql += ` ORDER BY ref_type, short_name`;
  if (fetchN !== undefined) {
    sql += ` LIMIT ?`;
    binds.push(fetchN);
  }
  const result = await db.prepare(sql).bind(...binds).all<RefRow>();
  const rows = result.results ?? [];
  if (limit === undefined || rows.length <= limit) return { rows };
  const trimmed = rows.slice(0, limit);
  const last = trimmed[trimmed.length - 1]!;
  return {
    rows: trimmed,
    nextCursor: { refType: last.ref_type, shortName: last.short_name },
  };
}

export async function findRefByFullName(
  db: D1Database,
  siteId: string,
  fullName: string,
): Promise<RefRow | null> {
  return db
    .prepare(
      `SELECT site_id, ref_name, ref_type, short_name, target_oid,
              revision_oid, is_default, sync_run_id, schema_version,
              updated_at
       FROM publish_refs
       WHERE site_id = ? AND ref_name = ?`,
    )
    .bind(siteId, fullName)
    .first<RefRow>();
}

/**
 * Resolve a short ref name (e.g. `main`, `v1.0.0`). If exactly one
 * branch or tag matches, returns it; if both match, raises an
 * AMBIGUOUS_REF (HTTP 409) so callers must qualify with `refs/heads/`
 * or `refs/tags/`.
 */
export async function resolveShortRef(
  db: D1Database,
  siteId: string,
  shortName: string,
): Promise<RefRow> {
  const result = await db
    .prepare(
      `SELECT site_id, ref_name, ref_type, short_name, target_oid,
              revision_oid, is_default, sync_run_id, schema_version,
              updated_at
       FROM publish_refs
       WHERE site_id = ? AND short_name = ?`,
    )
    .bind(siteId, shortName)
    .all<RefRow>();
  const rows = result.results ?? [];
  if (rows.length === 0) {
    throw notFound("REF_NOT_FOUND", `no published ref named '${shortName}'`);
  }
  if (rows.length > 1) {
    // Schema invariant: at most one branch and at most one tag per
    // short_name (different ref_type values). Both present ⇒ ambiguous.
    throw new PublishApiError(
      "AMBIGUOUS_REF",
      409,
      `'${shortName}' matches both a branch and a tag; specify refs/heads/${shortName} or refs/tags/${shortName}`,
    );
  }
  // Non-null assertion is safe because rows.length === 1 here.
  return rows[0]!;
}

export async function findDefaultRef(
  db: D1Database,
  siteId: string,
): Promise<RefRow | null> {
  return db
    .prepare(
      `SELECT site_id, ref_name, ref_type, short_name, target_oid,
              revision_oid, is_default, sync_run_id, schema_version,
              updated_at
       FROM publish_refs
       WHERE site_id = ? AND is_default = 1
       LIMIT 1`,
    )
    .bind(siteId)
    .first<RefRow>();
}

/* ------------------------------------------------------------------ *
 *  Revisions
 * ------------------------------------------------------------------ */

export async function findPublishedRevision(
  db: D1Database,
  siteId: string,
  revisionOid: string,
): Promise<RevisionRow | null> {
  return db
    .prepare(
      `SELECT site_id, revision_oid, status, code_manifest_key,
              ai_index_key, file_count, ai_object_count, ai_bundle_count,
              redaction_mode, redaction_rules_version, sync_run_id,
              schema_version, created_at, updated_at
       FROM publish_revisions
       WHERE site_id = ? AND revision_oid = ? AND status = 'published'`,
    )
    .bind(siteId, revisionOid)
    .first<RevisionRow>();
}

export type ListRevisionsResult = {
  readonly rows: readonly RevisionRow[];
  readonly nextCursor?: string;
};

export async function listPublishedRevisions(
  db: D1Database,
  siteId: string,
  limit: number,
  before?: { readonly createdAt: string; readonly revisionOid: string },
): Promise<ListRevisionsResult> {
  // Keyset pagination: order by (created_at DESC, revision_oid DESC),
  // continue with a cursor over the same tuple.
  const fetchN = limit + 1;
  const result = before
    ? await db
        .prepare(
          `SELECT site_id, revision_oid, status, code_manifest_key,
                  ai_index_key, file_count, ai_object_count, ai_bundle_count,
                  redaction_mode, redaction_rules_version, sync_run_id,
                  schema_version, created_at, updated_at
           FROM publish_revisions
           WHERE site_id = ? AND status = 'published'
             AND (created_at < ?
                  OR (created_at = ? AND revision_oid < ?))
           ORDER BY created_at DESC, revision_oid DESC
           LIMIT ?`,
        )
        .bind(siteId, before.createdAt, before.createdAt, before.revisionOid, fetchN)
        .all<RevisionRow>()
    : await db
        .prepare(
          `SELECT site_id, revision_oid, status, code_manifest_key,
                  ai_index_key, file_count, ai_object_count, ai_bundle_count,
                  redaction_mode, redaction_rules_version, sync_run_id,
                  schema_version, created_at, updated_at
           FROM publish_revisions
           WHERE site_id = ? AND status = 'published'
           ORDER BY created_at DESC, revision_oid DESC
           LIMIT ?`,
        )
        .bind(siteId, fetchN)
        .all<RevisionRow>();
  const rows = result.results ?? [];
  if (rows.length <= limit) return { rows };
  const trimmed = rows.slice(0, limit);
  const last = trimmed[trimmed.length - 1]!;
  return {
    rows: trimmed,
    nextCursor: JSON.stringify({ revision: last.revision_oid, startedAt: last.created_at }),
  };
}

/* ------------------------------------------------------------------ *
 *  Files
 * ------------------------------------------------------------------ */

export type DirEntry = FileRow & { readonly _isDirectory: boolean };

export async function listDirEntries(
  db: D1Database,
  siteId: string,
  revisionOid: string,
  dir: string,
): Promise<readonly DirEntry[]> {
  // Empty `dir` means repo root. We resolve a logical "tree listing"
  // for a directory by scanning every file row whose path starts
  // with `dir + '/'`, then derive the immediate children in JS.
  // `publish_files` only stores leaf rows; directories are
  // synthesised from path prefixes here so the Worker UI can render
  // a normal file-tree without a separate `publish_directories`
  // table. The fetch is bounded by the file cardinality of one
  // revision, itself capped by `max_preview_bytes` and ignore
  // policies on the Rust side.
  const prefix = dir === "" ? "" : `${dir}/`;
  const upper = prefixUpperBound(prefix);
  const result = upper === null
    ? await db
        .prepare(
          `SELECT site_id, revision_oid, path, display_mode,
                  content_sha256, r2_key, size_bytes, language,
                  schema_version
           FROM publish_files
           WHERE site_id = ? AND revision_oid = ?
           ORDER BY path`,
        )
        .bind(siteId, revisionOid)
        .all<FileRow>()
    : await db
        .prepare(
          `SELECT site_id, revision_oid, path, display_mode,
                  content_sha256, r2_key, size_bytes, language,
                  schema_version
           FROM publish_files
           WHERE site_id = ? AND revision_oid = ?
             AND path >= ? AND path < ?
           ORDER BY path`,
        )
        .bind(siteId, revisionOid, prefix, upper)
        .all<FileRow>();
  const rows = result.results ?? [];

  // Synthesize entries: emit each immediate file under `dir`, and
  // emit one `directory` entry per immediate-child directory name.
  const seen = new Map<string, DirEntry>();
  for (const row of rows) {
    const remainder = row.path.slice(prefix.length);
    if (remainder.length === 0) continue;
    const slashAt = remainder.indexOf("/");
    if (slashAt === -1) {
      // Immediate file child.
      seen.set(remainder, { ...row, _isDirectory: false });
      continue;
    }
    const dirName = remainder.slice(0, slashAt);
    if (!seen.has(dirName)) {
      // Aggregate placeholder for the subdirectory. We carry the
      // first child path so callers can construct stable URLs back
      // to it; the surface representation hides that detail.
      seen.set(dirName, {
        site_id: row.site_id,
        revision_oid: row.revision_oid,
        path: prefix + dirName,
        display_mode: "text",
        content_sha256: null,
        r2_key: null,
        size_bytes: 0,
        language: null,
        schema_version: row.schema_version,
        _isDirectory: true,
      });
    }
  }

  // Codex pass-1 P2: distinguish "directory exists but is empty"
  // (impossible in our schema — every revision has at least the
  // top-level commit tree) from "no such directory in this
  // revision". For non-root requests, refuse to silently return an
  // empty listing so clients get a typed FILE_NOT_FOUND instead of
  // a misleading 200/[]. Root listings always succeed (an empty
  // repo legitimately has no entries).
  if (dir !== "" && seen.size === 0) {
    throw notFound("FILE_NOT_FOUND", `path is not part of this revision: ${dir}`);
  }

  // Stable lexicographic order so cache headers / ETags are
  // deterministic across requests.
  return [...seen.values()].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

function prefixUpperBound(prefix: string): string | null {
  if (prefix === "") return null;
  // Increment last code unit to compute exclusive upper bound. We
  // intentionally do not handle UTF-16 surrogate edge cases: paths
  // are validated to be UTF-8 NF on ingest in the Rust side, and
  // SQLite collation here is BINARY.
  const lastIndex = prefix.length - 1;
  const lastCode = prefix.charCodeAt(lastIndex);
  if (lastCode >= 0xfffe) return null;
  return prefix.slice(0, lastIndex) + String.fromCharCode(lastCode + 1);
}

export async function findFileRow(
  db: D1Database,
  siteId: string,
  revisionOid: string,
  path: string,
): Promise<FileRow | null> {
  return db
    .prepare(
      `SELECT site_id, revision_oid, path, display_mode,
              content_sha256, r2_key, size_bytes, language,
              schema_version
       FROM publish_files
       WHERE site_id = ? AND revision_oid = ? AND path = ?`,
    )
    .bind(siteId, revisionOid, path)
    .first<FileRow>();
}

/* ------------------------------------------------------------------ *
 *  AI objects, versions
 * ------------------------------------------------------------------ */

export type ListAiObjectsArgs = {
  readonly siteId: string;
  readonly revisionOid: string;
  readonly objectType?: string;
  readonly layer?: "snapshot" | "event" | "projection";
  readonly limit: number;
  readonly afterObjectType?: string;
  readonly afterObjectId?: string;
};

export type ListAiObjectsResult = {
  readonly rows: readonly AiObjectRow[];
  readonly nextCursor?: string;
};

export async function listAiObjects(
  db: D1Database,
  args: ListAiObjectsArgs,
): Promise<ListAiObjectsResult> {
  const { siteId, revisionOid, objectType, layer, limit, afterObjectType, afterObjectId } = args;
  const fetchN = limit + 1;

  // Build a single prepared statement per filter shape to keep SQL
  // string fixed across requests. Order is deterministic on
  // (object_type, object_id) so cursors compare 2-tuple keysets.
  const baseFields = `SELECT site_id, revision_oid, object_type, object_id,
                             layer, r2_key, redaction_mode, payload_sha256,
                             schema_version, created_at`;
  const baseFrom = `FROM publish_ai_objects WHERE site_id = ? AND revision_oid = ?`;
  const order = `ORDER BY object_type, object_id LIMIT ?`;

  let sql: string;
  let binds: (string | number)[];
  if (objectType && layer) {
    sql = `${baseFields} ${baseFrom} AND object_type = ? AND layer = ?`;
    binds = [siteId, revisionOid, objectType, layer];
  } else if (objectType) {
    sql = `${baseFields} ${baseFrom} AND object_type = ?`;
    binds = [siteId, revisionOid, objectType];
  } else if (layer) {
    sql = `${baseFields} ${baseFrom} AND layer = ?`;
    binds = [siteId, revisionOid, layer];
  } else {
    sql = `${baseFields} ${baseFrom}`;
    binds = [siteId, revisionOid];
  }
  if (afterObjectType !== undefined && afterObjectId !== undefined) {
    sql += ` AND (object_type > ? OR (object_type = ? AND object_id > ?))`;
    binds.push(afterObjectType, afterObjectType, afterObjectId);
  }
  sql += ` ${order}`;
  binds.push(fetchN);

  const result = await db.prepare(sql).bind(...binds).all<AiObjectRow>();
  const rows = result.results ?? [];
  if (rows.length <= limit) return { rows };
  const trimmed = rows.slice(0, limit);
  const last = trimmed[trimmed.length - 1]!;
  return {
    rows: trimmed,
    nextCursor: JSON.stringify({
      objectType: last.object_type,
      objectId: last.object_id,
      revision: last.revision_oid,
    }),
  };
}

export async function findAiObject(
  db: D1Database,
  siteId: string,
  revisionOid: string,
  objectType: string,
  objectId: string,
): Promise<AiObjectRow | null> {
  return db
    .prepare(
      `SELECT site_id, revision_oid, object_type, object_id,
              layer, r2_key, redaction_mode, payload_sha256,
              schema_version, created_at
       FROM publish_ai_objects
       WHERE site_id = ? AND revision_oid = ?
         AND object_type = ? AND object_id = ?`,
    )
    .bind(siteId, revisionOid, objectType, objectId)
    .first<AiObjectRow>();
}

export async function listAiVersions(
  db: D1Database,
  siteId: string,
  revisionOid: string,
  limit: number,
  afterId?: string,
): Promise<{ readonly rows: readonly AiVersionRow[]; readonly nextCursor?: string }> {
  const fetchN = limit + 1;
  const result = afterId
    ? await db
        .prepare(
          `SELECT site_id, ai_version_id, revision_oid, bundle_key,
                  bundle_sha256, object_count, redaction_mode,
                  redaction_rules_version, schema_version, created_at
           FROM publish_ai_versions
           WHERE site_id = ? AND revision_oid = ? AND ai_version_id > ?
           ORDER BY ai_version_id LIMIT ?`,
        )
        .bind(siteId, revisionOid, afterId, fetchN)
        .all<AiVersionRow>()
    : await db
        .prepare(
          `SELECT site_id, ai_version_id, revision_oid, bundle_key,
                  bundle_sha256, object_count, redaction_mode,
                  redaction_rules_version, schema_version, created_at
           FROM publish_ai_versions
           WHERE site_id = ? AND revision_oid = ?
           ORDER BY ai_version_id LIMIT ?`,
        )
        .bind(siteId, revisionOid, fetchN)
        .all<AiVersionRow>();
  const rows = result.results ?? [];
  if (rows.length <= limit) return { rows };
  const trimmed = rows.slice(0, limit);
  const last = trimmed[trimmed.length - 1]!;
  return {
    rows: trimmed,
    nextCursor: JSON.stringify({ revision: last.revision_oid, objectId: last.ai_version_id }),
  };
}

export async function findAiVersion(
  db: D1Database,
  siteId: string,
  aiVersionId: string,
): Promise<AiVersionRow | null> {
  // Codex pass-5 P1: `bundle_sha256` MUST be in the projection.
  // Earlier replace-all missed this SELECT because of a different
  // indentation level, so the version-detail route was passing
  // `undefined` into `readPublishedJson(..., expectedSha256)` and
  // silently skipping tamper verification.
  return db
    .prepare(
      `SELECT site_id, ai_version_id, revision_oid, bundle_key,
              bundle_sha256, object_count, redaction_mode,
              redaction_rules_version, schema_version, created_at
       FROM publish_ai_versions
       WHERE site_id = ? AND ai_version_id = ?`,
    )
    .bind(siteId, aiVersionId)
    .first<AiVersionRow>();
}

/* ------------------------------------------------------------------ *
 *  Publish overview (hero page)
 *
 *  Aggregates per-ref publish state and AI-version counts so the
 *  hero / "publish" page can render every ref + its current health
 *  in a single Worker round-trip. The view is bounded by the repo's
 *  ref count (low thousands at the absolute upper bound), and runs
 *  three prepared queries:
 *
 *    1. publish_refs                 — all refs for the site.
 *    2. publish_revisions            — status of every referenced
 *                                      revision (in: clause keeps
 *                                      this O(distinct revisions)).
 *    3. publish_ai_versions          — count(*) per revision_oid.
 *
 *  All three queries are scoped to `site_id` so the Worker never
 *  leaks across sites; cross-revision joins happen in JS so we
 *  avoid synthesising large IN clauses. SQL strings stay fixed.
 * ------------------------------------------------------------------ */

export type PublishOverviewRefRow = RefRow & {
  readonly publish_state: "syncing" | "published" | "failed" | null;
  readonly revision_created_at: string | null;
  readonly file_count: number;
  readonly ai_versions_count: number;
};

export type PublishOverview = {
  readonly refs: readonly PublishOverviewRefRow[];
  /** Default ref row (for HEAD/clone-by-default rendering). Null
   * when the site has no refs published yet. */
  readonly defaultRef: RefRow | null;
};

export async function loadPublishOverview(
  db: D1Database,
  siteId: string,
): Promise<PublishOverview> {
  const refsResult = await db
    .prepare(
      `SELECT site_id, ref_name, ref_type, short_name, target_oid,
              revision_oid, is_default, sync_run_id, schema_version,
              updated_at
       FROM publish_refs
       WHERE site_id = ?
       ORDER BY ref_type, short_name`,
    )
    .bind(siteId)
    .all<RefRow>();
  const refs = refsResult.results ?? [];
  if (refs.length === 0) {
    return { refs: [], defaultRef: null };
  }

  // Codex pass-1 P2: bound the revision/AI-version follow-ups to the
  // distinct revision oids currently referenced by `publish_refs` so
  // accumulated historical revisions don't widen the scan over time.
  // Codex pass-2 P1: D1 caps prepared statements at 100 bound
  // parameters. With one slot reserved for site_id, the IN-list can
  // hold at most 99 oids before D1 rejects the query, so we chunk
  // the dedup'd oid list and merge results in JS. The chunk size
  // leaves a small safety margin in case of future internal binds.
  const distinctRevisionOids = [...new Set(refs.map((r) => r.revision_oid))];
  const PUBLISH_OVERVIEW_OIDS_PER_QUERY = 90;

  const revisionByOid = new Map<
    string,
    { readonly status: "syncing" | "published" | "failed"; readonly file_count: number; readonly created_at: string }
  >();
  const aiCounts = new Map<string, number>();

  for (let i = 0; i < distinctRevisionOids.length; i += PUBLISH_OVERVIEW_OIDS_PER_QUERY) {
    const chunk = distinctRevisionOids.slice(i, i + PUBLISH_OVERVIEW_OIDS_PER_QUERY);
    const inPlaceholders = chunk.map(() => "?").join(", ");

    const revisionsResult = await db
      .prepare(
        `SELECT revision_oid, status, file_count, created_at
         FROM publish_revisions
         WHERE site_id = ? AND revision_oid IN (${inPlaceholders})`,
      )
      .bind(siteId, ...chunk)
      .all<{
        readonly revision_oid: string;
        readonly status: "syncing" | "published" | "failed";
        readonly file_count: number;
        readonly created_at: string;
      }>();
    for (const row of revisionsResult.results ?? []) {
      revisionByOid.set(row.revision_oid, {
        status: row.status,
        file_count: row.file_count,
        created_at: row.created_at,
      });
    }

    const aiCountsResult = await db
      .prepare(
        `SELECT revision_oid, COUNT(*) AS n
         FROM publish_ai_versions
         WHERE site_id = ? AND revision_oid IN (${inPlaceholders})
         GROUP BY revision_oid`,
      )
      .bind(siteId, ...chunk)
      .all<{ readonly revision_oid: string; readonly n: number }>();
    for (const row of aiCountsResult.results ?? []) {
      // Each oid only appears in one chunk (chunks are disjoint),
      // so direct overwrite is safe and we don't need to sum.
      aiCounts.set(row.revision_oid, row.n);
    }
  }

  const enriched: PublishOverviewRefRow[] = refs.map((row) => {
    const rev = revisionByOid.get(row.revision_oid);
    return {
      ...row,
      publish_state: rev?.status ?? null,
      revision_created_at: rev?.created_at ?? null,
      file_count: rev?.file_count ?? 0,
      ai_versions_count: aiCounts.get(row.revision_oid) ?? 0,
    };
  });
  const defaultRef = refs.find((r) => r.is_default === 1) ?? null;
  return { refs: enriched, defaultRef };
}

/* ------------------------------------------------------------------ *
 *  Sync runs
 * ------------------------------------------------------------------ */

export async function findLatestSyncRun(
  db: D1Database,
  siteId: string,
): Promise<SyncRunRow | null> {
  return db
    .prepare(
      `SELECT sync_run_id, site_id, status, started_at, finished_at,
              refs_count, revision_count, file_count, ai_object_count,
              ai_bundle_count, warnings_json, error_message, cli_version,
              schema_version
       FROM publish_sync_runs
       WHERE site_id = ?
       ORDER BY started_at DESC
       LIMIT 1`,
    )
    .bind(siteId)
    .first<SyncRunRow>();
}

/* ------------------------------------------------------------------ *
 *  Helpers used by route handlers
 * ------------------------------------------------------------------ */

/**
 * Resolve a (ref?, revision?) pair into a concrete published
 * revision_oid. Exactly one of ref/revision must be provided; if
 * neither, fall back to the site's default ref (HEAD-equivalent).
 *
 *  - Invalid combinations (both, neither without default ref, malformed)
 *    raise BAD_REQUEST.
 *  - Unknown ref or unknown short ref raises REF_NOT_FOUND.
 *  - Ambiguous short ref (matches both a branch and a tag) raises
 *    AMBIGUOUS_REF (409).
 *  - Unknown revision raises REVISION_NOT_FOUND.
 *  - Non-`published` revisions are NEVER returned (`findPublishedRevision`
 *    filters at the SQL layer).
 */
export async function resolveRevision(
  db: D1Database,
  site: SiteRow,
  refRaw: string | null,
  revisionRaw: string | null,
): Promise<RevisionRow> {
  if (refRaw && revisionRaw) {
    throw badRequest("ref and revision are mutually exclusive", { code: "REF_AND_REVISION_CONFLICT" });
  }
  let revisionOid: string | null = null;
  if (revisionRaw) {
    revisionOid = revisionRaw;
  } else if (refRaw) {
    const ref = await resolveRef(db, site.site_id, refRaw);
    revisionOid = ref.revision_oid;
  } else if (site.default_ref) {
    const def = await findRefByFullName(db, site.site_id, site.default_ref);
    if (!def) {
      throw notFound("REF_NOT_FOUND", "site has a default ref that is missing from publish_refs");
    }
    revisionOid = def.revision_oid;
  } else if (site.latest_revision_oid) {
    revisionOid = site.latest_revision_oid;
  } else {
    throw notFound("REVISION_NOT_FOUND", "site has no published revisions yet");
  }
  const revision = await findPublishedRevision(db, site.site_id, revisionOid);
  if (!revision) {
    throw notFound("REVISION_NOT_FOUND", "no published revision matches this request");
  }
  return revision;
}

export async function resolveRef(
  db: D1Database,
  siteId: string,
  refRaw: string,
): Promise<RefRow> {
  // Full ref forms (refs/heads/* | refs/tags/*) — exact match by ref_name.
  if (refRaw.startsWith("refs/heads/") || refRaw.startsWith("refs/tags/")) {
    const ref = await findRefByFullName(db, siteId, refRaw);
    if (!ref) throw notFound("REF_NOT_FOUND", `no published ref named '${refRaw}'`);
    return ref;
  }
  // Short forms — must resolve uniquely across branches+tags.
  return resolveShortRef(db, siteId, refRaw);
}
