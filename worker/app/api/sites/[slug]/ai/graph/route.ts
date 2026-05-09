import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { listAiVersions, resolveRevision } from "@/lib/server/d1";
import { readPublishedJson } from "@/lib/server/r2";
import { respondError, respondOk } from "@/lib/server/response";
import { revisionToWire } from "@/lib/server/wire";
import { notFound } from "@/lib/server/errors";
import {
  parseObjectId,
  parseObjectType,
  parseRevisionOid,
  parseSlug,
} from "@/lib/server/validate";

export const runtime = "edge";
export const dynamic = "force-dynamic";

// Codex pass-3 P1: the bundle JSON the snapshot builder writes is a
// `PublishAiBundle` with `objects` + `relationships` (see
// `src/internal/publish/contract.rs`). The previous draft expected
// `nodes` + `edges`, so the graph endpoint silently returned an
// empty graph for every real bundle. Map the canonical fields here
// to the public graph wire shape (`nodes` / `edges`).
type BundleFromR2 = {
  schemaVersion?: number;
  generatedAt?: string;
  objects?: ReadonlyArray<BundleObjectEntry>;
  relationships?: ReadonlyArray<BundleRelationship>;
};

type BundleObjectEntry = {
  objectType: string;
  objectId: string;
  layer: "snapshot" | "event" | "projection";
  // r2Key / payloadSha256 are present in the canonical bundle but
  // are storage-only; we never echo them to the client.
};

type BundleRelationship = {
  kind: string;
  fromObjectType: string;
  fromObjectId: string;
  toObjectType: string;
  toObjectId: string;
};

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
    const rootTypeRaw = url.searchParams.get("rootType");
    const rootIdRaw = url.searchParams.get("rootId");
    if (revisionRaw) parseRevisionOid(revisionRaw);
    const revision = await resolveRevision(bindings.db, site, refRaw, revisionRaw);

    // Codex pass-1 P1: every R2 read MUST source its key from a D1
    // row. An earlier draft read `bundleKey` out of the AI index JSON
    // (which itself lives in R2) and used it as a second R2 key —
    // that lets a poisoned index payload exfiltrate any bucket
    // object. The fix: ask D1 for the bundle row directly, and only
    // use `publish_ai_versions.bundle_key` as the R2 lookup.
    const versions = await listAiVersions(
      bindings.db,
      site.site_id,
      revision.revision_oid,
      1,
    );
    const versionRow = versions.rows[0];
    if (!versionRow) {
      throw notFound("BUNDLE_NOT_FOUND", "no AI bundle for this revision");
    }
    // Codex pass-4 P2 + pass-5 P1: verify the bundle body against the
    // digest recorded in `publish_ai_versions.bundle_sha256` before
    // building the graph. Missing-digest defensive guard mirrors the
    // version detail route.
    if (!versionRow.bundle_sha256 || versionRow.bundle_sha256.length !== 64) {
      throw notFound("BUNDLE_NOT_FOUND", "AI bundle row is missing its sha256 digest");
    }
    const bundle = await readPublishedJson<BundleFromR2>(
      bindings.bucket,
      versionRow.bundle_key,
      versionRow.bundle_sha256,
    );

    const nodes: ReadonlyArray<BundleObjectEntry> = bundle.objects ?? [];
    const edges: ReadonlyArray<BundleRelationship> = bundle.relationships ?? [];

    let filteredNodes = nodes;
    let filteredEdges = edges;
    if (rootTypeRaw && rootIdRaw) {
      const rootType = parseObjectType(rootTypeRaw);
      const rootId = parseObjectId(rootIdRaw);
      // BFS one level out from the root node, matching `appliesTo`,
      // `isPartOf` and `groupedBy` edges in either direction.
      const reachable = new Set<string>();
      const key = (t: string, i: string) => `${t}::${i}`;
      reachable.add(key(rootType, rootId));
      let frontier: ReadonlyArray<{ objectType: string; objectId: string }> = [
        { objectType: rootType, objectId: rootId },
      ];
      for (let depth = 0; depth < 4 && frontier.length > 0; depth += 1) {
        const next: { objectType: string; objectId: string }[] = [];
        for (const edge of edges) {
          for (const root of frontier) {
            if (edge.fromObjectType === root.objectType && edge.fromObjectId === root.objectId) {
              const candidate = key(edge.toObjectType, edge.toObjectId);
              if (!reachable.has(candidate)) {
                reachable.add(candidate);
                next.push({ objectType: edge.toObjectType, objectId: edge.toObjectId });
              }
            }
            if (edge.toObjectType === root.objectType && edge.toObjectId === root.objectId) {
              const candidate = key(edge.fromObjectType, edge.fromObjectId);
              if (!reachable.has(candidate)) {
                reachable.add(candidate);
                next.push({ objectType: edge.fromObjectType, objectId: edge.fromObjectId });
              }
            }
          }
        }
        frontier = next;
      }
      filteredNodes = nodes.filter((node) => reachable.has(key(node.objectType, node.objectId)));
      filteredEdges = edges.filter(
        (edge) =>
          reachable.has(key(edge.fromObjectType, edge.fromObjectId)) &&
          reachable.has(key(edge.toObjectType, edge.toObjectId)),
      );
    }

    return respondOk(
      {
        revision: revisionToWire(revision),
        nodes: filteredNodes.map((node) => ({
          objectType: node.objectType,
          objectId: node.objectId,
          layer: node.layer,
        })),
        edges: filteredEdges,
        generatedAt: bundle.generatedAt ?? null,
      },
      {
        cache: { mode: revisionRaw ? "revision-long" : "short" },
        visibility: site.visibility,
      },
    );
  } catch (error) {
    return respondError(error);
  }
}

