import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { findAiObject, resolveRevision } from "@/lib/server/d1";
import { readPublishedJson } from "@/lib/server/r2";
import { respondError, respondOk } from "@/lib/server/response";
import { aiObjectIndexToWire, revisionToWire } from "@/lib/server/wire";
import { notFound } from "@/lib/server/errors";
import {
  parseObjectId,
  parseObjectType,
  parseRevisionOid,
  parseSlug,
} from "@/lib/server/validate";

export const runtime = "edge";
export const dynamic = "force-dynamic";

export async function GET(
  request: NextRequest,
  context: {
    readonly params: Promise<{ readonly slug: string; readonly type: string; readonly id: string }>;
  },
): Promise<Response> {
  try {
    const { slug: rawSlug, type: rawType, id: rawId } = await context.params;
    const slug = parseSlug(rawSlug);
    const objectType = parseObjectType(rawType);
    const objectId = parseObjectId(rawId);
    const bindings = getBindings();
    const site = await resolveSiteForSlug(bindings, request, slug);
    await gateRequest(bindings, request, site);

    const url = new URL(request.url);
    const refRaw = url.searchParams.get("ref");
    const revisionRaw = url.searchParams.get("revision");
    if (revisionRaw) parseRevisionOid(revisionRaw);
    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);

    const objectRow = await findAiObject(
      bindings.db,
      site.site_id,
      revision.revision_oid,
      objectType,
      objectId,
    );
    if (!objectRow) {
      throw notFound("OBJECT_NOT_FOUND", "no AI object matches this (type, id) at the requested revision");
    }
    // Codex pass-3 P1: verify the R2 body matches
    // `publish_ai_objects.payload_sha256` before parsing/returning.
    // The hash gates the redaction policy recorded alongside the
    // index row; a stale R2 write cannot serve unredacted payloads.
    const payload = await readPublishedJson<Record<string, unknown>>(
      bindings.bucket,
      objectRow.r2_key,
      objectRow.payload_sha256,
    );

    return respondOk(
      {
        revision: revisionToWire(revision),
        index: aiObjectIndexToWire(objectRow),
        payload,
      },
      {
        cache: { mode: "revision-long" },
        etag: `W/"obj-${objectRow.payload_sha256}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
