import { SiteShell } from "@/components/site-shell";
import { EmptyState } from "@/components/empty-state";
import { loadSiteContextForSlug } from "@/lib/server/page-helpers";
import { findLatestSyncRun } from "@/lib/server/d1";
import { syncRunToWire } from "@/lib/server/wire";
import { formatDate } from "@/lib/utils";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string }>;
};

export default async function StatusPage({ params }: Props) {
  const { slug } = await params;
  const ctx = await loadSiteContextForSlug(slug);
  const latestRow = await findLatestSyncRun(ctx.bindings.db, ctx.site.site_id);
  const latest = latestRow ? syncRunToWire(latestRow) : null;

  return (
    <SiteShell site={ctx.siteWire} activeNav="status">
      <div className="mb-6">
        <h1 className="text-xl font-semibold">Publish status</h1>
        <p className="mt-1 text-sm libra-text-muted">
          Snapshot of the most recent sync run, visibility configuration and
          clone URLs.
        </p>
      </div>

      <div className="grid gap-6 md:grid-cols-2">
        <section className="libra-card">
          <header className="libra-card-row">
            <h2 className="text-sm font-medium">Site</h2>
            <span
              className={
                ctx.siteWire.status === "active"
                  ? "libra-pill libra-pill-good"
                  : "libra-pill libra-pill-bad"
              }
            >
              {ctx.siteWire.status}
            </span>
          </header>
          <Row label="Visibility" value={ctx.siteWire.visibility} />
          <Row label="Slug" value={ctx.siteWire.slug} mono />
          <Row label="Repo id" value={ctx.siteWire.repoId} mono />
          <Row label="Clone domain" value={ctx.siteWire.cloneDomain} mono />
          <Row label="Display origin" value={ctx.siteWire.displayOrigin} mono />
          <Row label="Default ref" value={ctx.siteWire.defaultRef ?? "—"} mono />
          <Row
            label="Latest revision"
            value={ctx.siteWire.latestRevisionOid ? ctx.siteWire.latestRevisionOid.slice(0, 12) : "—"}
            mono
          />
          <Row label="Refs generation" value={`#${ctx.siteWire.refsGeneration}`} mono />
          <Row label="Updated" value={formatDate(ctx.siteWire.updatedAt)} mono />
        </section>

        <section className="libra-card">
          <header className="libra-card-row">
            <h2 className="text-sm font-medium">Latest sync</h2>
            {latest ? (
              <span
                className={
                  latest.status === "succeeded"
                    ? "libra-pill libra-pill-good"
                    : latest.status === "failed"
                      ? "libra-pill libra-pill-bad"
                      : "libra-pill libra-pill-warn"
                }
              >
                {latest.status}
              </span>
            ) : (
              <span className="libra-pill">never</span>
            )}
          </header>
          {latest ? (
            <>
              <Row label="Run id" value={latest.syncRunId} mono />
              <Row label="Started" value={formatDate(latest.startedAt)} mono />
              <Row label="Finished" value={formatDate(latest.finishedAt)} mono />
              <Row label="Refs" value={latest.refsCount.toLocaleString()} mono />
              <Row label="Revisions" value={latest.revisionCount.toLocaleString()} mono />
              <Row label="Files" value={latest.fileCount.toLocaleString()} mono />
              <Row label="AI objects" value={latest.aiObjectCount.toLocaleString()} mono />
              <Row label="AI bundles" value={latest.aiBundleCount.toLocaleString()} mono />
              <Row label="CLI" value={latest.cliVersion} mono />
              {latest.errorMessage && (
                <div className="px-4 py-3 text-sm text-[var(--bad)]">
                  {latest.errorMessage}
                </div>
              )}
              {latest.warnings.length > 0 && (
                <div className="px-4 py-3 text-sm">
                  <p className="mb-1 text-xs uppercase tracking-wide libra-text-faint">
                    Warnings
                  </p>
                  <ul className="list-disc space-y-1 pl-5 text-sm libra-text-muted">
                    {latest.warnings.map((line, idx) => (
                      <li key={idx}>{line}</li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          ) : (
            <EmptyState
              title="No sync runs yet"
              description="Run `libra publish sync` from the local repository."
            />
          )}
        </section>
      </div>

      <section className="mt-6 libra-card libra-card-pad">
        <h2 className="text-sm font-medium">Restore the repository locally</h2>
        <pre className="libra-codebox mt-3 libra-mono text-xs">
{`libra clone libra+cloud://${ctx.siteWire.cloneDomain}/${ctx.siteWire.slug}
libra clone libra+cloud://${ctx.siteWire.cloneDomain}/repo/${ctx.siteWire.repoId}
libra clone "libra+cloud://${ctx.siteWire.cloneDomain}/${ctx.siteWire.slug}?ref=refs/tags/<tag>"`}
        </pre>
      </section>
    </SiteShell>
  );
}

function Row({ label, value, mono }: { readonly label: string; readonly value: string; readonly mono?: boolean }) {
  return (
    <div className="libra-card-row">
      <span className="text-xs uppercase tracking-wide libra-text-faint">{label}</span>
      <span className={`text-sm ${mono ? "libra-mono" : ""}`}>{value}</span>
    </div>
  );
}
