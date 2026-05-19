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
  readonly params: Promise<{ readonly slug: string }>;
  readonly searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function SiteRoot({ params, searchParams }: Props) {
  const { slug } = await params;
  const sp = await searchParams;
  const refQuery = typeof sp.ref === "string" ? sp.ref : null;
  const ctx = await loadSiteContextForSlug(slug);
  const refs = await loadRefsForSite(ctx);

  let ambiguous = false;
  let activeRef = null;
  try {
    activeRef = await resolveRefOrDefault(ctx, refQuery);
  } catch (error) {
    if (error instanceof PublishApiError && error.code === "AMBIGUOUS_REF") {
      ambiguous = true;
    } else {
      throw error;
    }
  }

  if (refs.length === 0) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="code">
        <EmptyState
          title="No refs published yet"
          description="Run `libra publish sync` from the local repository to publish refs/heads/* and refs/tags/*."
        />
      </SiteShell>
    );
  }

  if (ambiguous) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="code">
        <Toolbar slug={slug} refs={refs} activeRefName={refQuery} />
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

  if (!activeRef) notFound();
  // Root tree never throws FILE_NOT_FOUND (root listing returns []
  // for empty repos), but match the same pattern as the path-based
  // tree page so future changes stay symmetric.
  let tree;
  try {
    tree = await loadTreeForRef(ctx, activeRef, "");
  } catch (error) {
    if (error instanceof PublishApiError && error.code === "FILE_NOT_FOUND") {
      notFound();
    }
    throw error;
  }
  if (!tree) notFound();

  return (
    <SiteShell site={ctx.siteWire} activeNav="code">
      <Toolbar slug={slug} refs={refs} activeRefName={activeRef.ref_name} />
      <Breadcrumbs slug={slug} path="" refQuery={refQuery ?? activeRef.ref_name} mode="tree" />
      <p className="mt-2 text-xs libra-text-muted">
        Revision <span className="libra-mono">{tree.revision.revisionOid.slice(0, 12)}</span>
        {" · "}
        {tree.revision.fileCount.toLocaleString()} files
        {" · "}
        redaction <span className="libra-mono">{tree.revision.redactionMode}</span>
      </p>
      <div className="mt-4">
        <TreeListing
          slug={slug}
          basePath=""
          refQuery={refQuery ?? activeRef.ref_name}
          entries={tree.entries}
        />
      </div>
    </SiteShell>
  );
}

function Toolbar({
  slug,
  refs,
  activeRefName,
}: {
  readonly slug: string;
  readonly refs: ReadonlyArray<{ readonly refName: string; readonly refType: "branch" | "tag"; readonly shortName: string; readonly targetOid: string; readonly revisionOid: string; readonly isDefault: boolean; readonly updatedAt: string }>;
  readonly activeRefName: string | null;
}) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-3">
      <RefPicker slug={slug} refs={refs} active={activeRefName} />
    </div>
  );
}
