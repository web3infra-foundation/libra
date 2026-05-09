/**
 * Browser-side API client. Calls the same-origin Worker `/api/*`
 * endpoints; never imports `@/lib/server/*` (which would pull D1/R2
 * bindings into the React bundle).
 */

import type {
  AiObjectIndexWire,
  AiVersionIndexWire,
  FileEntryWire,
  RefWire,
  RevisionWire,
  SiteWire,
  SyncRunWire,
} from "@/lib/wire-types";

export type ApiSuccess<T> = { readonly ok: true; readonly data: T };
export type ApiFailure = {
  readonly ok: false;
  readonly code: string;
  readonly message: string;
  readonly detail?: unknown;
};
export type ApiEnvelope<T> = ApiSuccess<T> | ApiFailure;

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly detail?: unknown;
  constructor(status: number, code: string, message: string, detail?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.detail = detail;
  }
}

async function request<T>(input: string, init?: RequestInit): Promise<T> {
  const response = await fetch(input, {
    ...init,
    headers: {
      Accept: "application/json",
      ...(init?.headers ?? {}),
    },
    credentials: "same-origin",
  });
  let body: ApiEnvelope<T>;
  try {
    body = (await response.json()) as ApiEnvelope<T>;
  } catch {
    throw new ApiError(response.status, "INVALID_RESPONSE", "API returned a non-JSON body");
  }
  if (!body.ok) {
    throw new ApiError(response.status, body.code, body.message, body.detail);
  }
  return body.data;
}

export type SiteSummary = {
  readonly site: SiteWire;
  readonly defaultRef: RefWire | null;
  readonly latestRevision: RevisionWire | null;
};
export const fetchSite = (slug: string) =>
  request<SiteSummary>(`/api/sites/${encodeURIComponent(slug)}`);

export type RefsList = {
  readonly siteId: string;
  readonly defaultRef: string | null;
  readonly refsGeneration: number;
  readonly refs: readonly RefWire[];
};
export const fetchRefs = (slug: string, type?: "branch" | "tag") =>
  request<RefsList>(
    `/api/sites/${encodeURIComponent(slug)}/refs${type ? `?type=${type}` : ""}`,
  );

export type Tree = {
  readonly revision: RevisionWire;
  readonly path: string;
  readonly entries: readonly FileEntryWire[];
};
export const fetchTree = (
  slug: string,
  options: { readonly ref?: string; readonly revision?: string; readonly path?: string },
) => {
  const params = new URLSearchParams();
  if (options.ref) params.set("ref", options.ref);
  if (options.revision) params.set("revision", options.revision);
  if (options.path !== undefined && options.path !== "") params.set("path", options.path);
  const qs = params.toString();
  return request<Tree>(`/api/sites/${encodeURIComponent(slug)}/tree${qs ? `?${qs}` : ""}`);
};

export type FileResponse = {
  readonly revision: RevisionWire;
  readonly file: FileEntryWire;
  readonly content:
    | { readonly encoding: "utf-8"; readonly body: string }
    | null;
};
export const fetchFile = (
  slug: string,
  options: { readonly ref?: string; readonly revision?: string; readonly path: string },
) => {
  const params = new URLSearchParams({ path: options.path });
  if (options.ref) params.set("ref", options.ref);
  if (options.revision) params.set("revision", options.revision);
  return request<FileResponse>(
    `/api/sites/${encodeURIComponent(slug)}/file?${params.toString()}`,
  );
};

export type AiVersionsList = {
  readonly revision: RevisionWire;
  readonly versions: readonly AiVersionIndexWire[];
  readonly nextCursor: string | null;
};
export const fetchAiVersions = (
  slug: string,
  options: { readonly ref?: string; readonly revision?: string; readonly cursor?: string },
) => {
  const params = new URLSearchParams();
  if (options.ref) params.set("ref", options.ref);
  if (options.revision) params.set("revision", options.revision);
  if (options.cursor) params.set("cursor", options.cursor);
  const qs = params.toString();
  return request<AiVersionsList>(
    `/api/sites/${encodeURIComponent(slug)}/ai/versions${qs ? `?${qs}` : ""}`,
  );
};

export type AiObjectsList = {
  readonly revision: RevisionWire;
  readonly filter: { readonly objectType: string | null; readonly layer: string | null };
  readonly objects: readonly AiObjectIndexWire[];
  readonly nextCursor: string | null;
};
export const fetchAiObjects = (
  slug: string,
  options: {
    readonly ref?: string;
    readonly revision?: string;
    readonly type?: string;
    readonly layer?: "snapshot" | "event" | "projection";
    readonly cursor?: string;
    readonly limit?: number;
  },
) => {
  const params = new URLSearchParams();
  if (options.ref) params.set("ref", options.ref);
  if (options.revision) params.set("revision", options.revision);
  if (options.type) params.set("type", options.type);
  if (options.layer) params.set("layer", options.layer);
  if (options.cursor) params.set("cursor", options.cursor);
  if (options.limit) params.set("limit", String(options.limit));
  const qs = params.toString();
  return request<AiObjectsList>(
    `/api/sites/${encodeURIComponent(slug)}/ai/objects${qs ? `?${qs}` : ""}`,
  );
};

export type AiObjectDetail = {
  readonly revision: RevisionWire;
  readonly index: AiObjectIndexWire;
  readonly payload: Record<string, unknown>;
};
export const fetchAiObject = (
  slug: string,
  type: string,
  id: string,
  options: { readonly ref?: string; readonly revision?: string },
) => {
  const params = new URLSearchParams();
  if (options.ref) params.set("ref", options.ref);
  if (options.revision) params.set("revision", options.revision);
  const qs = params.toString();
  return request<AiObjectDetail>(
    `/api/sites/${encodeURIComponent(slug)}/ai/objects/${encodeURIComponent(type)}/${encodeURIComponent(id)}${qs ? `?${qs}` : ""}`,
  );
};

export type Status = {
  readonly site: SiteWire;
  readonly latestSyncRun: SyncRunWire | null;
};
export const fetchStatus = (slug: string) =>
  request<Status>(`/api/sites/${encodeURIComponent(slug)}/status`);
