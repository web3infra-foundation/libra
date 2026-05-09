import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { findLatestSyncRun } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { siteToWire, syncRunToWire } from "@/lib/server/wire";
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

    const latest = await findLatestSyncRun(bindings.db, site.site_id);
    return respondOk(
      {
        site: siteToWire(site),
        latestSyncRun: latest ? syncRunToWire(latest) : null,
      },
      { cache: { mode: "no-store" }, visibility: site.visibility },
    );
  } catch (error) {
    return respondError(error);
  }
}
