import "server-only";

/**
 * Typed publish-API error. Worker route handlers throw `PublishApiError`
 * and the central `respondError` helper maps it to a stable JSON envelope:
 *
 *   { ok: false, code: "...", message: "...", detail?: ... }
 *
 * `code` is a stable enum string; `message` is human-readable; `detail`
 * MUST NOT contain bucket names, internal R2 keys, raw SQL or anything
 * that would let a caller infer storage layout.
 */
export class PublishApiError extends Error {
  readonly code: PublishErrorCode;
  readonly httpStatus: number;
  readonly detail: unknown;

  constructor(code: PublishErrorCode, httpStatus: number, message: string, detail?: unknown) {
    super(message);
    this.name = "PublishApiError";
    this.code = code;
    this.httpStatus = httpStatus;
    this.detail = detail;
  }
}

export type PublishErrorCode =
  | "BAD_REQUEST"
  | "INVALID_REF"
  | "INVALID_REVISION"
  | "INVALID_PATH"
  | "INVALID_SLUG"
  | "INVALID_OBJECT_TYPE"
  | "AMBIGUOUS_REF"
  | "REF_AND_REVISION_CONFLICT"
  | "ACCESS_REQUIRED"
  | "ACCESS_DENIED"
  | "SITE_NOT_FOUND"
  | "REVISION_NOT_FOUND"
  | "REF_NOT_FOUND"
  | "FILE_NOT_FOUND"
  | "OBJECT_NOT_FOUND"
  | "BUNDLE_NOT_FOUND"
  | "R2_OBJECT_MISSING"
  | "R2_OBJECT_CORRUPT"
  | "SITE_DISABLED"
  | "INTERNAL";

export function badRequest(message: string, detail?: unknown): PublishApiError {
  return new PublishApiError("BAD_REQUEST", 400, message, detail);
}

export function notFound(code: PublishErrorCode, message: string): PublishApiError {
  return new PublishApiError(code, 404, message);
}
