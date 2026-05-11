import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listDirEntries, resolveRevision } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { dirEntryToWire, revisionToWire } from "@/lib/server/wire";
import { parsePathOrRoot, parseRevisionOid, parseSlug } from "@/lib/server/validate";

export const runtime = "edge";
export const dynamic = "force-dynamic";

export async function GET(
  request: NextRequest,
  context: { readonly params: Promise<{ readonly slug: string }> },
): Promise<Response> {
  try {
    const { slug: rawSlug } = await context.params;
    const slug = parseSlug(rawSlug);
    const bindings = getBindings();
    const site = await resolveSiteForSlug(bindings, request, slug);
    await gateRequest(bindings, request, site);

    const url = new URL(request.url);
    const refRaw = url.searchParams.get("ref");
    const revisionRaw = url.searchParams.get("revision");
    const path = parsePathOrRoot(url.searchParams.get("path"));
    if (revisionRaw) parseRevisionOid(revisionRaw); // shape validation only.

    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);
    // Codex pass-12 P2 (deferred): publish.md `Worker API` table
    // does NOT call out tree as a paginated list; the immediate-
    // children synthesis logic in `listDirEntries` derives
    // directory entries from path prefixes and would need a
    // hierarchical cursor scheme to paginate cleanly. The current
    // listing is bounded by `publish_files.path` cardinality of one
    // revision (itself capped by the publish ignore policy and
    // `max_preview_bytes`). Add pagination here once the file count
    // per revision exceeds D1's `MAX_BIND_PARAMETERS` / response
    // size limits in practice.
    const entries = await listDirEntries(bindings.db, site.site_id, revision.revision_oid, path);

    // Codex pass-11 P2: cache the response immutably only when the
    // caller pinned an explicit `revision=<oid>`. A `ref` parameter
    // (or the default-ref fallback) can move on the next sync, so
    // cache it for the short-cache window with an ETag and let the
    // client revalidate.
    const cacheMode = revisionRaw ? ("revision-long" as const) : ("short" as const);
    return respondOk(
      {
        revision: revisionToWire(revision),
        path,
        entries: entries.map(dirEntryToWire),
      },
      {
        cache: { mode: cacheMode },
        etag: `W/"tree-${revision.revision_oid}-${entries.length}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
