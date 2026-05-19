import Link from "next/link";
import { SiteShell } from "@/components/site-shell";
import { EmptyState } from "@/components/empty-state";
import { loadRefsForSite, loadSiteContextForSlug } from "@/lib/server/page-helpers";
import { formatDate } from "@/lib/utils";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string }>;
};

export default async function RefsPage({ params }: Props) {
  const { slug } = await params;
  const ctx = await loadSiteContextForSlug(slug);
  const refs = await loadRefsForSite(ctx);
  const branches = refs.filter((r) => r.refType === "branch");
  const tags = refs.filter((r) => r.refType === "tag");

  return (
    <SiteShell site={ctx.siteWire} activeNav="refs">
      <div className="mb-6">
        <h1 className="text-xl font-semibold">Published refs</h1>
        <p className="mt-1 text-sm libra-text-muted">
          Branches under <span className="libra-mono">refs/heads/*</span> and
          tags under <span className="libra-mono">refs/tags/*</span> from the
          most recent full sync.
        </p>
      </div>
      {refs.length === 0 ? (
        <EmptyState
          title="No refs published yet"
          description="Run `libra publish sync` from the local repository."
        />
      ) : (
        <div className="grid gap-6 md:grid-cols-2">
          <RefSection slug={slug} title="Branches" refs={branches} />
          <RefSection slug={slug} title="Tags" refs={tags} />
        </div>
      )}
    </SiteShell>
  );
}

function RefSection({
  slug,
  title,
  refs,
}: {
  readonly slug: string;
  readonly title: string;
  readonly refs: ReadonlyArray<{
    readonly refName: string;
    readonly refType: "branch" | "tag";
    readonly shortName: string;
    readonly targetOid: string;
    readonly revisionOid: string;
    readonly isDefault: boolean;
    readonly updatedAt: string;
  }>;
}) {
  return (
    <section>
      <h2 className="mb-2 text-sm font-medium uppercase tracking-wide libra-text-muted">
        {title} · {refs.length}
      </h2>
      {refs.length === 0 ? (
        <p className="libra-card libra-card-pad text-sm libra-text-muted">
          No {title.toLowerCase()} published.
        </p>
      ) : (
        <ul className="libra-card divide-y" style={{ borderColor: "var(--line)" }}>
          {refs.map((ref) => (
            <li key={ref.refName} className="libra-card-row">
              <div className="min-w-0">
                <Link
                  href={`/sites/${slug}?ref=${encodeURIComponent(ref.refName)}`}
                  className="libra-link libra-mono truncate"
                >
                  {ref.shortName}
                </Link>
                {ref.isDefault && (
                  <span className="ml-2 libra-pill libra-pill-accent">default</span>
                )}
                <p className="mt-1 text-xs libra-text-faint libra-mono truncate">
                  {ref.refName}
                </p>
              </div>
              <div className="text-right text-xs libra-text-muted">
                <p className="libra-mono">{ref.revisionOid.slice(0, 12)}</p>
                <p className="libra-mono">{formatDate(ref.updatedAt)}</p>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
