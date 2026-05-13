import Link from "next/link";
import type { FileEntryWire } from "@/lib/wire-types";
import { encodePathForUrl, formatBytes } from "@/lib/utils";

type TreeListingProps = {
  readonly slug: string;
  readonly basePath: string;
  readonly refQuery?: string;
  readonly entries: readonly FileEntryWire[];
};

export function TreeListing({ slug, basePath, refQuery, entries }: TreeListingProps) {
  const refSuffix = refQuery ? `?ref=${encodeURIComponent(refQuery)}` : "";
  const slugSegment = encodeURIComponent(slug);

  if (entries.length === 0) {
    return (
      <div className="libra-card libra-card-pad text-sm libra-text-muted">
        This folder is empty in the published revision.
      </div>
    );
  }

  return (
    <div className="libra-card overflow-hidden">
      <ul>
        {entries.map((entry) => {
          const childName = basePath === "" ? entry.path : entry.path.slice(basePath.length + 1);
          // Codex pass-1 P2: encode each path segment to keep `?`,
          // `#`, `%`, spaces and other URL-active characters from
          // breaking the route.
          const encodedPath = encodePathForUrl(entry.path);
          const isDirectory = entry.entryKind === "directory";
          const href = isDirectory
            ? `/sites/${slugSegment}/tree/${encodedPath}${refSuffix}`
            : `/sites/${slugSegment}/blob/${encodedPath}${refSuffix}`;
          return (
            <li
              key={entry.path}
              className="flex items-center justify-between gap-3 border-b px-4 py-2.5"
              style={{ borderColor: "var(--line)" }}
            >
              <Link
                href={href}
                className="flex min-w-0 flex-1 items-baseline gap-3 libra-link"
              >
                <span aria-hidden className="libra-text-faint w-5 inline-block">
                  {isDirectory
                    ? "📁"
                    : entry.displayMode === "text"
                      ? "📄"
                      : entry.displayMode === "binary"
                        ? "▦"
                        : entry.displayMode === "too_large"
                          ? "⚠"
                          : "·"}
                </span>
                <span className="truncate libra-mono">{childName}</span>
              </Link>
              <div className="flex items-center gap-3 text-xs libra-text-muted">
                {entry.language && !isDirectory && (
                  <span className="libra-pill">{entry.language}</span>
                )}
                {!isDirectory && entry.displayMode === "binary" && (
                  <span className="libra-pill libra-pill-warn">binary</span>
                )}
                {!isDirectory && entry.displayMode === "too_large" && (
                  <span className="libra-pill libra-pill-warn">too large</span>
                )}
                {!isDirectory && entry.displayMode === "ignored" && (
                  <span className="libra-pill">ignored</span>
                )}
                <span className="libra-mono tabular-nums">
                  {isDirectory ? "—" : formatBytes(entry.sizeBytes)}
                </span>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
