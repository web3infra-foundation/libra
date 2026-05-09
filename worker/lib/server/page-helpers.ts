import "server-only";
import { notFound } from "next/navigation";
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
 * Server-side site lookup for Next page components. Throws Next 404
 * for unknown / disabled / Access-denied resources so the framework
 * renders the global not-found page.
 */
export async function loadSiteContextForSlug(slug: string): Promise<PageContext> {
  const bindings = getBindings();
  const cloneDomain = await getCloneDomain();
  const site = await findSiteBySlug(bindings.db, cloneDomain, slug);
  if (!site || site.status === "disabled") notFound();
  try {
    const fauxRequest = new Request("https://internal.libra/page", {
      headers: await headersToObject(),
    });
    await enforceVisibility(bindings, fauxRequest, site);
  } catch (error) {
    if (error instanceof PublishApiError) notFound();
    throw error;
  }
  return { site, siteWire: siteToWire(site), bindings, cloneDomain };
}

export async function loadSiteContextForRepoId(repoId: string): Promise<PageContext> {
  const bindings = getBindings();
  const cloneDomain = await getCloneDomain();
  if (!cloneDomain) notFound();
  const site = await findSiteByRepoId(bindings.db, cloneDomain, repoId);
  if (!site || site.status === "disabled") notFound();
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
