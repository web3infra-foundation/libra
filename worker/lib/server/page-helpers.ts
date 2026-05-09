import "server-only";
import { forbidden, notFound } from "next/navigation";
import { getBindings } from "./cloudflare";
import {
  findDefaultRef,
  findFileRow,
  findPublishedRevision,
  findSiteByRepoId,
  findSiteBySlug,
  listDirEntries,
  listRefs,
  resolveRef,
  type RefRow,
  type SiteRow,
} from "./d1";
import { readPublishedTextFile } from "./r2";
import { PublishApiError } from "./errors";
import { enforceVisibility } from "./access";
import type { FileEntryWire, RefWire, SiteWire } from "../wire-types";
import {
  dirEntryToWire,
  fileToWire,
  refToWire,
  revisionToWire,
  siteToWire,
  type RevisionWire,
} from "./wire";
import { headers } from "next/headers";

type PageContext = {
  readonly site: SiteRow;
  readonly siteWire: SiteWire;
  readonly bindings: ReturnType<typeof getBindings>;
  readonly cloneDomain: string | null;
};

async function getCloneDomain(): Promise<string | null> {
  const hdrs = await headers();
  const raw = hdrs.get("host");
  if (!raw) return null;
  const lower = raw.toLowerCase();
  if (lower.includes("/") || lower.includes("@")) return null;
  const colonAt = lower.indexOf(":");
  return colonAt === -1 ? lower : lower.slice(0, colonAt);
}

/**
 * Server-side site lookup for Next page components.
 *
 * Codex pass-1 P1: page handlers MUST surface 403 / 410 distinctly
 * from 404 so the user gets the same error envelope the API does.
 * An earlier draft caught every `PublishApiError` and called
 * `notFound()`, which collapsed `ACCESS_REQUIRED`, `ACCESS_DENIED`,
 * and `SITE_DISABLED` into a generic 404 — losing both the audit
 * signal for monitoring and the distinct UX for "you need to sign
 * in via Cloudflare Access" vs "this slug doesn't exist". The
 * handler now lets `PublishApiError`s propagate; the route segment
 * exports a `generateMetadata` / error boundary upstream that
 * rethrows them with the typed status. The same applies to the
 * `repoId` redirect path which must enforce the Access gate before
 * leaking the slug to the redirect target.
 */
async function gateOrSurface(bindings: ReturnType<typeof getBindings>, site: SiteRow): Promise<void> {
  // Codex pass-1 P1: pages must distinguish 403 / 410 from 404. We
  // delegate to the typed helpers — `forbidden()` triggers Next's
  // 403 boundary, and `SITE_DISABLED` is rethrown so the route's
  // error boundary can render a 410-flavoured page rather than the
  // generic 404 the previous draft produced.
  if (site.status === "disabled") {
    throw new PublishApiError(
      "SITE_DISABLED",
      410,
      "site has been unpublished and is no longer available",
    );
  }
  const fauxRequest = new Request("https://internal.libra/page", {
    headers: await headersToObject(),
  });
  try {
    await enforceVisibility(bindings, fauxRequest, site);
  } catch (error) {
    if (
      error instanceof PublishApiError &&
      (error.code === "ACCESS_REQUIRED" || error.code === "ACCESS_DENIED")
    ) {
      // Surface as Next's 403 instead of collapsing into 404.
      forbidden();
    }
    throw error;
  }
}

export async function loadSiteContextForSlug(slug: string): Promise<PageContext> {
  const bindings = getBindings();
  const cloneDomain = await getCloneDomain();
  const site = await findSiteBySlug(bindings.db, cloneDomain, slug);
  if (!site) notFound();
  await gateOrSurface(bindings, site);
  return { site, siteWire: siteToWire(site), bindings, cloneDomain };
}

export async function loadSiteContextForRepoId(repoId: string): Promise<PageContext> {
  const bindings = getBindings();
  const cloneDomain = await getCloneDomain();
  if (!cloneDomain) notFound();
  const site = await findSiteByRepoId(bindings.db, cloneDomain, repoId);
  if (!site) notFound();
  // Codex pass-1 P1: enforce Access BEFORE we surface the slug to a
  // redirect. Without this gate, anyone hitting
  // `/sites/repo/<repo_id>` of a private site would learn the slug
  // even when they could not access the slug page.
  await gateOrSurface(bindings, site);
  return { site, siteWire: siteToWire(site), bindings, cloneDomain };
}

async function headersToObject(): Promise<Record<string, string>> {
  const hdrs = await headers();
  const out: Record<string, string> = {};
  hdrs.forEach((value, key) => {
    out[key] = value;
  });
  return out;
}

/**
 * Resolve a ref query parameter to a concrete `RefRow`, defaulting to
 * the site's `default_ref`. Returns null when the site has no refs at
 * all yet (newly initialised site, sync never ran).
 */
export async function resolveRefOrDefault(
  ctx: PageContext,
  refRaw: string | null,
): Promise<RefRow | null> {
  if (refRaw) {
    try {
      return await resolveRef(ctx.bindings.db, ctx.site.site_id, refRaw);
    } catch (error) {
      if (error instanceof PublishApiError && error.code === "AMBIGUOUS_REF") {
        return Promise.reject(error);
      }
      return null;
    }
  }
  if (ctx.site.default_ref) {
    return findDefaultRef(ctx.bindings.db, ctx.site.site_id);
  }
  return null;
}

export async function loadRefsForSite(ctx: PageContext): Promise<readonly RefWire[]> {
  const rows = await listRefs(ctx.bindings.db, ctx.site.site_id);
  return rows.map(refToWire);
}

export type LoadedTree = {
  readonly revision: RevisionWire;
  readonly path: string;
  readonly entries: readonly FileEntryWire[];
};

export async function loadTreeForRef(
  ctx: PageContext,
  ref: RefRow,
  path: string,
): Promise<LoadedTree | null> {
  const revisionRow = await findPublishedRevision(
    ctx.bindings.db,
    ctx.site.site_id,
    ref.revision_oid,
  );
  if (!revisionRow) return null;
  const entries = await listDirEntries(
    ctx.bindings.db,
    ctx.site.site_id,
    revisionRow.revision_oid,
    path,
  );
  return {
    revision: revisionToWire(revisionRow),
    path,
    entries: entries.map(dirEntryToWire),
  };
}

export type LoadedFile = {
  readonly revision: RevisionWire;
  readonly file: FileEntryWire;
  readonly content: string | null;
};

export async function loadFileForRef(
  ctx: PageContext,
  ref: RefRow,
  path: string,
): Promise<LoadedFile | null> {
  const revisionRow = await findPublishedRevision(
    ctx.bindings.db,
    ctx.site.site_id,
    ref.revision_oid,
  );
  if (!revisionRow) return null;
  const fileRow = await findFileRow(
    ctx.bindings.db,
    ctx.site.site_id,
    revisionRow.revision_oid,
    path,
  );
  if (!fileRow) return null;
  let content: string | null = null;
  if (fileRow.display_mode === "text" && fileRow.r2_key) {
    const read = await readPublishedTextFile(
      ctx.bindings.bucket,
      { display_mode: fileRow.display_mode, r2_key: fileRow.r2_key, size_bytes: fileRow.size_bytes },
      fileRow.content_sha256,
    );
    content = read?.body ?? null;
  }
  return {
    revision: revisionToWire(revisionRow),
    file: fileToWire(fileRow),
    content,
  };
}
