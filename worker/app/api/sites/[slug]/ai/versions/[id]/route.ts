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

const STORAGE_KEY_FIELDS: ReadonlySet<string> = new Set([
  "r2Key",
  "bundleKey",
  "r2_key",
  "bundle_key",
]);

function redactBundleStorageKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(redactBundleStorageKeys);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      if (STORAGE_KEY_FIELDS.has(key)) continue;
      out[key] = redactBundleStorageKeys(child);
    }
    return out;
  }
  return value;
}

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

    const rawBundle = await readPublishedJson<Record<string, unknown>>(
      bindings.bucket,
      versionRow.bundle_key,
    );
    // Codex pass-3 P1: the canonical AI bundle JSON carries
    // per-object `r2Key` and a top-level `bundleKey` so the CLI can
    // re-hydrate without round-tripping through D1. Those are
    // internal storage paths and MUST NOT leave the Worker — public
    // and even authenticated callers see only D1-rooted indexes.
    // `redactBundleStorageKeys` walks `objects`/`bundles` arrays and
    // strips both keys; everything else passes through unchanged.
    const bundle = redactBundleStorageKeys(rawBundle);

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
