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

export type CacheVisibility = "public" | "private";

const HEADERS_BASE = {
  "Content-Type": "application/json; charset=utf-8",
  "X-Content-Type-Options": "nosniff",
  // Worker pages and APIs are read-only; deny framing by default to
  // avoid clickjacking when private sites are embedded behind Access.
  "X-Frame-Options": "DENY",
  "Referrer-Policy": "no-referrer",
} as const;

/**
 * Build the Cache-Control header for a successful response.
 *
 * Codex pass-1 P1: caches MUST never store responses for `private`
 * sites. An earlier draft emitted `Cache-Control: public, max-age=...`
 * for every cacheable response, which would have let a Cloudflare
 * edge cache (or any intermediary) reuse one authenticated read for
 * an unauthenticated follow-up. The visibility-aware policy below
 * forces `private, no-store` whenever the resource came from a
 * `private` site, even when the underlying handler is otherwise
 * cacheable.
 */
function cacheHeaders(
  cache: CacheMode,
  visibility: CacheVisibility,
  etag?: string,
): Record<string, string> {
  if (visibility === "private") {
    return { "Cache-Control": "private, no-store" };
  }
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
  options?: {
    readonly cache?: CacheMode;
    readonly etag?: string;
    /**
     * Site visibility for the response. Defaults to `"private"` —
     * callers without a SiteRow context (e.g. health endpoints, the
     * landing page) should leave this unset so the response is never
     * cached by intermediaries.
     */
    readonly visibility?: CacheVisibility;
  },
): Response {
  const body: Envelope<T> = { ok: true, data };
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      ...HEADERS_BASE,
      ...cacheHeaders(
        options?.cache ?? { mode: "no-store" },
        options?.visibility ?? "private",
        options?.etag,
      ),
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
