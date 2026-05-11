import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { findDefaultRef, findPublishedRevision } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { siteToWire, refToWire, revisionToWire } from "@/lib/server/wire";
import { parseSlug } from "@/lib/server/validate";

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

    const defaultRef = site.default_ref
      ? await findDefaultRef(bindings.db, site.site_id)
      : null;

    const latestRevision = site.latest_revision_oid
      ? await findPublishedRevision(bindings.db, site.site_id, site.latest_revision_oid)
      : null;

    return respondOk(
      {
        site: siteToWire(site),
        defaultRef: defaultRef ? refToWire(defaultRef) : null,
        latestRevision: latestRevision ? revisionToWire(latestRevision) : null,
      },
      { cache: { mode: "no-store" }, visibility: site.visibility },
    );
  } catch (error) {
    return respondError(error);
  }
}
