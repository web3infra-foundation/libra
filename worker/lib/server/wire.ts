import "server-only";
import type {
  AiObjectRow,
  AiVersionRow,
  DirEntry,
  FileRow,
  RefRow,
  RevisionRow,
  SiteRow,
  SyncRunRow,
} from "./d1";
import type {
  AiObjectIndexWire,
  AiVersionIndexWire,
  FileEntryWire,
  RefWire,
  RevisionWire,
  SiteWire,
  SyncRunWire,
} from "../wire-types";

// Re-export the shared wire-format types so server-side callers keep
// `@/lib/server/wire` as a one-stop import. The actual type
// declarations live in `@/lib/wire-types` so that React Client
// Components can import them without pulling `server-only` into the
// browser bundle.
export {
  PUBLISH_SCHEMA_VERSION,
  type AiObjectIndexWire,
  type AiVersionIndexWire,
  type FileEntryWire,
  type RefWire,
  type RevisionWire,
  type SiteWire,
  type SyncRunWire,
} from "../wire-types";

export function siteToWire(row: SiteRow): SiteWire {
  return {
    schemaVersion: row.schema_version,
    siteId: row.site_id,
    repoId: row.repo_id,
    cloneDomain: row.clone_domain,
    slug: row.slug,
    displayOrigin: row.display_origin,
    name: row.name,
    visibility: row.visibility,
    status: row.status,
    workerName: row.worker_name,
    defaultRef: row.default_ref,
    latestRevisionOid: row.latest_revision_oid,
    refsGeneration: row.refs_generation,
    maxPreviewBytes: row.max_preview_bytes,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export function refToWire(row: RefRow): RefWire {
  return {
    refName: row.ref_name,
    refType: row.ref_type,
    shortName: row.short_name,
    targetOid: row.target_oid,
    revisionOid: row.revision_oid,
    isDefault: row.is_default === 1,
    updatedAt: row.updated_at,
  };
}

export function revisionToWire(row: RevisionRow): RevisionWire {
  return {
    schemaVersion: row.schema_version,
    siteId: row.site_id,
    revisionOid: row.revision_oid,
    fileCount: row.file_count,
    aiObjectCount: row.ai_object_count,
    aiBundleCount: row.ai_bundle_count,
    redactionMode: row.redaction_mode,
    redactionRulesVersion: row.redaction_rules_version,
    syncRunId: row.sync_run_id,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
  };
}

export function fileToWire(row: FileRow): FileEntryWire {
  return {
    path: row.path,
    entryKind: "file",
    displayMode: row.display_mode,
    contentSha256: row.content_sha256,
    sizeBytes: row.size_bytes,
    language: row.language,
  };
}

export function dirEntryToWire(row: DirEntry): FileEntryWire {
  return {
    path: row.path,
    entryKind: row._isDirectory ? "directory" : "file",
    displayMode: row.display_mode,
    contentSha256: row.content_sha256,
    sizeBytes: row.size_bytes,
    language: row.language,
  };
}

export function aiObjectIndexToWire(row: AiObjectRow): AiObjectIndexWire {
  return {
    objectType: row.object_type,
    objectId: row.object_id,
    layer: row.layer,
    redactionMode: row.redaction_mode,
    payloadSha256: row.payload_sha256,
    createdAt: row.created_at,
  };
}

export function aiVersionIndexToWire(row: AiVersionRow): AiVersionIndexWire {
  return {
    aiVersionId: row.ai_version_id,
    revisionOid: row.revision_oid,
    objectCount: row.object_count,
    redactionMode: row.redaction_mode,
    redactionRulesVersion: row.redaction_rules_version,
    createdAt: row.created_at,
  };
}

export function syncRunToWire(row: SyncRunRow): SyncRunWire {
  let warnings: readonly string[] = [];
  try {
    const parsed = JSON.parse(row.warnings_json);
    if (Array.isArray(parsed)) {
      warnings = parsed.filter((entry): entry is string => typeof entry === "string");
    }
  } catch {
    warnings = [];
  }
  return {
    syncRunId: row.sync_run_id,
    status: row.status,
    startedAt: row.started_at,
    finishedAt: row.finished_at,
    refsCount: row.refs_count,
    revisionCount: row.revision_count,
    fileCount: row.file_count,
    aiObjectCount: row.ai_object_count,
    aiBundleCount: row.ai_bundle_count,
    warnings,
    errorMessage: row.error_message,
    cliVersion: row.cli_version,
  };
}
