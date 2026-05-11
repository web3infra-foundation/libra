import "server-only";
import type { Bindings } from "./cloudflare";
import { findSiteBySlug, findSiteByRepoId, type SiteRow } from "./d1";
import { PublishApiError, notFound } from "./errors";
import { enforceVisibility } from "./access";

/**
 * Resolve a slug → site row, scoped to the request's clone domain
 * (the Worker host). Throws SITE_NOT_FOUND on miss and SITE_DISABLED
 * (HTTP 410) when the row is `disabled`. Visibility is enforced via
 * `enforceVisibility` separately so callers can run cheap 404 checks
 * before mounting Access verification.
 */
export async function resolveSiteForSlug(
  bindings: Bindings,
  request: Request,
  slug: string,
): Promise<SiteRow> {
  const cloneDomain = parseHost(request.headers.get("Host"));
  const site = await findSiteBySlug(bindings.db, cloneDomain, slug);
  return ensureActive(site);
}

export async function resolveSiteForRepoId(
  bindings: Bindings,
  request: Request,
  repoId: string,
): Promise<SiteRow> {
  const cloneDomain = parseHost(request.headers.get("Host"));
  if (!cloneDomain) {
    throw notFound("SITE_NOT_FOUND", "could not resolve clone domain from Host header");
  }
  const site = await findSiteByRepoId(bindings.db, cloneDomain, repoId);
  return ensureActive(site);
}

export async function gateRequest(
  bindings: Bindings,
  request: Request,
  site: SiteRow,
): Promise<void> {
  await enforceVisibility(bindings, request, site);
}

function ensureActive(site: SiteRow | null): SiteRow {
  if (!site) {
    throw notFound("SITE_NOT_FOUND", "site is not published");
  }
  if (site.status === "disabled") {
    throw new PublishApiError(
      "SITE_DISABLED",
      410,
      "site has been unpublished and is no longer available",
    );
  }
  return site;
}

function parseHost(raw: string | null): string | null {
  if (!raw) return null;
  // Strip optional port; reject userinfo and path injection.
  const lower = raw.toLowerCase();
  if (lower.includes("/") || lower.includes("@")) return null;
  const colonAt = lower.indexOf(":");
  const host = colonAt === -1 ? lower : lower.slice(0, colonAt);
  if (host.length === 0) return null;
  return host;
}
