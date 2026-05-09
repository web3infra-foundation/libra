import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { findAiVersion, findPublishedRevision } from "@/lib/server/d1";
import { readPublishedJson } from "@/lib/server/r2";
import { respondError, respondOk } from "@/lib/server/response";
import { aiVersionIndexToWire, revisionToWire } from "@/lib/server/wire";
import { notFound } from "@/lib/server/errors";
import { parseAiVersionId, parseSlug } from "@/lib/server/validate";

export const runtime = "edge";
export const dynamic = "force-dynamic";

export async function GET(
  request: NextRequest,
  context: { readonly params: Promise<{ readonly slug: string; readonly id: string }> },
): Promise<Response> {
  try {
    const { slug: rawSlug, id: rawId } = await context.params;
    const slug = parseSlug(rawSlug);
    const versionId = parseAiVersionId(rawId);
    const bindings = getBindings();
    const site = await resolveSiteForSlug(bindings, request, slug);
    await gateRequest(bindings, request, site);

    const versionRow = await findAiVersion(bindings.db, site.site_id, versionId);
    if (!versionRow) {
      throw notFound("BUNDLE_NOT_FOUND", "no AI bundle matches this id for this site");
    }
    const revision = await findPublishedRevision(bindings.db, site.site_id, versionRow.revision_oid);
    if (!revision) {
      throw notFound("REVISION_NOT_FOUND", "AI bundle revision is not published");
    }

    const bundle = await readPublishedJson<Record<string, unknown>>(bindings.bucket, versionRow.bundle_key);

    return respondOk(
      {
        version: aiVersionIndexToWire(versionRow),
        revision: revisionToWire(revision),
        bundle,
      },
      {
        cache: { mode: "revision-long" },
        etag: `W/"bundle-${versionRow.ai_version_id}-${versionRow.revision_oid}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
