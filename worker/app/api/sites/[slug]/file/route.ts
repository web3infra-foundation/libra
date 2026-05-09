import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { findFileRow, resolveRevision } from "@/lib/server/d1";
import { readPublishedTextFile, sha256Hex } from "@/lib/server/r2";
import { respondError, respondOk } from "@/lib/server/response";
import { fileToWire, revisionToWire } from "@/lib/server/wire";
import { notFound } from "@/lib/server/errors";
import { parsePath, parseRevisionOid, parseSlug } from "@/lib/server/validate";

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
    const pathRaw = url.searchParams.get("path");
    const path = parsePath(pathRaw);
    if (revisionRaw) parseRevisionOid(revisionRaw);

    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);
    const fileRow = await findFileRow(bindings.db, site.site_id, revision.revision_oid, path);
    if (!fileRow) {
      throw notFound("FILE_NOT_FOUND", `path is not part of this revision: ${path}`);
    }

    // Codex pass-11 P2: ref-based requests cache short with an ETag
    // (the underlying ref can move on the next sync); explicit
    // `revision=<oid>` requests cache `revision-long` immutably.
    const cacheMode = revisionRaw ? ("revision-long" as const) : ("short" as const);

    if (fileRow.display_mode !== "text") {
      // Metadata-only response. The schema CHECK guarantees no R2 key
      // is recorded for non-text rows; we never fall through to R2.
      // Codex pass-3 P1: the ETag previously interpolated the raw
      // file path, which can contain quotes and other characters
      // that break HTTP header grammar. Hash the path-bound key so
      // the ETag is always quote-safe.
      const etagDigest = (await sha256Hex(
        `${revision.revision_oid}::${fileRow.path}::${fileRow.display_mode}`,
      )).slice(0, 32);
      return respondOk(
        {
          revision: revisionToWire(revision),
          file: fileToWire(fileRow),
          content: null,
        },
        {
          cache: { mode: cacheMode },
          etag: `W/"meta-${etagDigest}"`,
          visibility: site.visibility,
        },
      );
    }

    const content = await readPublishedTextFile(
      bindings.bucket,
      { display_mode: fileRow.display_mode, r2_key: fileRow.r2_key, size_bytes: fileRow.size_bytes },
      fileRow.content_sha256,
    );
    if (!content) {
      // Should be unreachable given schema CHECKs, but fail safely
      // rather than serve stale metadata if somebody bypasses the
      // SQL layer.
      throw notFound("FILE_NOT_FOUND", "file content is missing");
    }

    return respondOk(
      {
        revision: revisionToWire(revision),
        file: fileToWire(fileRow),
        content: { encoding: "utf-8", body: content.body },
      },
      {
        cache: { mode: cacheMode },
        etag: content.etag ?? `W/"${fileRow.content_sha256}"`,
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}
