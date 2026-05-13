import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listAiVersions, resolveRevision } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { aiVersionIndexToWire, revisionToWire } from "@/lib/server/wire";
import {
  encodeCursor,
  parseAiVersionId,
  parseCursor,
  parseLimit,
  parseRevisionOid,
  parseSlug,
} from "@/lib/server/validate";
import { PublishApiError, badRequest } from "@/lib/server/errors";

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
    const limit = parseLimit(url.searchParams.get("limit"), 100);
    const cursor = parseCursor(url.searchParams.get("cursor"));
    if (revisionRaw) parseRevisionOid(revisionRaw);

    // Codex pass-13 P2 + pass-14 P2: AI versions cursor uses
    // `objectId` (the ai_version_id) and `revision` (the snapshot
    // it was generated against). Reject empty cursors, missing
    // fields, and any stray fields from sibling routes.
    if (cursor) {
      const allowed = new Set(["objectId", "revision"]);
      const stray = Object.keys(cursor).filter((k) => !allowed.has(k));
      if (stray.length > 0) {
        throw badRequest(
          `ai-versions cursor contains fields not permitted: ${stray.join(",")}`,
        );
      }
      if (!cursor.objectId) {
        throw badRequest("ai-versions cursor must carry objectId");
      }
    }
    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);

    // Codex pass-17 P2: stale-cursor / moving-ref guard. A cursor
    // generated against a previous revision must not be applied to
    // a newer revision the ref now points at; surface 409 so the
    // client restarts pagination cleanly.
    if (cursor && (!cursor.revision || cursor.revision !== revision.revision_oid)) {
      throw new PublishApiError(
        "REVISION_NOT_FOUND",
        409,
        "ai-versions cursor was generated against a different revision; restart pagination",
      );
    }
    const after = cursor?.objectId ? parseAiVersionId(cursor.objectId) : undefined;

    const result = await listAiVersions(bindings.db, site.site_id, revision.revision_oid, limit, after);
    return respondOk(
      {
        revision: revisionToWire(revision),
        versions: result.rows.map(aiVersionIndexToWire),
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
