import { notFound } from "next/navigation";
import { SiteShell } from "@/components/site-shell";
import { TreeListing } from "@/components/tree-listing";
import { Breadcrumbs } from "@/components/breadcrumbs";
import { RefPicker } from "@/components/ref-picker";
import { EmptyState } from "@/components/empty-state";
import {
  loadRefsForSite,
  loadSiteContextForSlug,
  loadTreeForRef,
  resolveRefOrDefault,
} from "@/lib/server/page-helpers";
import { PublishApiError } from "@/lib/server/errors";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string; readonly path?: readonly string[] }>;
  readonly searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function TreePage({ params, searchParams }: Props) {
  const { slug, path: pathSegments } = await params;
  const sp = await searchParams;
  const refQuery = typeof sp.ref === "string" ? sp.ref : null;
  const ctx = await loadSiteContextForSlug(slug);
  const refs = await loadRefsForSite(ctx);

  const path = (pathSegments ?? []).join("/");
  // Defence-in-depth: validation matches the API path validator.
  // Codex pass-1 P2: `\0` was missing from the page-side validator.
  if (
    path.includes("..") ||
    path.startsWith("/") ||
    path.includes("//") ||
    path.includes("\0") ||
    path.length > 4096
  ) {
    notFound();
  }

  let activeRef = null;
  try {
    activeRef = await resolveRefOrDefault(ctx, refQuery);
  } catch (error) {
    if (error instanceof PublishApiError && error.code === "AMBIGUOUS_REF") {
      return (
        <SiteShell site={ctx.siteWire} activeNav="code">
          <RefPicker slug={slug} refs={refs} active={refQuery} />
          <div className="mt-6 libra-card libra-card-pad">
            <p className="text-sm font-medium">Ref name is ambiguous</p>
            <p className="mt-1 text-sm libra-text-muted">
              <span className="libra-mono">{refQuery}</span> matches both a
              branch and a tag. Use the full ref name —{" "}
              <span className="libra-mono">refs/heads/{refQuery}</span> or{" "}
              <span className="libra-mono">refs/tags/{refQuery}</span>.
            </p>
          </div>
        </SiteShell>
      );
    }
    throw error;
  }
  if (!activeRef) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="code">
        <EmptyState
          title="No published refs yet"
          description="Run `libra publish sync` to publish branches and tags."
        />
      </SiteShell>
    );
  }
  const tree = await loadTreeForRef(ctx, activeRef, path);
  if (!tree) notFound();

  return (
    <SiteShell site={ctx.siteWire} activeNav="code">
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <RefPicker slug={slug} refs={refs} active={activeRef.ref_name} />
      </div>
      <Breadcrumbs slug={slug} path={path} refQuery={refQuery ?? activeRef.ref_name} mode="tree" />
      <p className="mt-2 text-xs libra-text-muted">
        Revision <span className="libra-mono">{tree.revision.revisionOid.slice(0, 12)}</span>
        {" · "}
        {tree.entries.length.toLocaleString()} entries in this folder
      </p>
      <div className="mt-4">
        {tree.entries.length === 0 ? (
          <EmptyState title="Empty folder" description="No published entries under this path." />
        ) : (
          <TreeListing
            slug={slug}
            basePath={path}
            refQuery={refQuery ?? activeRef.ref_name}
            entries={tree.entries}
          />
        )}
      </div>
    </SiteShell>
  );
}
