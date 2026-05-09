import { notFound } from "next/navigation";
import { SiteShell } from "@/components/site-shell";
import { Breadcrumbs } from "@/components/breadcrumbs";
import { FileViewer } from "@/components/file-viewer";
import { RefPicker } from "@/components/ref-picker";
import {
  loadFileForRef,
  loadRefsForSite,
  loadSiteContextForSlug,
  resolveRefOrDefault,
} from "@/lib/server/page-helpers";
import { PublishApiError } from "@/lib/server/errors";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string; readonly path: readonly string[] }>;
  readonly searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function BlobPage({ params, searchParams }: Props) {
  const { slug, path: pathSegments } = await params;
  const sp = await searchParams;
  const refQuery = typeof sp.ref === "string" ? sp.ref : null;
  const ctx = await loadSiteContextForSlug(slug);
  const refs = await loadRefsForSite(ctx);

  const path = pathSegments.join("/");
  if (path === "" || path.includes("..") || path.startsWith("/") || path.includes("//") || path.length > 4096) {
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
          <p className="mt-4 text-sm libra-text-muted">
            <span className="libra-mono">{refQuery}</span> matches both a
            branch and a tag. Use the full ref name to disambiguate.
          </p>
        </SiteShell>
      );
    }
    throw error;
  }
  if (!activeRef) notFound();
  const file = await loadFileForRef(ctx, activeRef, path);
  if (!file) notFound();

  return (
    <SiteShell site={ctx.siteWire} activeNav="code">
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <RefPicker slug={slug} refs={refs} active={activeRef.ref_name} />
      </div>
      <Breadcrumbs slug={slug} path={path} refQuery={refQuery ?? activeRef.ref_name} mode="blob" />
      <p className="mt-2 mb-4 text-xs libra-text-muted">
        Revision <span className="libra-mono">{file.revision.revisionOid.slice(0, 12)}</span>
        {" · "}
        sha256 <span className="libra-mono">{file.file.contentSha256?.slice(0, 12) ?? "—"}</span>
      </p>
      <FileViewer file={file.file} content={file.content} />
    </SiteShell>
  );
}
