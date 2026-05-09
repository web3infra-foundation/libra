import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listPublishedRevisions } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { revisionToWire } from "@/lib/server/wire";
import { encodeCursor, parseCursor, parseLimit, parseRevisionOid, parseSlug } from "@/lib/server/validate";
import { badRequest } from "@/lib/server/errors";

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
    const limit = parseLimit(url.searchParams.get("limit"), 100);
    const cursor = parseCursor(url.searchParams.get("cursor"));

    // Codex pass-13 P2 + pass-14 P2: revisions cursor MUST carry
    // both `revision` AND `startedAt`. Reject empty cursors, partial
    // cursors, and any stray fields from sibling routes.
    if (cursor) {
      const allowed = new Set(["revision", "startedAt"]);
      const stray = Object.keys(cursor).filter((k) => !allowed.has(k));
      if (stray.length > 0) {
        throw badRequest(
          `revisions cursor contains fields not permitted: ${stray.join(",")}`,
        );
      }
      if (!cursor.revision || !cursor.startedAt) {
        throw badRequest("revisions cursor must carry both revision and startedAt");
      }
    }
    const before = cursor?.revision && cursor?.startedAt
      ? { revisionOid: parseRevisionOid(cursor.revision), createdAt: cursor.startedAt }
      : undefined;

    const result = await listPublishedRevisions(bindings.db, site.site_id, limit, before);
    return respondOk(
      {
        siteId: site.site_id,
        revisions: result.rows.map(revisionToWire),
        nextCursor: result.nextCursor
          ? encodeCursor(JSON.parse(result.nextCursor) as Record<string, string>)
          : null,
      },
      { cache: { mode: "short" }, visibility: site.visibility },
    );
  } catch (error) {
    return respondError(error);
  }
}
