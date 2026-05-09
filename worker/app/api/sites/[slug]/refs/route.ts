import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listRefs } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { refToWire } from "@/lib/server/wire";
import { badRequest } from "@/lib/server/errors";
import { encodeCursor, parseCursor, parseLimit, parseSlug } from "@/lib/server/validate";

export const runtime = "edge";
export const dynamic = "force-dynamic";

// Codex pass-11 P2: refs list paginates with keyset cursors so the
// publish.md "list 接口必须分页" rule applies uniformly. The cursor
// is `(ref_type, short_name)` since the listRefs SQL orders on
// those fields. Default limit 100, max 500.
const REFS_DEFAULT_LIMIT = 100;
const REFS_MAX_LIMIT = 500;

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

    const limit = parseLimit(url.searchParams.get("limit"), REFS_MAX_LIMIT);
    const limitOrDefault = url.searchParams.has("limit") ? limit : REFS_DEFAULT_LIMIT;
    const cursor = parseCursor(url.searchParams.get("cursor"));
    const afterRefType = cursor?.objectType as "branch" | "tag" | undefined;
    const afterShortName = cursor?.objectId;
    if (afterRefType && afterRefType !== "branch" && afterRefType !== "tag") {
      throw badRequest("cursor ref_type must be branch|tag");
    }

    const rows = await listRefs(bindings.db, site.site_id, type ? { type } : undefined);
    // listRefs returns the full set; trim with the cursor + limit
    // server-side. The set is bounded by the repo's ref count which
    // is itself bounded — D1 cardinality stays manageable.
    const filtered = afterRefType && afterShortName
      ? rows.filter((row) =>
          row.ref_type > afterRefType ||
          (row.ref_type === afterRefType && row.short_name > afterShortName),
        )
      : rows;
    const trimmed = filtered.slice(0, limitOrDefault);
    const nextCursor =
      filtered.length > limitOrDefault
        ? encodeCursor({
            objectType: trimmed[trimmed.length - 1]!.ref_type,
            objectId: trimmed[trimmed.length - 1]!.short_name,
          })
        : null;
    const refs = trimmed.map(refToWire);
    return respondOk(
      {
        siteId: site.site_id,
        defaultRef: site.default_ref,
        refsGeneration: site.refs_generation,
        refs,
        nextCursor,
      },
      {
        cache: { mode: "short" },
        etag: `W/"refs-${site.refs_generation}-${refs.length}-${nextCursor ?? "end"}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
