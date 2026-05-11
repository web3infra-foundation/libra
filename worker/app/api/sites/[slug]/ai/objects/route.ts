import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listAiObjects, resolveRevision } from "@/lib/server/d1";
import { respondError, respondOk } from "@/lib/server/response";
import { aiObjectIndexToWire, revisionToWire } from "@/lib/server/wire";
import {
  encodeCursor,
  parseCursor,
  parseLayer,
  parseLimit,
  parseObjectId,
  parseObjectType,
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
    const typeRaw = url.searchParams.get("type");
    const objectType = typeRaw ? parseObjectType(typeRaw) : undefined;
    const layer = parseLayer(url.searchParams.get("layer"));
    const limit = parseLimit(url.searchParams.get("limit"), 200);
    const cursor = parseCursor(url.searchParams.get("cursor"));
    if (revisionRaw) parseRevisionOid(revisionRaw);

    // Codex pass-13 P2 + pass-14 P2: AI objects cursor MUST carry
    // both `objectType` AND `objectId`. Reject empty cursors,
    // partial cursors, and stray fields from sibling routes.
    if (cursor) {
      const allowed = new Set(["objectType", "objectId", "revision"]);
      const stray = Object.keys(cursor).filter((k) => !allowed.has(k));
      if (stray.length > 0) {
        throw badRequest(
          `ai-objects cursor contains fields not permitted: ${stray.join(",")}`,
        );
      }
      if (!cursor.objectType || !cursor.objectId) {
        throw badRequest("ai-objects cursor must carry both objectType and objectId");
      }
    }
    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);

    // Codex pass-17 P2: a cursor ALWAYS pins the revision it was
    // generated against. If the caller supplied `?ref=main` and the
    // ref has advanced since the previous page resolved, the
    // resolved revision will differ from `cursor.revision`. Refuse
    // to silently apply the old cursor to the new revision —
    // returning 409 lets the client restart pagination cleanly. The
    // server-issued cursor never omits `revision`, so a missing
    // value means a manually-constructed cursor; reject those too.
    if (cursor && (!cursor.revision || cursor.revision !== revision.revision_oid)) {
      throw new PublishApiError(
        "REVISION_NOT_FOUND",
        409,
        "ai-objects cursor was generated against a different revision; restart pagination",
      );
    }

    const afterObjectType = cursor?.objectType ? parseObjectType(cursor.objectType) : undefined;
    const afterObjectId = cursor?.objectId ? parseObjectId(cursor.objectId) : undefined;

    const result = await listAiObjects(bindings.db, {
      siteId: site.site_id,
      revisionOid: revision.revision_oid,
      objectType,
      layer,
      limit,
      afterObjectType,
      afterObjectId,
    });

    return respondOk(
      {
        revision: revisionToWire(revision),
        filter: { objectType: objectType ?? null, layer: layer ?? null },
        objects: result.rows.map(aiObjectIndexToWire),
        nextCursor: result.nextCursor
          ? encodeCursor(JSON.parse(result.nextCursor) as Record<string, string>)
          : null,
      },
      {
        // Codex pass-11 P2: ref-based requests cache short; explicit
        // revision pins go to immutable.
        cache: { mode: revisionRaw ? "revision-long" : "short" },
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
