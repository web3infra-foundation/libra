import "server-only";
import { badRequest } from "./errors";

const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const REPO_ID_RE = /^[A-Za-z0-9_][A-Za-z0-9_-]{0,127}$/;
const HEX_RE = /^[0-9a-f]+$/;
const SHORT_REF_RE = /^[A-Za-z0-9._][A-Za-z0-9._/-]{0,254}$/;
const FULL_BRANCH_RE = /^refs\/heads\/[A-Za-z0-9._][A-Za-z0-9._/-]*$/;
const FULL_TAG_RE = /^refs\/tags\/[A-Za-z0-9._][A-Za-z0-9._/-]*$/;
const OBJECT_TYPE_RE = /^[A-Za-z][A-Za-z0-9]{0,63}$/;
const OBJECT_ID_RE = /^[A-Za-z0-9._:-]{1,256}$/;
// AI version IDs are produced by Libra; tolerate UUIDs and short-codes.
const AI_VERSION_ID_RE = /^[A-Za-z0-9._-]{1,128}$/;

export function parseSlug(raw: string | null | undefined): string {
  if (!raw) throw badRequest("slug is required");
  if (!SLUG_RE.test(raw)) {
    throw badRequest("slug must match [a-z0-9][a-z0-9-]{0,62}");
  }
  return raw;
}

export function parseRepoId(raw: string | null | undefined): string {
  if (!raw) throw badRequest("repo_id is required");
  if (!REPO_ID_RE.test(raw)) {
    throw badRequest("repo_id contains disallowed characters");
  }
  return raw;
}

export function parseRevisionOid(raw: string): string {
  if (!HEX_RE.test(raw) || raw.length < 4 || raw.length > 64) {
    throw badRequest("revision must be a hex object id (4..64 chars)");
  }
  return raw;
}

export type ParsedRef =
  | { readonly kind: "full"; readonly fullName: string; readonly type: "branch" | "tag"; readonly shortName: string }
  | { readonly kind: "short"; readonly shortName: string };

export function parseRef(raw: string): ParsedRef {
  if (!raw) throw badRequest("ref is required");
  if (FULL_BRANCH_RE.test(raw)) {
    return { kind: "full", fullName: raw, type: "branch", shortName: raw.slice("refs/heads/".length) };
  }
  if (FULL_TAG_RE.test(raw)) {
    return { kind: "full", fullName: raw, type: "tag", shortName: raw.slice("refs/tags/".length) };
  }
  if (raw.startsWith("refs/")) {
    throw badRequest("only refs/heads/* and refs/tags/* are published");
  }
  if (!SHORT_REF_RE.test(raw)) {
    throw badRequest("ref contains disallowed characters");
  }
  return { kind: "short", shortName: raw };
}

/**
 * Parse a path component for a tree/file lookup. Mirrors the safety
 * checks the Rust snapshot builder enforces on ingest:
 *
 *  - never empty
 *  - no `..` segment
 *  - no leading or duplicate `/`
 *  - no NUL byte
 *  - reasonable length cap
 *
 * Empty path is the repo root and is allowed via `parsePathOrRoot`.
 */
export function parsePath(raw: string | null | undefined): string {
  if (raw === null || raw === undefined) {
    throw badRequest("path is required");
  }
  if (raw.length === 0 || raw.length > 4096) {
    throw badRequest("path length is out of range (1..=4096)");
  }
  if (raw.includes("\0")) throw badRequest("path contains NUL");
  if (raw.startsWith("/") || raw.includes("//")) {
    throw badRequest("path must not start with or contain doubled slashes");
  }
  for (const segment of raw.split("/")) {
    if (segment === "..") throw badRequest("path must not contain '..'");
    if (segment.length === 0) throw badRequest("path must not contain empty segments");
  }
  return raw;
}

export function parsePathOrRoot(raw: string | null | undefined): string {
  if (raw === null || raw === undefined || raw === "") return "";
  return parsePath(raw);
}

export function parseObjectType(raw: string | null | undefined): string {
  if (!raw) throw badRequest("object type is required");
  if (!OBJECT_TYPE_RE.test(raw)) {
    throw badRequest("object type contains disallowed characters");
  }
  return raw;
}

export function parseObjectId(raw: string | null | undefined): string {
  if (!raw) throw badRequest("object id is required");
  if (!OBJECT_ID_RE.test(raw)) {
    throw badRequest("object id contains disallowed characters");
  }
  return raw;
}

export function parseAiVersionId(raw: string | null | undefined): string {
  if (!raw) throw badRequest("ai version id is required");
  if (!AI_VERSION_ID_RE.test(raw)) {
    throw badRequest("ai version id contains disallowed characters");
  }
  return raw;
}

export function parseLayer(raw: string | null | undefined): "snapshot" | "event" | "projection" | undefined {
  if (raw === null || raw === undefined || raw === "") return undefined;
  if (raw === "snapshot" || raw === "event" || raw === "projection") return raw;
  throw badRequest("layer must be one of snapshot|event|projection");
}

export function parseLimit(raw: string | null | undefined, max = 100): number {
  if (raw === null || raw === undefined || raw === "") return Math.min(50, max);
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0 || n > max) {
    throw badRequest(`limit must be a positive integer up to ${max}`);
  }
  return n;
}

/**
 * Cursor encoding: base64url JSON `{revision_oid?, ref_name?, started_at?, object_type?, object_id?}`.
 * Cursors are produced by the same module that consumes them; the
 * regex-only approach keeps callers from injecting arbitrary keys
 * even if they craft a base64 blob.
 */
export type Cursor = Readonly<{
  ref?: string;
  revision?: string;
  startedAt?: string;
  objectType?: string;
  objectId?: string;
}>;

export function parseCursor(raw: string | null | undefined): Cursor | undefined {
  if (!raw) return undefined;
  if (raw.length > 1024) throw badRequest("cursor is too large");
  let parsed: unknown;
  try {
    const padded = raw.padEnd(raw.length + ((4 - (raw.length % 4)) % 4), "=");
    const normalised = padded.replace(/-/g, "+").replace(/_/g, "/");
    const decoded = atob(normalised);
    parsed = JSON.parse(decoded);
  } catch {
    throw badRequest("cursor is malformed");
  }
  if (!parsed || typeof parsed !== "object") {
    throw badRequest("cursor must encode an object");
  }
  const obj = parsed as Record<string, unknown>;
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (typeof value !== "string") {
      throw badRequest("cursor field types are invalid");
    }
    if (
      key !== "ref" && key !== "revision" && key !== "startedAt" &&
      key !== "objectType" && key !== "objectId"
    ) {
      throw badRequest(`cursor contains unknown field: ${key}`);
    }
    out[key] = value;
  }
  return out as Cursor;
}

export function encodeCursor(cursor: Cursor): string {
  const encoded = btoa(JSON.stringify(cursor));
  return encoded.replace(/=+$/g, "").replace(/\+/g, "-").replace(/\//g, "_");
}
