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
    const entries = await listDirEntries(bindings.db, site.site_id, revision.revision_oid, path);

    return respondOk(
      {
        revision: revisionToWire(revision),
        path,
        entries: entries.map(dirEntryToWire),
      },
      {
        cache: { mode: "revision-long" },
        etag: `W/"tree-${revision.revision_oid}-${entries.length}"`,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
