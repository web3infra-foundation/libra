import { redirect } from "next/navigation";
import { loadSiteContextForRepoId } from "@/lib/server/page-helpers";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly repoId: string }>;
};

export default async function RepoStableEntry({ params }: Props) {
  const { repoId } = await params;
  const ctx = await loadSiteContextForRepoId(repoId);
  redirect(`/sites/${ctx.siteWire.slug}`);
}
