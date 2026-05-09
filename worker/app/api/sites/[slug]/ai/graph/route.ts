import { NextRequest } from "next/server";
import { getBindings } from "@/lib/server/cloudflare";
import { resolveSiteForSlug, gateRequest } from "@/lib/server/site";
import { resolveRevision } from "@/lib/server/d1";
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

type GraphIndex = {
  schemaVersion?: number;
  bundles?: ReadonlyArray<{ aiVersionId: string; bundleKey: string }>;
};

type GraphBundle = {
  schemaVersion?: number;
  nodes?: ReadonlyArray<GraphNode>;
  edges?: ReadonlyArray<GraphEdge>;
  generatedAt?: string;
};

type GraphNode = {
  objectType: string;
  objectId: string;
  layer: "snapshot" | "event" | "projection";
  r2Key?: string;
};

type GraphEdge = {
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

    // Use the revision's index file to find the latest bundle. The
    // index is the same JSON the snapshot builder writes to
    // `ai/index.json`, and the Worker reads it via D1's stored
    // pointer (`publish_revisions.ai_index_key`). We never accept a
    // user-provided R2 key.
    if (!revision.ai_index_key) {
      throw notFound("BUNDLE_NOT_FOUND", "this revision has no AI graph index");
    }
    const index = await readPublishedJson<GraphIndex>(bindings.bucket, revision.ai_index_key);
    const bundleEntry = index.bundles?.[0];
    if (!bundleEntry) {
      throw notFound("BUNDLE_NOT_FOUND", "no AI bundle for this revision");
    }
    const bundle = await readPublishedJson<GraphBundle>(bindings.bucket, bundleEntry.bundleKey);

    const nodes = bundle.nodes ?? [];
    const edges = bundle.edges ?? [];

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
      { cache: { mode: "revision-long" } },
    );
  } catch (error) {
    return respondError(error);
  }
}

