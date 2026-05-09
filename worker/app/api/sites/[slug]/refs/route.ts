import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listRefs } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { refToWire } from "@/lib/server/wire";
import { badRequest } from "@/lib/server/errors";
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

    const url = new URL(request.url);
    const typeRaw = url.searchParams.get("type");
    let type: "branch" | "tag" | undefined;
    if (typeRaw && typeRaw !== "branch" && typeRaw !== "tag") {
      throw badRequest("type must be one of branch|tag");
    }
    if (typeRaw === "branch" || typeRaw === "tag") type = typeRaw;

    const rows = await listRefs(bindings.db, site.site_id, type ? { type } : undefined);
    const refs = rows.map(refToWire);
    return respondOk(
      {
        siteId: site.site_id,
        defaultRef: site.default_ref,
        refsGeneration: site.refs_generation,
        refs,
      },
      {
        cache: { mode: "short" },
        etag: `W/"refs-${site.refs_generation}-${refs.length}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
