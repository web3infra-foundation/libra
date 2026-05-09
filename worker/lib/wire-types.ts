// Shared wire types — pure type module, importable by both client
// components and server modules. No runtime side effects, no imports
// from `@/lib/server/*`. Mirror of the Rust serde contract types in
// `src/internal/publish/contract.rs`.

export const PUBLISH_SCHEMA_VERSION = 1;

export type SiteWire = {
  schemaVersion: number;
  siteId: string;
  repoId: string;
  cloneDomain: string;
  slug: string;
  displayOrigin: string;
  name: string;
  visibility: "public" | "private";
  status: "active" | "disabled";
  workerName: string;
  defaultRef: string | null;
  latestRevisionOid: string | null;
  refsGeneration: number;
  maxPreviewBytes: number;
  createdAt: string;
  updatedAt: string;
};

export type RefWire = {
  refName: string;
  refType: "branch" | "tag";
  shortName: string;
  targetOid: string;
  revisionOid: string;
  isDefault: boolean;
  updatedAt: string;
};

export type RevisionWire = {
  schemaVersion: number;
  siteId: string;
  revisionOid: string;
  fileCount: number;
  aiObjectCount: number;
  aiBundleCount: number;
  redactionMode: "default" | "strict";
  redactionRulesVersion: string;
  syncRunId: string;
  createdAt: string;
  updatedAt: string;
};

export type FileEntryWire = {
  path: string;
  /** "directory" is synthesised from path prefixes; only leaf files have a real row in `publish_files`. */
  entryKind: "directory" | "file";
  displayMode: "text" | "binary" | "too_large" | "ignored";
  contentSha256: string | null;
  sizeBytes: number;
  language: string | null;
};

export type AiObjectIndexWire = {
  objectType: string;
  objectId: string;
  layer: "snapshot" | "event" | "projection";
  redactionMode: "default" | "strict";
  payloadSha256: string;
  createdAt: string;
};

export type AiVersionIndexWire = {
  aiVersionId: string;
  revisionOid: string;
  objectCount: number;
  redactionMode: "default" | "strict";
  redactionRulesVersion: string;
  /**
   * Codex pass-4 P2: lowercase 64-char hex sha256 of the bundle JSON
   * referenced by this row. Surfaced so clients (and contract round-
   * trip tests) can compare R2 bodies, but the Worker performs the
   * authoritative verification before responding.
   */
  bundleSha256: string;
  createdAt: string;
};

export type SyncRunWire = {
  syncRunId: string;
  status: "running" | "succeeded" | "failed";
  startedAt: string;
  finishedAt: string | null;
  refsCount: number;
  revisionCount: number;
  fileCount: number;
  aiObjectCount: number;
  aiBundleCount: number;
  warnings: readonly string[];
  errorMessage: string | null;
  cliVersion: string;
};
