import "server-only";
import { PublishApiError, type PublishErrorCode } from "./errors";

export type Envelope<T> =
  | { readonly ok: true; readonly data: T }
  | {
      readonly ok: false;
      readonly code: PublishErrorCode;
      readonly message: string;
      readonly detail?: unknown;
    };

export type CacheMode =
  | { readonly mode: "no-store" }
  | { readonly mode: "short" } // small s-maxage for list/status with ETag
  | { readonly mode: "revision-long" }; // cacheable per (revision, asset)

const HEADERS_BASE = {
  "Content-Type": "application/json; charset=utf-8",
  "X-Content-Type-Options": "nosniff",
  // Worker pages and APIs are read-only; deny framing by default to
  // avoid clickjacking when private sites are embedded behind Access.
  "X-Frame-Options": "DENY",
  "Referrer-Policy": "no-referrer",
} as const;

function cacheHeaders(cache: CacheMode, etag?: string): Record<string, string> {
  switch (cache.mode) {
    case "no-store":
      return { "Cache-Control": "no-store" };
    case "short":
      return {
        "Cache-Control": "public, max-age=15, s-maxage=60, must-revalidate",
        ...(etag ? { ETag: etag } : {}),
      };
    case "revision-long":
      // Revision-scoped: revision_oid is included in the cache key
      // upstream (path/query) so we can cache aggressively.
      return {
        "Cache-Control": "public, max-age=300, s-maxage=86400, immutable",
        ...(etag ? { ETag: etag } : {}),
      };
  }
}

export function respondOk<T>(
  data: T,
  options?: { readonly cache?: CacheMode; readonly etag?: string },
): Response {
  const body: Envelope<T> = { ok: true, data };
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      ...HEADERS_BASE,
      ...cacheHeaders(options?.cache ?? { mode: "no-store" }, options?.etag),
    },
  });
}

export function respondError(error: unknown): Response {
  const apiError = error instanceof PublishApiError
    ? error
    : new PublishApiError(
        "INTERNAL",
        500,
        "publish API encountered an internal error",
      );

  const body: Envelope<never> = {
    ok: false,
    code: apiError.code,
    message: apiError.message,
    ...(apiError.detail !== undefined ? { detail: apiError.detail } : {}),
  };

  return new Response(JSON.stringify(body), {
    status: apiError.httpStatus,
    headers: {
      ...HEADERS_BASE,
      "Cache-Control": "no-store",
    },
  });
}
