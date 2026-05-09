import Link from "next/link";
import { encodePathForUrl } from "@/lib/utils";

type BreadcrumbsProps = {
  readonly slug: string;
  readonly path: string;
  readonly refQuery?: string;
  readonly mode: "tree" | "blob";
};

export function Breadcrumbs({ slug, path, refQuery, mode }: BreadcrumbsProps) {
  const segments = path === "" ? [] : path.split("/");
  const refSuffix = refQuery ? `?ref=${encodeURIComponent(refQuery)}` : "";
  // Codex pass-1 P2: every URL-bound segment is encoded individually
  // so spaces / `?` / `#` / `%` in repo paths route correctly.
  const slugSegment = encodeURIComponent(slug);
  return (
    <nav className="flex flex-wrap items-baseline gap-1 text-sm libra-mono">
      <Link
        href={`/sites/${slugSegment}/tree${refSuffix}`}
        className="libra-link"
      >
        {slug}
      </Link>
      {segments.map((segment, idx) => {
        const subPath = segments.slice(0, idx + 1).join("/");
        const encodedSubPath = encodePathForUrl(subPath);
        const isLast = idx === segments.length - 1;
        const href = isLast && mode === "blob"
          ? `/sites/${slugSegment}/blob/${encodedSubPath}${refSuffix}`
          : `/sites/${slugSegment}/tree/${encodedSubPath}${refSuffix}`;
        return (
          <span key={subPath} className="contents">
            <span className="libra-text-faint" aria-hidden>/</span>
            {isLast ? (
              <span className="font-medium">{segment}</span>
            ) : (
              <Link href={href} className="libra-link">
                {segment}
              </Link>
            )}
          </span>
        );
      })}
    </nav>
  );
}
