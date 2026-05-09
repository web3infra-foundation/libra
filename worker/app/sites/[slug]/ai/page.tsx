import { SiteShell } from "@/components/site-shell";
import { EmptyState } from "@/components/empty-state";
import { AiBrowser } from "@/components/ai-browser";
import { RefPicker } from "@/components/ref-picker";
import {
  loadRefsForSite,
  loadSiteContextForSlug,
  resolveRefOrDefault,
} from "@/lib/server/page-helpers";
import { findPublishedRevision } from "@/lib/server/d1";
import { revisionToWire } from "@/lib/server/wire";
import { PublishApiError } from "@/lib/server/errors";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string }>;
  readonly searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function AiBrowserPage({ params, searchParams }: Props) {
  const { slug } = await params;
  const sp = await searchParams;
  const refQuery = typeof sp.ref === "string" ? sp.ref : null;
  const ctx = await loadSiteContextForSlug(slug);
  const refs = await loadRefsForSite(ctx);

  let activeRef = null;
  let ambiguous = false;
  try {
    activeRef = await resolveRefOrDefault(ctx, refQuery);
  } catch (error) {
    if (error instanceof PublishApiError && error.code === "AMBIGUOUS_REF") {
      ambiguous = true;
    } else {
      throw error;
    }
  }

  if (ambiguous) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="ai">
        <EmptyState
          title="Ambiguous ref"
          description={`'${refQuery}' matches both a branch and a tag. Open /sites/${slug}/ai?ref=refs/heads/<name> or /sites/${slug}/ai?ref=refs/tags/<name>.`}
        />
      </SiteShell>
    );
  }
  if (!activeRef) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="ai">
        <EmptyState title="No published AI data" description="No published refs yet." />
      </SiteShell>
    );
  }

  const revisionRow = await findPublishedRevision(
    ctx.bindings.db,
    ctx.site.site_id,
    activeRef.revision_oid,
  );
  if (!revisionRow) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="ai">
        <EmptyState
          title="No published revision for this ref"
          description="The selected ref points at an unpublished or failed revision."
        />
      </SiteShell>
    );
  }
  const revisionWire = revisionToWire(revisionRow);

  if (revisionWire.aiObjectCount === 0) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="ai">
        <Header revision={revisionWire} refs={refs} activeRef={activeRef.ref_name} slug={slug} />
        <EmptyState
          title="No AI objects in this revision"
          description="The published snapshot does not include any AI-object-model entries. Run `libra publish sync` after agent activity to populate this view."
        />
      </SiteShell>
    );
  }

  return (
    <SiteShell site={ctx.siteWire} activeNav="ai">
      <Header revision={revisionWire} refs={refs} activeRef={activeRef.ref_name} slug={slug} />
      <AiBrowser slug={slug} refName={activeRef.ref_name} />
    </SiteShell>
  );
}

function Header({
  slug,
  revision,
  refs,
  activeRef,
}: {
  readonly slug: string;
  readonly revision: { readonly revisionOid: string; readonly aiObjectCount: number; readonly aiBundleCount: number; readonly redactionMode: string; readonly redactionRulesVersion: string };
  readonly refs: readonly { readonly refName: string; readonly refType: "branch" | "tag"; readonly shortName: string; readonly targetOid: string; readonly revisionOid: string; readonly isDefault: boolean; readonly updatedAt: string }[];
  readonly activeRef: string;
}) {
  // Codex pass-13 P2: AI page shows the ref picker so users can
  // switch branches/tags from the AI model view, matching Phase 7
  // behaviour on the code/refs/status pages.
  return (
    <div className="mb-6 space-y-3">
      <RefPicker slug={slug} refs={refs} active={activeRef} />
      <div>
        <h1 className="text-xl font-semibold">AI object model</h1>
        <p className="mt-1 text-sm libra-text-muted">
          Revision <span className="libra-mono">{revision.revisionOid.slice(0, 12)}</span>
          {" · "}
          {revision.aiObjectCount.toLocaleString()} objects
          {" · "}
          {revision.aiBundleCount.toLocaleString()} bundle{revision.aiBundleCount === 1 ? "" : "s"}
          {" · "}
          redaction <span className="libra-mono">{revision.redactionMode}</span>
          {" · "}
          rules <span className="libra-mono">{revision.redactionRulesVersion}</span>
        </p>
      </div>
    </div>
  );
}
