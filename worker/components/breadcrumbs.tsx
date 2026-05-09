import Link from "next/link";

type BreadcrumbsProps = {
  readonly slug: string;
  readonly path: string;
  readonly refQuery?: string;
  readonly mode: "tree" | "blob";
};

export function Breadcrumbs({ slug, path, refQuery, mode }: BreadcrumbsProps) {
  const segments = path === "" ? [] : path.split("/");
  const refSuffix = refQuery ? `?ref=${encodeURIComponent(refQuery)}` : "";
  return (
    <nav className="flex flex-wrap items-baseline gap-1 text-sm libra-mono">
      <Link
        href={`/sites/${slug}/tree${refSuffix}`}
        className="libra-link"
      >
        {slug}
      </Link>
      {segments.map((segment, idx) => {
        const subPath = segments.slice(0, idx + 1).join("/");
        const isLast = idx === segments.length - 1;
        const href = isLast && mode === "blob"
          ? `/sites/${slug}/blob/${subPath}${refSuffix}`
          : `/sites/${slug}/tree/${subPath}${refSuffix}`;
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
